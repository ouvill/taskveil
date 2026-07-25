use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use taskveil_domain::{List, Task, TaskContent, TaskSeries, TaskSeriesConfig, TaskTemplate, Uuid};

use crate::{
    account::{AccountClient, AccountClientError},
    decrypt_plaintext, merge_lww, EncryptedSyncState, EnvelopeError, Hlc, PullRecord, PushOp,
    PushStatus, SyncCollection, SyncEngine, SyncEngineError, SyncPlaintext, SyncRunSummary,
    LISTS_COLLECTION, SYNC_CURSOR_NAME, SYNC_UPGRADE_REQUIRED_SETTING_KEY, TASKS_COLLECTION,
    TASK_SERIES_COLLECTION, TEMPLATES_COLLECTION,
};

use crate::enqueue::{
    enqueue_merged_plaintext, enqueue_rebased_tombstone, enqueue_task_sync,
    enqueue_timer_session_sync, list_plaintext, observe_remote_hlc, task_plaintext,
    task_series_plaintext, template_plaintext, LocalFullResyncPhase, LocalListAlias,
    LocalSyncAtomicStore, LocalSyncQuarantineEntry, LocalSyncRecordState, LocalSyncSemanticState,
    LocalSyncStore, LocalSyncWriteTransaction, PullFailureReason, RebasePlaintextRequest,
    RebaseTombstoneRequest,
};
use crate::keys::{tenant_root_dek, LocalSyncKeys};

const PUSH_BATCH_LIMIT: usize = 100;
const MAX_PUSH_DRAIN_ITERATIONS: usize = 100;
const QUARANTINE_REPLAY_BATCH_LIMIT: usize = 100;
const FULL_RESYNC_PAGE_LIMIT: i64 = 100;
const FULL_RESYNC_SWEEP_BATCH_LIMIT: usize = 100;

fn sync_engine_error_to_string(error: SyncEngineError) -> String {
    match error {
        SyncEngineError::Server(status) if status == reqwest::StatusCode::UNAUTHORIZED => {
            "unauthorized".to_string()
        }
        SyncEngineError::EntitlementRequired => "entitlement required".to_string(),
        SyncEngineError::UpgradeRequired { .. } => "upgrade required".to_string(),
        _ => "sync failed".to_string(),
    }
}

fn account_sync_error_to_string(error: AccountClientError) -> String {
    match error {
        AccountClientError::Server(401) => "unauthorized".to_string(),
        AccountClientError::EntitlementRequired => "entitlement required".to_string(),
        _ => "sync failed".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyDisposition {
    AppliedCurrent,
    Rebased,
    Deferred(PullFailureReason, Option<Uuid>),
    UpgradeRequired(u8),
}

enum TaskDependencyDisposition {
    Valid,
    Missing,
    Deleted,
}

pub trait SyncKeyRefresher {
    fn refresh<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<LocalSyncKeys, String>> + Send + 'a>>;
}

struct UnavailableKeyRefresher;

impl SyncKeyRefresher for UnavailableKeyRefresher {
    fn refresh<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<LocalSyncKeys, String>> + Send + 'a>> {
        Box::pin(async { Err("key refresh unavailable".to_string()) })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSyncContext {
    pub server_url: String,
    pub tenant_id: Uuid,
    pub device_id: String,
    pub session_token: crate::SecretString,
    pub keys: LocalSyncKeys,
    pub manifest_auth_key: zeroize::Zeroizing<[u8; 32]>,
}

pub async fn run_sync_now<S, N>(
    context: ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
) -> Result<SyncRunSummary, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    run_sync_now_with_key_refresh(context, store, now_ms, &mut UnavailableKeyRefresher).await
}

pub async fn run_sync_now_with_key_refresh<S, N, R>(
    context: ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    key_refresher: &mut R,
) -> Result<SyncRunSummary, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    R: SyncKeyRefresher,
{
    let mut no_pre_push = |_store: &mut S| Ok(());
    run_sync_now_with_key_refresh_and_pre_push(
        context,
        store,
        now_ms,
        key_refresher,
        &mut no_pre_push,
    )
    .await
}

pub async fn run_sync_now_with_key_refresh_and_pre_push<S, N, R, P>(
    mut context: ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    key_refresher: &mut R,
    pre_push: &mut P,
) -> Result<SyncRunSummary, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    R: SyncKeyRefresher,
    P: FnMut(&mut S) -> Result<(), String>,
{
    let engine = SyncEngine::new(
        context.server_url.clone(),
        context.tenant_id,
        context.session_token.expose(),
    )
    .map_err(|_| "sync failed".to_string())?;
    let mut summary = SyncRunSummary::default();

    let durable_upgrade_block = store.get_setting(SYNC_UPGRADE_REQUIRED_SETTING_KEY)?;
    if durable_upgrade_block
        .as_deref()
        .is_some_and(upgrade_block_is_active)
    {
        return Err("upgrade required".to_string());
    }
    let since = store.get_cursor_seq(SYNC_CURSOR_NAME)?.unwrap_or(0);
    let preflight = match engine.preflight(since).await {
        Ok(preflight) => {
            if durable_upgrade_block.is_some() {
                store.set_setting(SYNC_UPGRADE_REQUIRED_SETTING_KEY, "0:0", now_ms()?)?;
            }
            preflight
        }
        Err(SyncEngineError::UpgradeRequired {
            protocol_version,
            envelope_version,
        }) => {
            store.set_setting(
                SYNC_UPGRADE_REQUIRED_SETTING_KEY,
                &upgrade_block_value(protocol_version, envelope_version),
                now_ms()?,
            )?;
            return Err("upgrade required".to_string());
        }
        Err(SyncEngineError::EntitlementRequired) => {
            return Err("entitlement required".to_string());
        }
        Err(error) => return Err(sync_engine_error_to_string(error)),
    };
    if validate_preflight_key_state(&context, &preflight, store, now_ms).is_err() {
        context.keys = key_refresher.refresh().await?;
        validate_preflight_key_state(&context, &preflight, store, now_ms)?;
    }
    // Initial local rows must have durable outbox protection before any remote
    // absence sweep. This hook is intentionally before key-bundle/entity push.
    pre_push(store)?;

    let ran_full_resync = store.load_full_resync()?.is_some() || preflight.full_resync_required;
    let refreshed_in_full_resync = if ran_full_resync {
        run_full_resync(
            &engine,
            &mut context,
            store,
            now_ms,
            key_refresher,
            &mut summary,
        )
        .await?
    } else {
        false
    };

    // ADR-016: a normal device must reconcile every remote head visible at
    // preflight before it may upload keys or entities. This closes the stale
    // outbox / late-descendant window that existed when push preceded pull.
    let refreshed_in_normal_pull = if !ran_full_resync {
        pull_to_closure(
            &engine,
            &mut context,
            store,
            now_ms,
            key_refresher,
            &mut summary,
        )
        .await?
    } else {
        false
    };

    // A full resync already retried missing-key records once and classified
    // the remaining failures durably. Replaying them immediately would issue
    // the same key refresh twice in one run without any new server state.
    if !store.list_replayable_quarantine(None, 1)?.is_empty() {
        if !refreshed_in_full_resync
            && !refreshed_in_normal_pull
            && !store.list_replayable_quarantine(None, 1)?.is_empty()
        {
            context.keys = key_refresher.refresh().await?;
        }
        if let Err(error) = replay_quarantine(&context, store, now_ms, &mut summary) {
            if let Some(envelope_version) = replay_upgrade_version(&error) {
                store.set_setting(
                    SYNC_UPGRADE_REQUIRED_SETTING_KEY,
                    &upgrade_block_value(crate::protocol::SYNC_PROTOCOL_VERSION, envelope_version),
                    now_ms()?,
                )?;
                return Err("upgrade required".to_string());
            }
            return Err(error);
        }
    }

    // ADR-015: only a closed, fully classified remote view may elect the
    // canonical Inbox. The owned transaction also makes aliases visible to UI
    // readers only after every known task has moved and been re-encrypted.
    reconcile_canonical_inbox(&context, store, now_ms)?;

    for _ in 0..MAX_PUSH_DRAIN_ITERATIONS {
        let outbox = store.list_outbox_heads(PUSH_BATCH_LIMIT)?;
        if outbox.is_empty() {
            break;
        }
        summary.pushed_count += outbox.len();
        let revisions = outbox
            .iter()
            .map(|entry| (entry.op_id, entry.revision_hlc.clone()))
            .collect::<HashMap<_, _>>();
        let push_ops = outbox
            .into_iter()
            .map(|entry| PushOp {
                op_id: entry.op_id,
                record_id: entry.record_id,
                collection: entry.collection,
                base_revision_hlc: entry.base_revision_hlc,
                revision_hlc: entry.revision_hlc,
                state: entry.state,
            })
            .collect::<Vec<_>>();
        let push_outcome = engine
            .push_batch(push_ops)
            .await
            .map_err(sync_engine_error_to_string)?;
        for outcome in push_outcome.outcomes {
            match outcome.status {
                PushStatus::Accepted | PushStatus::NoOp => {
                    let revision_hlc = revisions
                        .get(&outcome.op_id)
                        .ok_or_else(|| "sync failed".to_string())?;
                    let mut transaction = store.begin_write_transaction()?;
                    if transaction.ack_outbox_op(outcome.op_id)? {
                        update_current_revision(
                            &mut transaction,
                            outcome.collection,
                            outcome.record_id,
                            revision_hlc,
                            now_ms,
                        )?;
                        summary.push_acked_count += 1;
                    }
                    transaction.commit()?;
                }
                PushStatus::Superseded => {
                    let current = outcome
                        .current
                        .as_ref()
                        .ok_or_else(|| "sync failed".to_string())?;
                    let mut transaction = store.begin_write_transaction()?;
                    reconcile_nonaccepted_push_in_transaction(
                        current,
                        outcome.op_id,
                        &context,
                        &mut transaction,
                        now_ms,
                        &mut summary,
                    )?;
                    transaction.commit()?;
                    summary.push_superseded_count += 1;
                }
                PushStatus::Conflict => {
                    let current = outcome
                        .current
                        .as_ref()
                        .ok_or_else(|| "sync failed".to_string())?;
                    let mut transaction = store.begin_write_transaction()?;
                    reconcile_nonaccepted_push_in_transaction(
                        current,
                        outcome.op_id,
                        &context,
                        &mut transaction,
                        now_ms,
                        &mut summary,
                    )?;
                    transaction.commit()?;
                    summary.push_conflict_count += 1;
                }
            }
        }
    }

    if !store.list_outbox_heads(1)?.is_empty() {
        return Err("sync failed".to_string());
    }
    if preflight.active_key_generation > 1 {
        AccountClient::new(context.server_url.clone())
            .map_err(|_| "sync failed".to_string())?
            .acknowledge_key_generation(
                context.tenant_id,
                preflight.active_key_generation,
                context.session_token.expose(),
            )
            .await
            .map_err(account_sync_error_to_string)?;
    }

    Ok(summary)
}

fn validate_preflight_key_state<S, N>(
    context: &ActiveSyncContext,
    preflight: &crate::PreflightResult,
    store: &mut S,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    if preflight.suite_id != taskveil_crypto::CRYPTO_SUITE_ID
        || context.keys.tenant_id != context.tenant_id
        || context.keys.tenant_generation != preflight.active_key_generation
        || context.keys.tenant_generation < preflight.minimum_write_generation
        || preflight
            .migrating_key_generation
            .is_some_and(|generation| {
                context
                    .keys
                    .historical_tenant_root_deks
                    .iter()
                    .all(|(candidate, _)| *candidate != generation)
            })
    {
        return Err("active key generation required".to_string());
    }
    let tenant_manifest = preflight
        .key_manifests
        .first()
        .ok_or_else(|| "authenticated key manifest required".to_string())?;
    let tenant = verify_preflight_manifest(tenant_manifest, context)?;
    verify_manifest_anchor(tenant_manifest, &tenant, context, store)?;
    if tenant_manifest.generation != context.keys.tenant_generation {
        return Err("active key generation required".to_string());
    }
    let accepted = vec![(
        manifest_anchor_key(),
        tenant_manifest.signed_manifest.clone(),
    )];
    let updated_at = now_ms()?;
    for (key, value) in accepted {
        store.set_setting(&key, &value, updated_at)?;
    }
    Ok(())
}

fn verify_preflight_manifest(
    descriptor: &crate::protocol::KeyManifestDescriptor,
    context: &ActiveSyncContext,
) -> Result<crate::KeyManifest, String> {
    let bytes = STANDARD
        .decode(&descriptor.signed_manifest)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    let manifest = crate::KeyManifest::from_authenticated_bytes(&bytes)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    if manifest.tenant_id != context.tenant_id
        || manifest.suite_id != descriptor.suite_id
        || manifest.generation != descriptor.generation
        || manifest.status != descriptor.status
        || manifest.minimum_write_generation != descriptor.minimum_write_generation
        || manifest.status != crate::RotationStatus::Active
    {
        return Err("authenticated key manifest required".to_string());
    }
    manifest
        .verify_personal_with_auth_key(&context.manifest_auth_key)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    Ok(manifest)
}

fn manifest_anchor_key() -> String {
    "key_manifest_anchor:tenant".to_string()
}

fn verify_manifest_anchor<S: LocalSyncStore>(
    descriptor: &crate::protocol::KeyManifestDescriptor,
    current: &crate::KeyManifest,
    context: &ActiveSyncContext,
    store: &mut S,
) -> Result<(), String> {
    let key = manifest_anchor_key();
    let Some(encoded_anchor) = store.get_setting(&key)? else {
        return Ok(());
    };
    let anchor_bytes = STANDARD
        .decode(encoded_anchor)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    let mut anchor = crate::KeyManifest::from_authenticated_bytes(&anchor_bytes)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    anchor
        .verify_personal_with_auth_key(&context.manifest_auth_key)
        .map_err(|_| "authenticated key manifest required".to_string())?;
    if anchor
        .authenticated_hash()
        .map_err(|_| "sync failed".to_string())?
        == current
            .authenticated_hash()
            .map_err(|_| "sync failed".to_string())?
    {
        return Ok(());
    }
    for encoded in &descriptor.predecessor_manifests {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "authenticated key manifest required".to_string())?;
        let next = crate::KeyManifest::from_authenticated_bytes(&bytes)
            .map_err(|_| "authenticated key manifest required".to_string())?;
        anchor
            .verify_successor_with_auth_key(&next, &context.manifest_auth_key)
            .map_err(|_| "authenticated key manifest required".to_string())?;
        anchor = next;
    }
    anchor
        .verify_successor_with_auth_key(current, &context.manifest_auth_key)
        .map_err(|_| "authenticated key manifest required".to_string())
}

fn reconcile_canonical_inbox<S, N>(
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut transaction = store.begin_write_transaction()?;
    reconcile_canonical_inbox_in_transaction(context, &mut transaction, now_ms)?;
    transaction.commit()
}

fn reconcile_canonical_inbox_in_transaction<S, N>(
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    if store.has_live_quarantine(SyncCollection::Lists)? {
        return Ok(());
    }

    let mut candidates = Vec::new();
    for (record_id, state) in store.list_record_states(SyncCollection::Lists)? {
        let LocalSyncSemanticState::Live { plaintext_json, .. } = state.state else {
            continue;
        };
        let plaintext: SyncPlaintext =
            serde_json::from_str(&plaintext_json).map_err(|_| "sync failed".to_string())?;
        plaintext
            .validate_for_collection(LISTS_COLLECTION, &record_id.to_string())
            .map_err(|_| "sync failed".to_string())?;
        let SyncPlaintext::List(list) = plaintext else {
            return Err("sync failed".to_string());
        };
        if list.is_default.value {
            candidates.push(record_id);
        }
    }
    candidates.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    candidates.dedup();
    let Some(canonical_id) = candidates.first().copied() else {
        return Ok(());
    };
    if tenant_root_dek(&context.keys).is_none() {
        return Err("sync failed".to_string());
    }

    let existing_aliases = store.list_list_aliases()?;
    let mut alias_ids = existing_aliases
        .iter()
        .map(|alias| alias.alias_list_id)
        .chain(candidates.iter().copied().skip(1))
        .filter(|alias_id| *alias_id != canonical_id)
        .collect::<Vec<_>>();
    alias_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    alias_ids.dedup();
    let aliases = alias_ids
        .iter()
        .copied()
        .map(|alias_list_id| LocalListAlias {
            alias_list_id,
            canonical_list_id: canonical_id,
        })
        .collect::<Vec<_>>();

    // Validate that every candidate was materialized before changing any row.
    // A live list quarantine is handled above; a missing row here is a local
    // consistency failure and must roll the owned transaction back.
    let _candidate_lists = candidates
        .iter()
        .copied()
        .map(|id| store.get_list(id)?.ok_or_else(|| "sync failed".to_string()))
        .collect::<Result<Vec<_>, String>>()?;
    store.materialize_canonical_list(canonical_id)?;

    for mut task in store.list_all_tasks_for_sync()? {
        if alias_ids.binary_search(&task.list_id).is_err() {
            continue;
        }
        task.list_id = canonical_id;
        store.upsert_task_for_sync(task.clone())?;
        enqueue_task_sync(
            store,
            &context.keys,
            &context.device_id,
            &task,
            false,
            now_ms,
        )?;
    }

    let mut normalized_existing = existing_aliases;
    normalized_existing.sort_by(|left, right| {
        left.alias_list_id
            .as_bytes()
            .cmp(right.alias_list_id.as_bytes())
            .then_with(|| {
                left.canonical_list_id
                    .as_bytes()
                    .cmp(right.canonical_list_id.as_bytes())
            })
    });
    if normalized_existing != aliases {
        store.replace_list_aliases(&aliases, now_ms()?)?;
    }
    Ok(())
}

async fn pull_to_closure<S, N, R>(
    engine: &SyncEngine,
    context: &mut ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    key_refresher: &mut R,
    summary: &mut SyncRunSummary,
) -> Result<bool, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    R: SyncKeyRefresher,
{
    let mut refreshed = false;
    loop {
        let since = store.get_cursor_seq(SYNC_CURSOR_NAME)?.unwrap_or(0);
        let page = engine
            .pull_page(since, 100)
            .await
            .map_err(sync_engine_error_to_string)?;
        match apply_pull_page(&page, context, store, now_ms, false) {
            Ok(page_summary) => merge_summary(summary, page_summary),
            Err(PageApplyError::MissingKey) => {
                context.keys = key_refresher.refresh().await?;
                refreshed = true;
                if let Err(error) = replay_quarantine(context, store, now_ms, summary) {
                    if let Some(envelope_version) = replay_upgrade_version(&error) {
                        store.set_setting(
                            SYNC_UPGRADE_REQUIRED_SETTING_KEY,
                            &upgrade_block_value(
                                crate::protocol::SYNC_PROTOCOL_VERSION,
                                envelope_version,
                            ),
                            now_ms()?,
                        )?;
                        return Err("upgrade required".to_string());
                    }
                    return Err(error);
                }
                let page_summary = apply_pull_page(&page, context, store, now_ms, true)
                    .map_err(page_apply_error_to_string)?;
                merge_summary(summary, page_summary);
            }
            Err(PageApplyError::UpgradeRequired(envelope_version)) => {
                store.set_setting(
                    SYNC_UPGRADE_REQUIRED_SETTING_KEY,
                    &upgrade_block_value(crate::protocol::SYNC_PROTOCOL_VERSION, envelope_version),
                    now_ms()?,
                )?;
                return Err("upgrade required".to_string());
            }
            Err(error) => return Err(page_apply_error_to_string(error)),
        }
        if !page.has_more {
            let proof = page
                .closure_proof
                .clone()
                .filter(|_| page.reached_closure())
                .ok_or_else(|| "sync failed".to_string())?;
            engine
                .ack_continuity(proof)
                .await
                .map_err(sync_engine_error_to_string)?;
            return Ok(refreshed);
        }
    }
}

async fn run_full_resync<S, N, R>(
    engine: &SyncEngine,
    context: &mut ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    key_refresher: &mut R,
    summary: &mut SyncRunSummary,
) -> Result<bool, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    R: SyncKeyRefresher,
{
    let mut refreshed_keys = false;
    if store
        .load_full_resync()
        .map_err(|_| "sync failed".to_string())?
        .is_none()
    {
        let start = engine
            .begin_full_resync()
            .await
            .map_err(sync_engine_error_to_string)?;
        let mut transaction = store
            .begin_write_transaction()
            .map_err(|_| "sync failed".to_string())?;
        transaction
            .start_full_resync(Uuid::now_v7(), start.generation, start.base_seq, now_ms()?)
            .map_err(|_| "sync failed".to_string())?;
        transaction
            .commit()
            .map_err(|_| "sync failed".to_string())?;
    }

    loop {
        let progress = store
            .load_full_resync()
            .map_err(|_| "sync failed".to_string())?
            .ok_or_else(|| "sync failed".to_string())?;
        match progress.phase {
            LocalFullResyncPhase::Base => {
                let page = engine
                    .scan_base_page(
                        progress.continuity_generation,
                        progress.base_cursor.as_ref(),
                        FULL_RESYNC_PAGE_LIMIT,
                    )
                    .await
                    .map_err(sync_engine_error_to_string)?;
                if page.has_more && page.next_cursor.is_none() {
                    return Err("sync failed".to_string());
                }
                let base_complete = !page.has_more;
                let page_updated_at = now_ms()?;
                let apply = apply_full_resync_page(
                    &page.records,
                    context,
                    store,
                    now_ms,
                    false,
                    |transaction| {
                        transaction.advance_full_resync_base(
                            progress.generation_id,
                            page.next_cursor.as_ref(),
                            base_complete,
                            page_updated_at,
                        )
                    },
                );
                let page_summary = match apply {
                    Ok(summary) => summary,
                    Err(PageApplyError::MissingKey) => {
                        context.keys = key_refresher.refresh().await?;
                        refreshed_keys = true;
                        match apply_full_resync_page(
                            &page.records,
                            context,
                            store,
                            now_ms,
                            true,
                            |transaction| {
                                transaction.advance_full_resync_base(
                                    progress.generation_id,
                                    page.next_cursor.as_ref(),
                                    base_complete,
                                    page_updated_at,
                                )
                            },
                        ) {
                            Ok(summary) => summary,
                            Err(PageApplyError::UpgradeRequired(version)) => {
                                persist_full_resync_upgrade_block(store, now_ms, version)?;
                                return Err("upgrade required".to_string());
                            }
                            Err(error) => return Err(page_apply_error_to_string(error)),
                        }
                    }
                    Err(PageApplyError::UpgradeRequired(version)) => {
                        persist_full_resync_upgrade_block(store, now_ms, version)?;
                        return Err("upgrade required".to_string());
                    }
                    Err(error) => return Err(page_apply_error_to_string(error)),
                };
                merge_summary(summary, page_summary);
            }
            LocalFullResyncPhase::Delta => {
                let page = engine
                    .pull_page_for_generation(
                        progress.delta_cursor,
                        FULL_RESYNC_PAGE_LIMIT,
                        Some(progress.continuity_generation),
                    )
                    .await
                    .map_err(sync_engine_error_to_string)?;
                let reached_closure = page.reached_closure();
                let page_updated_at = now_ms()?;
                let apply = apply_full_resync_page(
                    &page.records,
                    context,
                    store,
                    now_ms,
                    false,
                    |transaction| {
                        transaction.advance_full_resync_delta(
                            progress.generation_id,
                            page.next_since,
                            page_updated_at,
                        )?;
                        if reached_closure {
                            transaction.enter_full_resync_sweep(
                                progress.generation_id,
                                page.high_water,
                                page_updated_at,
                            )?;
                        }
                        Ok(())
                    },
                );
                let page_summary = match apply {
                    Ok(summary) => summary,
                    Err(PageApplyError::MissingKey) => {
                        context.keys = key_refresher.refresh().await?;
                        refreshed_keys = true;
                        match apply_full_resync_page(
                            &page.records,
                            context,
                            store,
                            now_ms,
                            true,
                            |transaction| {
                                transaction.advance_full_resync_delta(
                                    progress.generation_id,
                                    page.next_since,
                                    page_updated_at,
                                )?;
                                if reached_closure {
                                    transaction.enter_full_resync_sweep(
                                        progress.generation_id,
                                        page.high_water,
                                        page_updated_at,
                                    )?;
                                }
                                Ok(())
                            },
                        ) {
                            Ok(summary) => summary,
                            Err(PageApplyError::UpgradeRequired(version)) => {
                                persist_full_resync_upgrade_block(store, now_ms, version)?;
                                return Err("upgrade required".to_string());
                            }
                            Err(error) => return Err(page_apply_error_to_string(error)),
                        }
                    }
                    Err(PageApplyError::UpgradeRequired(version)) => {
                        persist_full_resync_upgrade_block(store, now_ms, version)?;
                        return Err("upgrade required".to_string());
                    }
                    Err(error) => return Err(page_apply_error_to_string(error)),
                };
                merge_summary(summary, page_summary);
            }
            LocalFullResyncPhase::Sweep => {
                let mut transaction = store
                    .begin_write_transaction()
                    .map_err(|_| "sync failed".to_string())?;
                let swept = transaction
                    .sweep_full_resync_batch(
                        progress.generation_id,
                        FULL_RESYNC_SWEEP_BATCH_LIMIT,
                        now_ms()?,
                    )
                    .map_err(|_| "sync failed".to_string())?;
                transaction
                    .commit()
                    .map_err(|_| "sync failed".to_string())?;
                summary.deleted_count += swept.swept_lists
                    + swept.swept_tasks
                    + swept.swept_templates
                    + swept.swept_task_series
                    + swept.swept_timer_sessions;
                if swept.scanned_records == 0 {
                    let mut transaction = store
                        .begin_write_transaction()
                        .map_err(|_| "sync failed".to_string())?;
                    let high_water = transaction
                        .finalize_full_resync(progress.generation_id, SYNC_CURSOR_NAME, now_ms()?)
                        .map_err(|_| "sync failed".to_string())?;
                    transaction
                        .commit()
                        .map_err(|_| "sync failed".to_string())?;
                    let closure = engine
                        .pull_page_for_generation(
                            high_water,
                            FULL_RESYNC_PAGE_LIMIT,
                            Some(progress.continuity_generation),
                        )
                        .await
                        .map_err(sync_engine_error_to_string)?;
                    let proof = closure
                        .closure_proof
                        .clone()
                        .filter(|_| closure.reached_closure())
                        .ok_or_else(|| "sync failed".to_string())?;
                    engine
                        .ack_continuity(proof)
                        .await
                        .map_err(sync_engine_error_to_string)?;
                    return Ok(refreshed_keys);
                }
            }
        }
    }
}

fn persist_full_resync_upgrade_block<S, N>(
    store: &mut S,
    now_ms: &mut N,
    envelope_version: u8,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    store.set_setting(
        SYNC_UPGRADE_REQUIRED_SETTING_KEY,
        &upgrade_block_value(crate::protocol::SYNC_PROTOCOL_VERSION, envelope_version),
        now_ms()?,
    )
}

fn apply_full_resync_page<S, N, F>(
    records: &[PullRecord],
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    quarantine_missing: bool,
    finish: F,
) -> Result<SyncRunSummary, PageApplyError>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    F: FnOnce(&mut S::WriteTransaction) -> Result<(), String>,
{
    let progress = store
        .load_full_resync()
        .map_err(|_| PageApplyError::Hard)?
        .ok_or(PageApplyError::Hard)?;
    let mut transaction = store
        .begin_write_transaction()
        .map_err(|_| PageApplyError::Hard)?;
    let mut page_summary = SyncRunSummary {
        pulled_count: records.len(),
        ..SyncRunSummary::default()
    };
    for record in records {
        let disposition =
            apply_pull_record(record, context, &mut transaction, now_ms, &mut page_summary)
                .map_err(|_| PageApplyError::Hard)?;
        match disposition {
            ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => {
                if transaction
                    .delete_quarantine(record.record_id)
                    .map_err(|_| PageApplyError::Hard)?
                {
                    page_summary.resolved_quarantine_count += 1;
                }
            }
            ApplyDisposition::Deferred(reason, required_list_id) => {
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) && !quarantine_missing
                {
                    return Err(PageApplyError::MissingKey);
                }
                let failed_at = now_ms().map_err(|_| PageApplyError::Hard)?;
                transaction
                    .put_quarantine(LocalSyncQuarantineEntry {
                        record_id: record.record_id,
                        collection: record.collection,
                        seq: record.seq,
                        revision_hlc: record.revision_hlc.clone(),
                        state: record.state.clone(),
                        reason,
                        required_list_id,
                        first_failed_at: failed_at,
                        last_failed_at: failed_at,
                        attempt_count: 1,
                    })
                    .map_err(|_| PageApplyError::Hard)?;
                page_summary.decrypt_failed_count += 1;
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) {
                    page_summary.missing_key_quarantined_count += 1;
                } else {
                    page_summary.corruption_quarantined_count += 1;
                }
            }
            ApplyDisposition::UpgradeRequired(version) => {
                return Err(PageApplyError::UpgradeRequired(version));
            }
        }
        // Server presence is independent of decrypt/quarantine success.
        transaction
            .mark_full_resync_record(progress.generation_id, record.collection, record.record_id)
            .map_err(|_| PageApplyError::Hard)?;
    }
    finish(&mut transaction).map_err(|_| PageApplyError::Hard)?;
    transaction.commit().map_err(|_| PageApplyError::Hard)?;
    Ok(page_summary)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageApplyError {
    MissingKey,
    UpgradeRequired(u8),
    Hard,
}

fn page_apply_error_to_string(error: PageApplyError) -> String {
    match error {
        PageApplyError::UpgradeRequired(_) => "upgrade required".to_string(),
        PageApplyError::MissingKey | PageApplyError::Hard => "sync failed".to_string(),
    }
}

fn merge_summary(target: &mut SyncRunSummary, page: SyncRunSummary) {
    target.pulled_count += page.pulled_count;
    target.applied_count += page.applied_count;
    target.deleted_count += page.deleted_count;
    target.decrypt_failed_count += page.decrypt_failed_count;
    target.repush_count += page.repush_count;
    target.missing_key_quarantined_count += page.missing_key_quarantined_count;
    target.corruption_quarantined_count += page.corruption_quarantined_count;
    target.resolved_quarantine_count += page.resolved_quarantine_count;
}

fn apply_pull_page<S, N>(
    page: &crate::DeltaPage,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    quarantine_missing: bool,
) -> Result<SyncRunSummary, PageApplyError>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut transaction = store
        .begin_write_transaction()
        .map_err(|_| PageApplyError::Hard)?;
    let mut page_summary = SyncRunSummary {
        pulled_count: page.records.len(),
        ..SyncRunSummary::default()
    };
    for record in &page.records {
        let disposition =
            apply_pull_record(record, context, &mut transaction, now_ms, &mut page_summary)
                .map_err(|_| PageApplyError::Hard)?;
        match disposition {
            ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => {
                if transaction
                    .delete_quarantine(record.record_id)
                    .map_err(|_| PageApplyError::Hard)?
                {
                    page_summary.resolved_quarantine_count += 1;
                }
            }
            ApplyDisposition::Deferred(reason, required_list_id) => {
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) && !quarantine_missing
                {
                    return Err(PageApplyError::MissingKey);
                }
                let failed_at = now_ms().map_err(|_| PageApplyError::Hard)?;
                transaction
                    .put_quarantine(LocalSyncQuarantineEntry {
                        record_id: record.record_id,
                        collection: record.collection,
                        seq: record.seq,
                        revision_hlc: record.revision_hlc.clone(),
                        state: record.state.clone(),
                        reason,
                        required_list_id,
                        first_failed_at: failed_at,
                        last_failed_at: failed_at,
                        attempt_count: 1,
                    })
                    .map_err(|_| PageApplyError::Hard)?;
                page_summary.decrypt_failed_count += 1;
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) {
                    page_summary.missing_key_quarantined_count += 1;
                } else {
                    page_summary.corruption_quarantined_count += 1;
                }
            }
            ApplyDisposition::UpgradeRequired(version) => {
                return Err(PageApplyError::UpgradeRequired(version));
            }
        }
    }
    transaction
        .set_cursor(
            SYNC_CURSOR_NAME,
            page.next_since,
            now_ms().map_err(|_| PageApplyError::Hard)?,
        )
        .map_err(|_| PageApplyError::Hard)?;
    transaction.commit().map_err(|_| PageApplyError::Hard)?;
    Ok(page_summary)
}

fn replay_quarantine<S, N>(
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<(), String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut after = None;
    loop {
        let entries = store.list_replayable_quarantine(after, QUARANTINE_REPLAY_BATCH_LIMIT)?;
        if entries.is_empty() {
            break;
        }
        let page_len = entries.len();
        for entry in entries {
            after = Some((entry.seq, entry.record_id));
            let record = PullRecord {
                record_id: entry.record_id,
                collection: entry.collection,
                seq: entry.seq,
                revision_hlc: entry.revision_hlc,
                state: entry.state,
            };
            let mut transaction = store.begin_write_transaction()?;
            let mut replay_summary = SyncRunSummary::default();
            match apply_pull_record(
                &record,
                context,
                &mut transaction,
                now_ms,
                &mut replay_summary,
            )? {
                ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => {
                    transaction.delete_quarantine(record.record_id)?;
                    transaction.commit()?;
                    replay_summary.resolved_quarantine_count += 1;
                    merge_summary(summary, replay_summary);
                }
                ApplyDisposition::Deferred(reason, required_list_id) => {
                    let failed_at = now_ms()?;
                    transaction.put_quarantine(LocalSyncQuarantineEntry {
                        record_id: record.record_id,
                        collection: record.collection,
                        seq: record.seq,
                        revision_hlc: record.revision_hlc,
                        state: record.state,
                        reason,
                        required_list_id,
                        first_failed_at: failed_at,
                        last_failed_at: failed_at,
                        attempt_count: 1,
                    })?;
                    transaction.commit()?;
                }
                ApplyDisposition::UpgradeRequired(version) => {
                    return Err(format!("upgrade required:{version}"));
                }
            }
        }
        if page_len < QUARANTINE_REPLAY_BATCH_LIMIT {
            break;
        }
    }
    Ok(())
}

fn update_current_revision<S, N>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
    revision_hlc: &str,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let Some(mut state) = store.get_record_state(collection, record_id)? else {
        return Err("sync failed".to_string());
    };
    state.current_revision_hlc = Some(revision_hlc.to_string());
    store.put_record_state(collection, record_id, state, now_ms()?)
}

fn reconcile_nonaccepted_push_in_transaction<S, N>(
    current: &PullRecord,
    stale_op_id: Uuid,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    if !store.ack_outbox_op(stale_op_id)? {
        return Ok(());
    }
    match apply_pull_record(current, context, store, now_ms, summary)? {
        ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => Ok(()),
        ApplyDisposition::Deferred(_, _) | ApplyDisposition::UpgradeRequired(_) => {
            Err("sync failed".to_string())
        }
    }
}

fn apply_pull_record<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    match record.collection {
        SyncCollection::Lists => apply_pull_list(record, context, store, now_ms, summary),
        SyncCollection::Tasks => apply_pull_task(record, context, store, now_ms, summary),
        SyncCollection::Templates => apply_pull_template(record, context, store, now_ms, summary),
        SyncCollection::TaskSeries => {
            apply_pull_task_series(record, context, store, now_ms, summary)
        }
        SyncCollection::TimerSessions => {
            apply_pull_timer_session(record, context, store, now_ms, summary)
        }
    }
}

fn apply_pull_timer_session<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::TimerSessions, record.record_id)?;
    if let Some(LocalSyncRecordState {
        state: LocalSyncSemanticState::Tombstone { delete_hlc },
        ..
    }) = local_state.as_ref()
    {
        enqueue_rebased_tombstone(
            store,
            RebaseTombstoneRequest {
                record_id: record.record_id,
                collection: SyncCollection::TimerSessions,
                delete_hlc,
                device_id: &context.device_id,
                base_revision_hlc: Some(&record.revision_hlc),
            },
            now_ms,
        )?;
        return Ok(ApplyDisposition::Rebased);
    }

    match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::TimerSessions, record.record_id)?;
            store.delete_timer_session_for_sync(record.record_id)?;
            store.put_record_state(
                SyncCollection::TimerSessions,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            summary.deleted_count += 1;
            Ok(ApplyDisposition::AppliedCurrent)
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            let header = match crate::parse_envelope_header(blob) {
                Ok(header) => header,
                Err(error) => {
                    return Ok(classify_envelope_error(
                        error,
                        None,
                        blob.first().copied().unwrap_or(0),
                    ))
                }
            };
            let Some(dek) =
                crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
            else {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::MissingDek,
                    None,
                ));
            };
            let plaintext = decrypt_plaintext(
                dek,
                context.tenant_id,
                header.key_generation,
                crate::TIMER_SESSIONS_COLLECTION,
                record.record_id,
                blob,
            )
            .map_err(|error| {
                classify_envelope_error(error, None, blob.first().copied().unwrap_or(0))
            });
            let plaintext = match plaintext {
                Ok(value) => value,
                Err(disposition) => return Ok(disposition),
            };
            let SyncPlaintext::TimerSession(incoming) = &plaintext else {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::InvalidPlaintext,
                    None,
                ));
            };
            let task_state =
                store.get_record_state(SyncCollection::Tasks, incoming.value.task_id)?;
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = task_state
            {
                enqueue_rebased_tombstone(
                    store,
                    RebaseTombstoneRequest {
                        record_id: record.record_id,
                        collection: SyncCollection::TimerSessions,
                        delete_hlc: &delete_hlc,
                        device_id: &context.device_id,
                        base_revision_hlc: Some(&record.revision_hlc),
                    },
                    now_ms,
                )?;
                return Ok(ApplyDisposition::Rebased);
            }
            if store.get_task(incoming.value.task_id)?.is_none() {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::MissingDependency,
                    Some(incoming.value.task_id),
                ));
            }
            if let Some(existing) = store.get_timer_session(record.record_id)? {
                if existing != incoming.value {
                    return Ok(ApplyDisposition::Deferred(
                        PullFailureReason::InvalidPlaintext,
                        None,
                    ));
                }
            } else {
                store.upsert_timer_session_for_sync(incoming.value.clone())?;
            }
            store.delete_outbox_head(SyncCollection::TimerSessions, record.record_id)?;
            store.put_record_state(
                SyncCollection::TimerSessions,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Live {
                        mutation_hlc: mutation_hlc.clone(),
                        plaintext_json: serde_json::to_string(&plaintext)
                            .map_err(|_| "sync failed".to_string())?,
                    },
                },
                now_ms()?,
            )?;
            summary.applied_count += 1;
            if header.key_generation < context.keys.tenant_generation {
                enqueue_timer_session_sync(
                    store,
                    &context.keys,
                    &context.device_id,
                    &incoming.value,
                    false,
                    now_ms,
                )?;
                summary.repush_count += 1;
                Ok(ApplyDisposition::Rebased)
            } else {
                Ok(ApplyDisposition::AppliedCurrent)
            }
        }
    }
}

fn apply_pull_template<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::Templates, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Templates, record.record_id)?;
            if store.delete_template_for_sync(record.record_id)? {
                summary.deleted_count += 1;
            }
            store.put_record_state(
                SyncCollection::Templates,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Templates,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            None,
        ));
    };
    let incoming = match decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        TEMPLATES_COLLECTION,
        record.record_id,
        blob,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let existing = store.get_template(record.record_id)?;
    let stored_plaintext =
        stored_sync_plaintext(store, SyncCollection::Templates, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local), _) => {
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local = template_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let template = template_from_plaintext(record.record_id, &merged)?;
    store.upsert_template_for_sync(template)?;
    store_sync_plaintext(
        store,
        SyncCollection::Templates,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Templates,
                plaintext: &merged,
                dek: tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

fn apply_pull_task_series<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::TaskSeries, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::TaskSeries, record.record_id)?;
            if store.delete_series_for_sync(record.record_id)? {
                summary.deleted_count += 1;
            }
            store.put_record_state(
                SyncCollection::TaskSeries,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::TaskSeries,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            None,
        ));
    };
    let incoming = match decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        TASK_SERIES_COLLECTION,
        record.record_id,
        blob,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let existing = store.get_series(record.record_id)?;
    let stored_plaintext =
        stored_sync_plaintext(store, SyncCollection::TaskSeries, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local), _) => {
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local = task_series_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let series = task_series_from_plaintext(record.record_id, &merged)?;
    store.upsert_series_for_sync(series)?;
    store_sync_plaintext(
        store,
        SyncCollection::TaskSeries,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::TaskSeries,
                plaintext: &merged,
                dek: tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

fn apply_pull_list<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::Lists, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Lists, record.record_id)?;
            let default_list_id = store
                .default_list_id()?
                .filter(|default_id| *default_id != record.record_id)
                .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
            let mut known_tasks = store.list_tasks_by_list_for_sync(record.record_id)?;
            for task in &mut known_tasks {
                store.delete_outbox_head(SyncCollection::Tasks, task.id)?;
                task.list_id = default_list_id;
                task.updated_at = now_ms()?;
                store.upsert_task_for_sync(task.clone())?;
                enqueue_task_sync(
                    store,
                    &context.keys,
                    &context.device_id,
                    task,
                    false,
                    now_ms,
                )?;
                summary.repush_count += 1;
            }
            store.delete_list_and_rehome_tasks_for_sync(record.record_id)?;
            summary.deleted_count += 1;
            store.put_record_state(
                SyncCollection::Lists,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Lists,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                Some(record.record_id),
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            Some(record.record_id),
        ));
    };
    let incoming = decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        LISTS_COLLECTION,
        record.record_id,
        blob,
    );
    let incoming = match incoming {
        Ok(incoming) => incoming,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                Some(record.record_id),
                blob.first().copied().unwrap_or(0),
            ));
        }
    };
    let existing = store.get_list(record.record_id)?;
    let stored_plaintext = stored_sync_plaintext(store, SyncCollection::Lists, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local_plaintext), _) => {
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local_plaintext = list_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let mut list = list_from_plaintext(record.record_id, existing.as_ref(), &merged, now_ms)?;
    if list.is_default {
        if let Some(default_list_id) = store.default_list_id()? {
            if default_list_id != list.id {
                if default_list_id.as_bytes() < list.id.as_bytes() {
                    // Preserve the authenticated candidate identity in record
                    // state, while keeping the domain UNIQUE index valid until
                    // the closure-level canonical transaction runs.
                    list.is_default = false;
                } else {
                    let mut previous = store
                        .get_list(default_list_id)?
                        .ok_or_else(|| "sync failed".to_string())?;
                    previous.is_default = false;
                    store.upsert_list_for_sync(previous)?;
                }
            }
        }
    }
    store.upsert_list_for_sync(list)?;
    store_sync_plaintext(
        store,
        SyncCollection::Lists,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        let active_dek = tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?;
        let active_generation = context.keys.tenant_generation;
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Lists,
                plaintext: &merged,
                dek: active_dek,
                tenant_id: context.tenant_id,
                generation: active_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

fn apply_pull_task<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let existing = store.get_task(record.record_id)?;
    let local_state = store.get_record_state(SyncCollection::Tasks, record.record_id)?;
    let (incoming_mutation_hlc, _blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Tasks, record.record_id)?;
            let known_tasks = store.list_task_subtree_for_sync(record.record_id)?;
            summary.deleted_count += cascade_timer_sessions_for_tasks(
                store,
                &known_tasks,
                delete_hlc,
                &context.device_id,
                now_ms,
            )?;
            let deleted = store.delete_task_subtree_for_sync(record.record_id)?;
            summary.deleted_count += deleted;
            store.put_record_state(
                SyncCollection::Tasks,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Tasks,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let incoming_generation = match &record.state {
        EncryptedSyncState::Live { blob, .. } => {
            crate::parse_envelope_header(blob)
                .map_err(|_| "sync failed".to_string())?
                .key_generation
        }
        EncryptedSyncState::Tombstone { .. } => return Err("sync failed".to_string()),
    };
    let incoming = match decrypt_task_plaintext(record, existing.as_ref(), &context.keys) {
        Ok(incoming) => incoming,
        Err(disposition) => return Ok(disposition),
    };
    match &incoming {
        SyncPlaintext::Task(_) => {}
        SyncPlaintext::List(_)
        | SyncPlaintext::Template(_)
        | SyncPlaintext::TaskSeries(_)
        | SyncPlaintext::TimerSession(_) => return Err("sync failed".to_string()),
    }
    let dek = tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?;
    let stored_plaintext = stored_sync_plaintext(store, SyncCollection::Tasks, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local_plaintext), _) => {
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local_plaintext = task_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || incoming_generation < context.keys.tenant_generation;
    let mut task = task_from_plaintext(record.record_id, existing.as_ref(), &merged, now_ms)?;
    let authenticated_list_id = task.list_id;
    let resolved_list_id = store.resolve_list_alias(authenticated_list_id)?;
    let resolved_alias = resolved_list_id != authenticated_list_id;
    task.list_id = resolved_list_id;
    let dependency = task_dependency_disposition(store, &task)?;
    if matches!(dependency, TaskDependencyDisposition::Missing)
        && store.load_full_resync()?.is_some()
    {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDependency,
            Some(task.list_id),
        ));
    }
    if let TaskDependencyDisposition::Deleted = dependency {
        task.list_id = store
            .default_list_id()?
            .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
        task.updated_at = now_ms()?;
        store.upsert_task_for_sync(task.clone())?;
        store_sync_plaintext(
            store,
            SyncCollection::Tasks,
            record.record_id,
            &record.revision_hlc,
            incoming_mutation_hlc,
            &merged,
            now_ms,
        )?;
        enqueue_task_sync(
            store,
            &context.keys,
            &context.device_id,
            &task,
            false,
            now_ms,
        )?;
        summary.repush_count += 1;
        return Ok(ApplyDisposition::Rebased);
    }
    if matches!(dependency, TaskDependencyDisposition::Missing) {
        let delete_hlc = record.revision_hlc.clone();
        store.delete_outbox_head(SyncCollection::Tasks, record.record_id)?;
        let known_tasks = store.list_task_subtree_for_sync(record.record_id)?;
        summary.deleted_count += cascade_timer_sessions_for_tasks(
            store,
            &known_tasks,
            &delete_hlc,
            &context.device_id,
            now_ms,
        )?;
        let deleted = store.delete_task_subtree_for_sync(record.record_id)?;
        summary.deleted_count += deleted;
        enqueue_rebased_tombstone(
            store,
            RebaseTombstoneRequest {
                record_id: record.record_id,
                collection: SyncCollection::Tasks,
                delete_hlc: &delete_hlc,
                device_id: &context.device_id,
                base_revision_hlc: Some(&record.revision_hlc),
            },
            now_ms,
        )?;
        summary.repush_count += 1;
        return Ok(ApplyDisposition::Rebased);
    }
    store.upsert_task_for_sync(task.clone())?;
    store_sync_plaintext(
        store,
        SyncCollection::Tasks,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if resolved_alias {
        // Persist the authenticated remote merge first, then reuse the normal
        // mutation enqueue path to stamp only placement, reuse the Tenant
        // generation, and replace any stale outbox head transactionally.
        enqueue_task_sync(
            store,
            &context.keys,
            &context.device_id,
            &task,
            false,
            now_ms,
        )?;
        summary.repush_count += 1;
    } else if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Tasks,
                plaintext: &merged,
                dek,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush || resolved_alias {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

fn cascade_timer_sessions_for_tasks<S, N>(
    store: &mut S,
    tasks: &[Task],
    delete_hlc: &str,
    device_id: &str,
    now_ms: &mut N,
) -> Result<usize, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut deleted = 0;
    for task in tasks {
        for session in store.list_timer_sessions_by_task(task.id)? {
            store.delete_outbox_head(SyncCollection::TimerSessions, session.id)?;
            let base_revision = store
                .get_record_state(SyncCollection::TimerSessions, session.id)?
                .and_then(|state| state.current_revision_hlc);
            enqueue_rebased_tombstone(
                store,
                RebaseTombstoneRequest {
                    record_id: session.id,
                    collection: SyncCollection::TimerSessions,
                    delete_hlc,
                    device_id,
                    base_revision_hlc: base_revision.as_deref(),
                },
                now_ms,
            )?;
            deleted += usize::from(store.delete_timer_session_for_sync(session.id)?);
        }
        store.clear_active_timer_for_task(task.id)?;
    }
    Ok(deleted)
}

fn task_dependency_disposition<S>(
    store: &mut S,
    task: &Task,
) -> Result<TaskDependencyDisposition, String>
where
    S: LocalSyncStore,
{
    if let Some(LocalSyncRecordState {
        state: LocalSyncSemanticState::Tombstone { delete_hlc },
        ..
    }) = store.get_record_state(SyncCollection::Lists, task.list_id)?
    {
        let _ = delete_hlc;
        return Ok(TaskDependencyDisposition::Deleted);
    }
    if store.get_list(task.list_id)?.is_none() {
        return Ok(TaskDependencyDisposition::Missing);
    }

    let mut parent_id = task.parent_task_id;
    let mut visited = HashSet::new();
    while let Some(id) = parent_id {
        if !visited.insert(id) {
            return Ok(TaskDependencyDisposition::Missing);
        }
        if let Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Tombstone { delete_hlc },
            ..
        }) = store.get_record_state(SyncCollection::Tasks, id)?
        {
            let _ = delete_hlc;
            return Ok(TaskDependencyDisposition::Deleted);
        }
        let Some(parent) = store.get_task(id)? else {
            return Ok(TaskDependencyDisposition::Missing);
        };
        if parent.list_id != task.list_id {
            return Ok(TaskDependencyDisposition::Missing);
        }
        parent_id = parent.parent_task_id;
    }
    Ok(TaskDependencyDisposition::Valid)
}

fn stored_sync_plaintext<S>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
) -> Result<Option<SyncPlaintext>, String>
where
    S: LocalSyncStore,
{
    match store.get_record_state(collection, record_id)? {
        Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Live { plaintext_json, .. },
            ..
        }) => {
            let plaintext: SyncPlaintext =
                serde_json::from_str(&plaintext_json).map_err(|_| "sync failed".to_string())?;
            plaintext
                .validate_for_collection(collection.as_str(), &record_id.to_string())
                .map_err(|_| "sync failed".to_string())?;
            Ok(Some(plaintext))
        }
        Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Tombstone { .. },
            ..
        })
        | None => Ok(None),
    }
}

fn store_sync_plaintext<S, N>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
    current_revision_hlc: &str,
    incoming_mutation_hlc: &str,
    plaintext: &SyncPlaintext,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let plaintext_json = serde_json::to_string(plaintext).map_err(|_| "sync failed".to_string())?;
    let merged_mutation_hlc = plaintext
        .record_hlc()
        .encode()
        .map_err(|_| "sync failed".to_string())?;
    let mutation_hlc = if compare_encoded_hlc(&merged_mutation_hlc, incoming_mutation_hlc)?
        == std::cmp::Ordering::Less
    {
        incoming_mutation_hlc.to_string()
    } else {
        merged_mutation_hlc
    };
    store.put_record_state(
        collection,
        record_id,
        LocalSyncRecordState {
            current_revision_hlc: Some(current_revision_hlc.to_string()),
            state: LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
        },
        now_ms()?,
    )
}

fn compare_encoded_hlc(left: &str, right: &str) -> Result<std::cmp::Ordering, String> {
    let left = Hlc::decode(left).map_err(|_| "sync failed".to_string())?;
    let right = Hlc::decode(right).map_err(|_| "sync failed".to_string())?;
    Ok(left.cmp(&right))
}

fn decrypt_task_plaintext(
    record: &PullRecord,
    existing: Option<&Task>,
    keys: &LocalSyncKeys,
) -> Result<SyncPlaintext, ApplyDisposition> {
    let EncryptedSyncState::Live { blob, .. } = &record.state else {
        return Err(ApplyDisposition::Deferred(
            PullFailureReason::CorruptEnvelope,
            None,
        ));
    };
    let header = crate::parse_envelope_header(blob)
        .map_err(|error| classify_envelope_error(error, None, 0))?;
    let expected_list_id = existing.map(|task| task.list_id);
    let Some(tenant_key) = crate::tenant_root_dek_for_generation(keys, header.key_generation)
    else {
        return Err(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            expected_list_id,
        ));
    };
    decrypt_plaintext(
        tenant_key,
        keys.tenant_id,
        header.key_generation,
        TASKS_COLLECTION,
        record.record_id,
        blob,
    )
    .map_err(|error| {
        classify_envelope_error(error, expected_list_id, blob.first().copied().unwrap_or(0))
    })
}

fn classify_envelope_error(
    error: EnvelopeError,
    required_list_id: Option<Uuid>,
    envelope_version: u8,
) -> ApplyDisposition {
    match error {
        EnvelopeError::UnsupportedVersion => ApplyDisposition::UpgradeRequired(envelope_version),
        EnvelopeError::Crypto(_) => {
            ApplyDisposition::Deferred(PullFailureReason::AuthenticationFailed, required_list_id)
        }
        EnvelopeError::Deserialization | EnvelopeError::Serialization => {
            ApplyDisposition::Deferred(PullFailureReason::InvalidPlaintext, required_list_id)
        }
        EnvelopeError::BlobTooShort
        | EnvelopeError::BlobTooLarge
        | EnvelopeError::UnsupportedSuite
        | EnvelopeError::InvalidGeneration
        | EnvelopeError::InvalidIdentity
        | EnvelopeError::CollectionTooLong
        | EnvelopeError::KeyDerivation => {
            ApplyDisposition::Deferred(PullFailureReason::CorruptEnvelope, required_list_id)
        }
    }
}

fn upgrade_block_value(protocol_version: u16, envelope_version: u8) -> String {
    format!("{protocol_version}:{envelope_version}")
}

fn upgrade_block_is_active(value: &str) -> bool {
    let Some((protocol, envelope)) = value.split_once(':') else {
        return true;
    };
    let (Ok(protocol), Ok(envelope)) = (protocol.parse::<u16>(), envelope.parse::<u8>()) else {
        return true;
    };
    crate::protocol::SYNC_PROTOCOL_VERSION < protocol || crate::ENVELOPE_VERSION < envelope
}

fn replay_upgrade_version(error: &str) -> Option<u8> {
    error
        .strip_prefix("upgrade required:")
        .and_then(|value| value.parse().ok())
}

fn record_hlc_or_initial(plaintext: &SyncPlaintext) -> Hlc {
    plaintext.record_hlc().clone()
}

fn task_from_plaintext<N>(
    id: Uuid,
    _existing: Option<&Task>,
    plaintext: &SyncPlaintext,
    _now_ms: &mut N,
) -> Result<Task, String>
where
    N: FnMut() -> Result<i64, String>,
{
    plaintext
        .validate_for_collection(TASKS_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::Task(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(Task {
        id,
        list_id: fields.placement.value.list_id,
        parent_task_id: fields.placement.value.parent_task_id,
        content: TaskContent {
            title: fields.title.value.clone(),
            note: fields.note.value.clone(),
            priority: fields.priority.value,
            estimated_minutes: fields.estimated_minutes.value,
        },
        status: fields.completion.value.status,
        due: fields.due.value.clone(),
        scheduled_at: fields.scheduled_at.value,
        sort_order: fields.placement.value.rank.clone(),
        completed_at: fields.completion.value.completed_at,
        closed_reason: fields.completion.value.closed_reason.clone(),
        deleted_at: None,
        assignee: fields.assignee.value,
        series_occurrence: fields.series_occurrence.value.clone(),
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}

fn template_from_plaintext(id: Uuid, plaintext: &SyncPlaintext) -> Result<TaskTemplate, String> {
    plaintext
        .validate_for_collection(TEMPLATES_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::Template(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(TaskTemplate {
        id,
        name: fields.name.value.clone(),
        default_list_id: fields.default_list_id.value,
        blueprint: fields.blueprint.value.blueprint.clone(),
        blueprint_revision: fields.blueprint.value.revision.clone(),
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}

fn task_series_from_plaintext(id: Uuid, plaintext: &SyncPlaintext) -> Result<TaskSeries, String> {
    plaintext
        .validate_for_collection(TASK_SERIES_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::TaskSeries(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(TaskSeries {
        id,
        config: TaskSeriesConfig {
            blueprint: fields.config.value.blueprint.clone(),
            target_list_id: fields.config.value.target_list_id,
            rrule: fields.config.value.rrule.clone(),
            starts_at: fields.config.value.starts_at,
            time_zone: fields.config.value.time_zone.clone(),
            enabled: fields.config.value.enabled,
            config_revision: fields.config.value.revision.clone(),
            config_parent_revision: fields.config.value.parent_revision.clone(),
            config_effective_from: fields.config.value.effective_from,
            lineage: fields.config.value.lineage.clone(),
        },
        cursor: fields.cursor.value.cursor,
        created_at: fields.config.value.created_at,
        updated_at: fields.config.value.updated_at,
    })
}

fn list_from_plaintext<N>(
    id: Uuid,
    _existing: Option<&List>,
    plaintext: &SyncPlaintext,
    _now_ms: &mut N,
) -> Result<List, String>
where
    N: FnMut() -> Result<i64, String>,
{
    plaintext
        .validate_for_collection(LISTS_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::List(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(List {
        id,
        name: fields.name.value.clone(),
        color: fields.color.value.clone(),
        icon: fields.icon.value.clone(),
        sort_order: fields.placement.value.rank.clone(),
        is_default: fields.is_default.value,
        archived_at: fields.archived_at.value,
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use taskveil_crypto::key_hierarchy::KEY_LEN;
    use taskveil_domain::{
        new_task, CompletedTimerSession, SeriesCursor, TaskBlueprint, TaskBlueprintNode,
        TaskContent, TaskSeriesConfig, TimerFinishKind, TimerMode, TASK_BLUEPRINT_SCHEMA_REVISION,
    };
    use zeroize::Zeroizing;

    use super::*;
    use crate::{
        LocalMutationSyncStore, LocalSyncOutboxEntry, NewLocalSyncOutboxEntry,
        TimerSessionPlaintext, TIMER_SESSIONS_COLLECTION,
    };

    fn test_tenant_id() -> Uuid {
        Uuid::from_u128(100)
    }

    fn encrypt_plaintext(
        dek: &[u8; 32],
        collection: &str,
        record_id: &str,
        plaintext: &SyncPlaintext,
    ) -> Result<Vec<u8>, EnvelopeError> {
        crate::envelope::encrypt_plaintext(
            dek,
            test_tenant_id(),
            1,
            collection,
            Uuid::parse_str(record_id).map_err(|_| EnvelopeError::InvalidIdentity)?,
            plaintext,
        )
    }

    fn decrypt_plaintext(
        dek: &[u8; 32],
        collection: &str,
        record_id: &str,
        blob: &[u8],
    ) -> Result<SyncPlaintext, EnvelopeError> {
        crate::envelope::decrypt_plaintext(
            dek,
            test_tenant_id(),
            1,
            collection,
            Uuid::parse_str(record_id).map_err(|_| EnvelopeError::InvalidIdentity)?,
            blob,
        )
    }

    #[derive(Default)]
    struct FakeStore {
        lists: HashMap<Uuid, List>,
        tasks: HashMap<Uuid, Task>,
        templates: HashMap<Uuid, TaskTemplate>,
        series: HashMap<Uuid, TaskSeries>,
        timer_sessions: HashMap<Uuid, CompletedTimerSession>,
        active_timer_task: Option<Uuid>,
        record_states: HashMap<(SyncCollection, Uuid), LocalSyncRecordState>,
        outbox: Vec<LocalSyncOutboxEntry>,
        aliases: Vec<LocalListAlias>,
        live_list_quarantine: bool,
        settings: HashMap<String, String>,
    }

    impl LocalMutationSyncStore for FakeStore {
        fn has_outbox_head(
            &mut self,
            collection: SyncCollection,
            record_id: Uuid,
        ) -> Result<bool, String> {
            Ok(self
                .outbox
                .iter()
                .any(|entry| entry.collection == collection && entry.record_id == record_id))
        }

        fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
            Ok(self.settings.get(key).cloned())
        }

        fn set_setting(&mut self, key: &str, value: &str, _updated_at: i64) -> Result<(), String> {
            self.settings.insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
            self.outbox.retain(|head| head.record_id != entry.record_id);
            self.outbox.push(LocalSyncOutboxEntry {
                op_id: entry.op_id,
                record_id: entry.record_id,
                collection: entry.collection,
                base_revision_hlc: entry.base_revision_hlc,
                revision_hlc: entry.revision_hlc,
                state: entry.state,
                created_at: entry.created_at,
            });
            Ok(())
        }

        fn get_record_state(
            &mut self,
            collection: SyncCollection,
            record_id: Uuid,
        ) -> Result<Option<LocalSyncRecordState>, String> {
            Ok(self.record_states.get(&(collection, record_id)).cloned())
        }

        fn put_record_state(
            &mut self,
            collection: SyncCollection,
            record_id: Uuid,
            state: LocalSyncRecordState,
            _updated_at: i64,
        ) -> Result<(), String> {
            self.record_states.insert((collection, record_id), state);
            Ok(())
        }
    }

    impl LocalSyncStore for FakeStore {
        fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
            Ok(self.outbox.iter().take(limit).cloned().collect())
        }

        fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
            let previous_len = self.outbox.len();
            self.outbox.retain(|entry| entry.op_id != op_id);
            Ok(previous_len != self.outbox.len())
        }

        fn delete_outbox_head(
            &mut self,
            collection: SyncCollection,
            record_id: Uuid,
        ) -> Result<bool, String> {
            let previous_len = self.outbox.len();
            self.outbox
                .retain(|entry| entry.collection != collection || entry.record_id != record_id);
            Ok(previous_len != self.outbox.len())
        }

        fn get_cursor_seq(&mut self, _name: &str) -> Result<Option<i64>, String> {
            Ok(None)
        }

        fn set_cursor(&mut self, _name: &str, _seq: i64, _updated_at: i64) -> Result<(), String> {
            Ok(())
        }

        fn delete_cursor(&mut self, _name: &str) -> Result<(), String> {
            Ok(())
        }

        fn list_record_states(
            &mut self,
            collection: SyncCollection,
        ) -> Result<Vec<(Uuid, LocalSyncRecordState)>, String> {
            Ok(self
                .record_states
                .iter()
                .filter(|((stored_collection, _), _)| *stored_collection == collection)
                .map(|((_, record_id), state)| (*record_id, state.clone()))
                .collect())
        }

        fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
            Ok(collection == SyncCollection::Lists && self.live_list_quarantine)
        }

        fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
            Ok(self.aliases.clone())
        }

        fn replace_list_aliases(
            &mut self,
            aliases: &[LocalListAlias],
            _updated_at: i64,
        ) -> Result<(), String> {
            self.aliases = aliases.to_vec();
            Ok(())
        }

        fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String> {
            Ok(self
                .aliases
                .iter()
                .find(|alias| alias.alias_list_id == list_id)
                .map_or(list_id, |alias| alias.canonical_list_id))
        }

        fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
            if !self.lists.contains_key(&canonical_list_id) {
                return Err("canonical list is missing".to_string());
            }
            for list in self.lists.values_mut() {
                list.is_default = list.id == canonical_list_id;
            }
            Ok(())
        }

        fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
            Ok(self
                .lists
                .values()
                .find(|list| list.is_default)
                .map(|list| list.id))
        }

        fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
            Ok(self.lists.get(&id).cloned())
        }

        fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
            if list.is_default
                && self
                    .lists
                    .values()
                    .any(|existing| existing.is_default && existing.id != list.id)
            {
                return Err("default list conflict".to_string());
            }
            self.lists.insert(list.id, list);
            Ok(())
        }

        fn delete_list_and_rehome_tasks_for_sync(
            &mut self,
            list_id: Uuid,
        ) -> Result<usize, String> {
            let default_list_id = self
                .default_list_id()?
                .filter(|default_id| *default_id != list_id)
                .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
            self.lists.remove(&list_id);
            let mut rehomed = 0;
            for task in self.tasks.values_mut() {
                if task.list_id == list_id {
                    task.list_id = default_list_id;
                    rehomed += 1;
                }
            }
            Ok(rehomed)
        }

        fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
            Ok(self.tasks.get(&id).cloned())
        }

        fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String> {
            Ok(self
                .tasks
                .values()
                .filter(|task| task.list_id == list_id)
                .cloned()
                .collect())
        }

        fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
            Ok(self.tasks.values().cloned().collect())
        }

        fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
            self.tasks.insert(task.id, task);
            Ok(())
        }

        fn delete_task_subtree_for_sync(&mut self, _task_id: Uuid) -> Result<usize, String> {
            Ok(usize::from(self.tasks.remove(&_task_id).is_some()))
        }

        fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
            Ok(self.templates.get(&id).cloned())
        }

        fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
            self.templates.insert(template.id, template);
            Ok(())
        }

        fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
            Ok(self.templates.remove(&id).is_some())
        }

        fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
            Ok(self.series.get(&id).cloned())
        }

        fn upsert_series_for_sync(&mut self, series: TaskSeries) -> Result<(), String> {
            self.series.insert(series.id, series);
            Ok(())
        }

        fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
            Ok(self.series.remove(&id).is_some())
        }

        fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
            Ok(self.timer_sessions.get(&id).cloned())
        }

        fn upsert_timer_session_for_sync(
            &mut self,
            session: CompletedTimerSession,
        ) -> Result<(), String> {
            self.timer_sessions.insert(session.id, session);
            Ok(())
        }

        fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
            Ok(self.timer_sessions.remove(&id).is_some())
        }

        fn list_timer_sessions_by_task(
            &mut self,
            task_id: Uuid,
        ) -> Result<Vec<CompletedTimerSession>, String> {
            Ok(self
                .timer_sessions
                .values()
                .filter(|session| session.task_id == task_id)
                .cloned()
                .collect())
        }

        fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
            let matched = self.active_timer_task == Some(task_id);
            if matched {
                self.active_timer_task = None;
            }
            Ok(matched)
        }
    }

    #[test]
    fn remote_task_tombstone_cascades_timer_with_same_delete_hlc_and_clears_active() {
        let list_id = uuid(90);
        let task_id = uuid(91);
        let session_id = uuid(92);
        let task = new_task(
            list_id,
            None,
            "timed".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1_799_000_000_000,
        )
        .unwrap();
        let mut task = task;
        task.id = task_id;
        let session = CompletedTimerSession {
            id: session_id,
            task_id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: 1_799_000_000_000,
            ended_at: 1_799_000_060_000,
            active_duration_ms: 60_000,
            created_at: 1_799_000_060_000,
        };
        let delete_hlc = Hlc {
            wall_ms: 1_799_000_070_000,
            counter: 0,
            device_id: "remote".to_string(),
        }
        .encode()
        .unwrap();
        let record = PullRecord {
            seq: 1,
            record_id: task_id,
            collection: SyncCollection::Tasks,
            revision_hlc: delete_hlc.clone(),
            state: EncryptedSyncState::Tombstone {
                delete_hlc: delete_hlc.clone(),
            },
        };
        let mut store = FakeStore::default();
        store.lists.insert(list_id, sample_list(list_id, false));
        store.tasks.insert(task_id, task);
        store.timer_sessions.insert(session_id, session);
        store.active_timer_task = Some(task_id);
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_task(
            &record,
            &context_for(list_id, [9; KEY_LEN]),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert!(!store.timer_sessions.contains_key(&session_id));
        assert_eq!(store.active_timer_task, None);
        let timer_tombstone = store
            .outbox
            .iter()
            .find(|entry| entry.collection == SyncCollection::TimerSessions)
            .unwrap();
        assert_eq!(timer_tombstone.record_id, session_id);
        assert_eq!(
            timer_tombstone.state,
            EncryptedSyncState::Tombstone { delete_hlc }
        );
    }

    #[test]
    fn late_timer_uses_terminal_task_delete_hlc() {
        let list_id = uuid(93);
        let task_id = uuid(94);
        let session_id = uuid(95);
        let root = [0x95; KEY_LEN];
        let session = CompletedTimerSession {
            id: session_id,
            task_id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: 1_799_000_000_000,
            ended_at: 1_799_000_030_000,
            active_duration_ms: 30_000,
            created_at: 1_799_000_030_000,
        };
        let session_hlc = Hlc {
            wall_ms: 1_799_000_050_000,
            counter: 0,
            device_id: "remote".into(),
        };
        let task_delete_hlc = Hlc {
            wall_ms: 1_799_000_040_000,
            counter: 0,
            device_id: "deleting-device".into(),
        }
        .encode()
        .unwrap();
        let plaintext = SyncPlaintext::TimerSession(TimerSessionPlaintext {
            value: session,
            hlc: session_hlc.clone(),
        });
        let record = PullRecord {
            record_id: session_id,
            collection: SyncCollection::TimerSessions,
            seq: 1,
            revision_hlc: session_hlc.encode().unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: session_hlc.encode().unwrap(),
                blob: encrypt_plaintext(
                    &root,
                    TIMER_SESSIONS_COLLECTION,
                    &session_id.to_string(),
                    &plaintext,
                )
                .unwrap(),
            },
        };
        let mut context = context_for(list_id, [0x93; KEY_LEN]);
        context.keys.tenant_root_dek = Some(Zeroizing::new(root));
        let mut store = FakeStore::default();
        store.record_states.insert(
            (SyncCollection::Tasks, task_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(task_delete_hlc.clone()),
                state: LocalSyncSemanticState::Tombstone {
                    delete_hlc: task_delete_hlc.clone(),
                },
            },
        );
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        assert_eq!(
            apply_pull_timer_session(&record, &context, &mut store, &mut now, &mut summary)
                .unwrap(),
            ApplyDisposition::Rebased
        );
        let tombstone = store.outbox.first().unwrap();
        assert_eq!(
            tombstone.state,
            EncryptedSyncState::Tombstone {
                delete_hlc: task_delete_hlc
            }
        );
    }

    #[test]
    fn pull_default_list_with_existing_different_default_demotes_local_row_only() {
        let local_default = sample_list(uuid(1), true);
        let incoming_list = sample_list(uuid(2), true);
        let dek = [0x7a; KEY_LEN];
        let record = encrypted_list_record(&incoming_list, &dek);
        let context = context_for(incoming_list.id, dek);
        let mut store = FakeStore::default();
        store.lists.insert(local_default.id, local_default);
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

        assert!(!store.lists.get(&incoming_list.id).unwrap().is_default);
        let stored_plaintext =
            stored_sync_plaintext(&mut store, SyncCollection::Lists, incoming_list.id).unwrap();
        let SyncPlaintext::List(stored) = stored_plaintext.unwrap() else {
            panic!("list");
        };
        assert!(stored.is_default.value);
        assert_eq!(store.outbox.len(), 0);
        assert_eq!(summary.applied_count, 1);
        assert_eq!(summary.repush_count, 0);
    }

    #[test]
    fn pull_default_list_without_existing_default_keeps_default_flag() {
        let incoming_list = sample_list(uuid(3), true);
        let dek = [0x3b; KEY_LEN];
        let record = encrypted_list_record(&incoming_list, &dek);
        let context = context_for(incoming_list.id, dek);
        let mut store = FakeStore::default();
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

        assert!(store.lists.get(&incoming_list.id).unwrap().is_default);
        assert_eq!(summary.applied_count, 1);
        assert_eq!(summary.repush_count, 0);
    }

    #[test]
    fn pull_smaller_default_candidate_materializes_deterministically() {
        let existing = sample_list(uuid(20), true);
        let incoming = sample_list(uuid(10), true);
        let dek = [0x3c; KEY_LEN];
        let record = encrypted_list_record(&incoming, &dek);
        let context = context_for(incoming.id, dek);
        let mut store = FakeStore::default();
        store.lists.insert(existing.id, existing.clone());
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

        assert!(store.lists.get(&incoming.id).unwrap().is_default);
        assert!(!store.lists.get(&existing.id).unwrap().is_default);
        let stored = stored_sync_plaintext(&mut store, SyncCollection::Lists, incoming.id)
            .unwrap()
            .unwrap();
        let SyncPlaintext::List(stored) = stored else {
            panic!("list");
        };
        assert!(stored.is_default.value);
    }

    #[test]
    fn canonical_reconcile_moves_tasks_reencrypts_and_is_idempotent() {
        let canonical = sample_list(uuid(1), true);
        let loser = sample_list(uuid(2), true);
        let mut local_canonical = canonical.clone();
        local_canonical.is_default = false;
        let canonical_dek = [0x11; KEY_LEN];
        let loser_dek = [0x22; KEY_LEN];
        let mut task = new_task(
            loser.id,
            None,
            "alias task".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1_799_000_000_000,
        )
        .unwrap();
        task.id = uuid(30);
        let mut store = FakeStore::default();
        store.lists.insert(canonical.id, local_canonical);
        store.lists.insert(loser.id, loser.clone());
        store.tasks.insert(task.id, task.clone());
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Lists,
            canonical.id,
            list_plaintext(&canonical, test_hlc(1, "canonical")),
        );
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Lists,
            loser.id,
            list_plaintext(&loser, test_hlc(1, "loser")),
        );
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Tasks,
            task.id,
            task_plaintext(&task, test_hlc(1, "task")),
        );
        let context = context_with_keys(&[(canonical.id, canonical_dek), (loser.id, loser_dek)]);
        let mut now = ticking_now();

        reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

        assert!(store.lists.get(&canonical.id).unwrap().is_default);
        assert!(!store.lists.get(&loser.id).unwrap().is_default);
        assert_eq!(store.tasks.get(&task.id).unwrap().list_id, canonical.id);
        assert_eq!(
            store.aliases,
            vec![LocalListAlias {
                alias_list_id: loser.id,
                canonical_list_id: canonical.id,
            }]
        );
        assert_eq!(store.outbox.len(), 1);
        let encrypted = &store.outbox[0].state;
        let EncryptedSyncState::Live { blob, .. } = encrypted else {
            panic!("live");
        };
        let plaintext =
            decrypt_plaintext(&canonical_dek, TASKS_COLLECTION, &task.id.to_string(), blob)
                .unwrap();
        let SyncPlaintext::Task(plaintext) = plaintext else {
            panic!("task");
        };
        assert_eq!(plaintext.placement.value.list_id, canonical.id);
        assert!(
            decrypt_plaintext(&loser_dek, TASKS_COLLECTION, &task.id.to_string(), blob).is_err()
        );
        let op_id = store.outbox[0].op_id;

        reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].op_id, op_id);
    }

    #[test]
    fn later_smaller_candidate_flattens_existing_aliases() {
        let first = sample_list(uuid(20), true);
        let old_loser = sample_list(uuid(30), true);
        let later = sample_list(uuid(10), true);
        let mut store = FakeStore::default();
        store.lists.insert(first.id, first.clone());
        let mut old_loser_domain = old_loser.clone();
        old_loser_domain.is_default = false;
        store.lists.insert(old_loser.id, old_loser_domain);
        let mut later_domain = later.clone();
        later_domain.is_default = false;
        store.lists.insert(later.id, later_domain);
        for list in [&first, &old_loser, &later] {
            put_live_plaintext_state(
                &mut store,
                SyncCollection::Lists,
                list.id,
                list_plaintext(list, test_hlc(1, "remote")),
            );
        }
        store.aliases = vec![LocalListAlias {
            alias_list_id: old_loser.id,
            canonical_list_id: first.id,
        }];
        let context = context_with_keys(&[
            (first.id, [0x20; KEY_LEN]),
            (old_loser.id, [0x30; KEY_LEN]),
            (later.id, [0x10; KEY_LEN]),
        ]);
        let mut now = ticking_now();

        reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

        assert!(store.lists.get(&later.id).unwrap().is_default);
        assert_eq!(
            store.aliases,
            vec![
                LocalListAlias {
                    alias_list_id: first.id,
                    canonical_list_id: later.id,
                },
                LocalListAlias {
                    alias_list_id: old_loser.id,
                    canonical_list_id: later.id,
                },
            ]
        );
    }

    #[test]
    fn live_list_quarantine_defers_election_without_writes() {
        let first = sample_list(uuid(1), true);
        let second = sample_list(uuid(2), true);
        let mut store = FakeStore::default();
        store.lists.insert(first.id, first.clone());
        let mut second_domain = second.clone();
        second_domain.is_default = false;
        store.lists.insert(second.id, second_domain);
        for list in [&first, &second] {
            put_live_plaintext_state(
                &mut store,
                SyncCollection::Lists,
                list.id,
                list_plaintext(list, test_hlc(1, "remote")),
            );
        }
        store.live_list_quarantine = true;
        let context =
            context_with_keys(&[(first.id, [0x11; KEY_LEN]), (second.id, [0x22; KEY_LEN])]);
        let mut now = ticking_now();

        reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

        assert!(store.aliases.is_empty());
        assert!(store.outbox.is_empty());
        assert!(store.lists.get(&first.id).unwrap().is_default);
    }

    #[test]
    fn pulled_old_generation_live_head_uses_history_key_and_repushed_active_generation() {
        let list = sample_list(uuid(77), false);
        let old_dek = [0x31; KEY_LEN];
        let active_dek = [0x32; KEY_LEN];
        let record = encrypted_list_record(&list, &old_dek);
        let mut context = context_with_keys(&[(list.id, active_dek)]);
        context.keys.tenant_generation = 2;
        context
            .keys
            .historical_tenant_root_deks
            .push((1, Zeroizing::new(old_dek)));
        let mut store = FakeStore::default();
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        assert_eq!(
            apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
            ApplyDisposition::Rebased
        );
        let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
            panic!("live")
        };
        assert_eq!(
            crate::parse_envelope_header(blob).unwrap().key_generation,
            2
        );
        assert!(crate::envelope::decrypt_plaintext(
            &active_dek,
            test_tenant_id(),
            2,
            LISTS_COLLECTION,
            list.id,
            blob,
        )
        .is_ok());
    }

    #[test]
    fn missing_tenant_key_fails_before_canonical_materialization() {
        let canonical = sample_list(uuid(1), true);
        let loser = sample_list(uuid(2), true);
        let mut canonical_domain = canonical.clone();
        canonical_domain.is_default = false;
        let mut store = FakeStore::default();
        store.lists.insert(canonical.id, canonical_domain);
        store.lists.insert(loser.id, loser.clone());
        for list in [&canonical, &loser] {
            put_live_plaintext_state(
                &mut store,
                SyncCollection::Lists,
                list.id,
                list_plaintext(list, test_hlc(1, "remote")),
            );
        }
        let mut context = context_with_keys(&[(loser.id, [0x22; KEY_LEN])]);
        context.keys.tenant_root_dek = None;
        let mut now = ticking_now();

        assert!(reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).is_err());
        assert!(!store.lists.get(&canonical.id).unwrap().is_default);
        assert!(store.lists.get(&loser.id).unwrap().is_default);
        assert!(store.aliases.is_empty());
        assert!(store.outbox.is_empty());
    }

    #[test]
    fn pulled_task_for_durable_alias_is_rehomed_and_reencrypted() {
        let canonical = sample_list(uuid(1), true);
        let mut alias = sample_list(uuid(2), true);
        alias.is_default = false;
        let tenant_dek = [0x41; KEY_LEN];
        let mut task = new_task(
            alias.id,
            None,
            "late".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1_799_000_000_000,
        )
        .unwrap();
        task.id = uuid(40);
        let task_hlc = test_hlc(1, "remote");
        let task_plaintext = task_plaintext(&task, task_hlc.clone());
        let record = encrypted_task_record(task.id, &task_plaintext, &tenant_dek, &task_hlc);
        let context = context_with_keys(&[(canonical.id, tenant_dek)]);
        let mut store = FakeStore::default();
        store.lists.insert(canonical.id, canonical.clone());
        store.lists.insert(alias.id, alias.clone());
        store.aliases = vec![LocalListAlias {
            alias_list_id: alias.id,
            canonical_list_id: canonical.id,
        }];
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        assert_eq!(
            apply_pull_task(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
            ApplyDisposition::Rebased
        );
        assert_eq!(store.tasks.get(&task.id).unwrap().list_id, canonical.id);
        let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
            panic!("live");
        };
        let plaintext =
            decrypt_plaintext(&tenant_dek, TASKS_COLLECTION, &task.id.to_string(), blob).unwrap();
        let SyncPlaintext::Task(plaintext) = plaintext else {
            panic!("task");
        };
        assert_eq!(plaintext.placement.value.list_id, canonical.id);
        assert_eq!(summary.repush_count, 1);
    }

    #[test]
    fn remote_tombstone_discards_newer_local_live_and_outbox() {
        let list_id = uuid(30);
        let local = sample_list(list_id, false);
        let default_list = sample_list(uuid(29), true);
        let local_hlc = Hlc {
            wall_ms: 1_799_000_000_500,
            counter: 0,
            device_id: "local".to_string(),
        }
        .encode()
        .unwrap();
        let delete_hlc = Hlc {
            wall_ms: 1_799_000_000_100,
            counter: 0,
            device_id: "remote".to_string(),
        }
        .encode()
        .unwrap();
        let revision_hlc = Hlc {
            wall_ms: 1_799_000_000_600,
            counter: 0,
            device_id: "remote".to_string(),
        }
        .encode()
        .unwrap();
        let mut store = FakeStore::default();
        store.lists.insert(default_list.id, default_list);
        store.lists.insert(list_id, local.clone());
        store.record_states.insert(
            (SyncCollection::Lists, list_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(local_hlc.clone()),
                state: LocalSyncSemanticState::Live {
                    mutation_hlc: local_hlc.clone(),
                    plaintext_json: serde_json::to_string(&list_plaintext(
                        &local,
                        Hlc::decode(&local_hlc).unwrap(),
                    ))
                    .unwrap(),
                },
            },
        );
        store.outbox.push(LocalSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id: list_id,
            collection: SyncCollection::Lists,
            base_revision_hlc: Some(local_hlc.clone()),
            revision_hlc: local_hlc,
            state: EncryptedSyncState::Live {
                mutation_hlc: Hlc::decode(&revision_hlc).unwrap().encode().unwrap(),
                blob: vec![1],
            },
            created_at: 1,
        });
        let record = PullRecord {
            record_id: list_id,
            collection: SyncCollection::Lists,
            seq: 2,
            revision_hlc: revision_hlc.clone(),
            state: EncryptedSyncState::Tombstone {
                delete_hlc: delete_hlc.clone(),
            },
        };
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(
            &record,
            &context_for(list_id, [3; KEY_LEN]),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert!(!store.lists.contains_key(&list_id));
        assert!(store.outbox.is_empty());
        assert_eq!(
            store.record_states[&(SyncCollection::Lists, list_id)],
            LocalSyncRecordState {
                current_revision_hlc: Some(revision_hlc),
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
            }
        );
        assert_eq!(summary.repush_count, 0);
    }

    #[test]
    fn remote_series_tombstone_removes_only_series_and_keeps_generated_task() {
        let series = sample_series(uuid(31));
        let list = sample_list(uuid(32), true);
        let mut generated = new_task(
            list.id,
            None,
            "Generated".into(),
            "7fffffffffffffffffffffffffffffff".into(),
            1,
        )
        .unwrap();
        generated.series_occurrence = Some(taskveil_domain::SeriesOccurrenceRef {
            series_id: series.id,
            series_revision: series.config.config_revision.clone(),
            occurrence_at: series.config.starts_at,
            blueprint_node_key: "root".into(),
        });
        let delete_hlc = test_hlc(1, "remote").encode().unwrap();
        let revision_hlc = test_hlc(2, "remote").encode().unwrap();
        let record = PullRecord {
            record_id: series.id,
            collection: SyncCollection::TaskSeries,
            seq: 2,
            revision_hlc: revision_hlc.clone(),
            state: EncryptedSyncState::Tombstone {
                delete_hlc: delete_hlc.clone(),
            },
        };
        let mut store = FakeStore::default();
        store.lists.insert(list.id, list);
        store.series.insert(series.id, series.clone());
        store.tasks.insert(generated.id, generated.clone());
        store.outbox.push(LocalSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id: series.id,
            collection: SyncCollection::TaskSeries,
            base_revision_hlc: None,
            revision_hlc: test_hlc(1, "local").encode().unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: test_hlc(1, "local").encode().unwrap(),
                blob: vec![1],
            },
            created_at: 1,
        });
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_task_series(
            &record,
            &context_with_keys(&[]),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert!(!store.series.contains_key(&series.id));
        assert_eq!(store.tasks.get(&generated.id), Some(&generated));
        assert!(store.outbox.is_empty());
        assert_eq!(
            store.record_states[&(SyncCollection::TaskSeries, series.id)],
            LocalSyncRecordState {
                current_revision_hlc: Some(revision_hlc),
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
            }
        );
        assert_eq!(summary.deleted_count, 1);
    }

    #[test]
    fn remote_template_tombstone_keeps_independent_series_and_task() {
        let template = sample_template(uuid(41));
        let mut series = sample_series(uuid(42));
        series.config.blueprint = template.blueprint.clone();
        let list = sample_list(uuid(43), true);
        let generated = new_task(
            list.id,
            None,
            "Generated".into(),
            "7fffffffffffffffffffffffffffffff".into(),
            1,
        )
        .unwrap();
        let delete_hlc = test_hlc(1, "remote").encode().unwrap();
        let revision_hlc = test_hlc(2, "remote").encode().unwrap();
        let record = PullRecord {
            record_id: template.id,
            collection: SyncCollection::Templates,
            seq: 2,
            revision_hlc,
            state: EncryptedSyncState::Tombstone { delete_hlc },
        };
        let mut store = FakeStore::default();
        store.templates.insert(template.id, template.clone());
        store.series.insert(series.id, series.clone());
        store.tasks.insert(generated.id, generated.clone());
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_template(
            &record,
            &context_with_keys(&[]),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert!(!store.templates.contains_key(&template.id));
        assert_eq!(store.series.get(&series.id), Some(&series));
        assert_eq!(store.tasks.get(&generated.id), Some(&generated));
        assert_eq!(summary.deleted_count, 1);
    }

    #[test]
    fn remote_list_tombstone_rehomes_known_descendant_and_republishes_it() {
        let list_id = uuid(33);
        let list = sample_list(list_id, false);
        let default_list = sample_list(uuid(34), true);
        let task = new_task(
            list_id,
            None,
            "known descendant".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1_799_000_000_000,
        )
        .unwrap();
        let current_revision = Hlc {
            wall_ms: 1_799_000_000_100,
            counter: 0,
            device_id: "local".to_string(),
        }
        .encode()
        .unwrap();
        let delete_hlc = Hlc {
            wall_ms: 1_799_000_000_200,
            counter: 0,
            device_id: "remote".to_string(),
        }
        .encode()
        .unwrap();
        let mut store = FakeStore::default();
        store.lists.insert(default_list.id, default_list.clone());
        store.lists.insert(list_id, list);
        store.tasks.insert(task.id, task.clone());
        store.record_states.insert(
            (SyncCollection::Tasks, task.id),
            LocalSyncRecordState {
                current_revision_hlc: Some(current_revision.clone()),
                state: LocalSyncSemanticState::Live {
                    mutation_hlc: current_revision.clone(),
                    plaintext_json: serde_json::to_string(&task_plaintext(
                        &task,
                        Hlc::decode(&current_revision).unwrap(),
                    ))
                    .unwrap(),
                },
            },
        );
        store.outbox.push(LocalSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id: task.id,
            collection: SyncCollection::Tasks,
            base_revision_hlc: Some(current_revision.clone()),
            revision_hlc: current_revision,
            state: EncryptedSyncState::Live {
                mutation_hlc: delete_hlc.clone(),
                blob: vec![1],
            },
            created_at: 1,
        });
        let record = PullRecord {
            record_id: list_id,
            collection: SyncCollection::Lists,
            seq: 3,
            revision_hlc: delete_hlc.clone(),
            state: EncryptedSyncState::Tombstone {
                delete_hlc: delete_hlc.clone(),
            },
        };
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(
            &record,
            &context_for(list_id, [5; KEY_LEN]),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert_eq!(store.tasks.get(&task.id).unwrap().list_id, default_list.id);
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].record_id, task.id);
        assert!(matches!(
            store.outbox[0].state,
            EncryptedSyncState::Live { .. }
        ));
    }

    #[test]
    fn late_descendant_of_tombstoned_list_is_rehomed_to_default_inbox() {
        let list_id = uuid(31);
        let task_id = uuid(32);
        let default_list = sample_list(uuid(30), true);
        let dek = [4; KEY_LEN];
        let task = new_task(
            list_id,
            None,
            "late".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1_799_000_000_000,
        )
        .unwrap();
        let mut task = task;
        task.id = task_id;
        let hlc = Hlc {
            wall_ms: 1_799_000_000_200,
            counter: 0,
            device_id: "remote".to_string(),
        };
        let plaintext = task_plaintext(&task, hlc.clone());
        let record = encrypted_task_record(task_id, &plaintext, &dek, &hlc);
        let mut store = FakeStore::default();
        store.lists.insert(default_list.id, default_list.clone());
        store.record_states.insert(
            (SyncCollection::Lists, list_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(record.revision_hlc.clone()),
                state: LocalSyncSemanticState::Tombstone {
                    delete_hlc: record.revision_hlc.clone(),
                },
            },
        );
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_task(
            &record,
            &context_for(list_id, dek),
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert_eq!(store.tasks.get(&task_id).unwrap().list_id, default_list.id);
        assert_eq!(store.outbox.len(), 1);
        assert!(matches!(
            store.outbox[0].state,
            EncryptedSyncState::Live { .. }
        ));
        assert_eq!(summary.repush_count, 1);
    }

    #[test]
    fn conflict_current_merges_distinct_fields_and_rebases_without_first_client() {
        let list_id = uuid(4);
        let dek = [0x4c; KEY_LEN];
        let base_clock = Hlc {
            wall_ms: 1_799_000_000_000,
            counter: 0,
            device_id: "base".to_string(),
        };
        let local_clock = Hlc {
            wall_ms: 1_799_000_000_100,
            counter: 0,
            device_id: "client-b".to_string(),
        };
        let remote_clock = Hlc {
            wall_ms: 1_799_000_000_101,
            counter: 0,
            device_id: "client-a".to_string(),
        };
        let server_revision = Hlc {
            wall_ms: 1_799_000_000_102,
            counter: 0,
            device_id: "client-a".to_string(),
        }
        .encode()
        .unwrap();
        let base_list = sample_list(list_id, false);
        let mut local_list_for_plaintext = base_list.clone();
        local_list_for_plaintext.color = "#00ff00".to_string();
        let local_plaintext = list_plaintext(&base_list, base_clock.clone())
            .stamp_list_changes(&local_list_for_plaintext, local_clock.clone())
            .unwrap();
        let mut remote_list_for_plaintext = base_list.clone();
        remote_list_for_plaintext.name = "Remote name".to_string();
        let remote_plaintext = list_plaintext(&base_list, base_clock)
            .stamp_list_changes(&remote_list_for_plaintext, remote_clock.clone())
            .unwrap();

        let blob = encrypt_plaintext(
            &dek,
            LISTS_COLLECTION,
            &list_id.to_string(),
            &remote_plaintext,
        )
        .unwrap();
        let record = PullRecord {
            record_id: list_id,
            collection: SyncCollection::Lists,
            seq: 2,
            revision_hlc: server_revision.clone(),
            state: EncryptedSyncState::Live {
                mutation_hlc: remote_clock.encode().unwrap(),
                blob,
            },
        };
        let context = context_for(list_id, dek);
        let mut local_list = base_list;
        local_list.color = "#00ff00".to_string();
        let mut store = FakeStore::default();
        store.lists.insert(list_id, local_list);
        store.record_states.insert(
            (SyncCollection::Lists, list_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(
                    Hlc {
                        wall_ms: 1_799_000_000_001,
                        counter: 0,
                        device_id: "base".to_string(),
                    }
                    .encode()
                    .unwrap(),
                ),
                state: LocalSyncSemanticState::Live {
                    mutation_hlc: local_clock.encode().unwrap(),
                    plaintext_json: serde_json::to_string(&local_plaintext).unwrap(),
                },
            },
        );
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

        let merged_list = store.lists.get(&list_id).unwrap();
        assert_eq!(merged_list.name, "Remote name");
        assert_eq!(merged_list.color, "#00ff00");
        assert_eq!(summary.repush_count, 1);
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(
            store.outbox[0].base_revision_hlc.as_deref(),
            Some(server_revision.as_str())
        );
        let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
            panic!("expected rebased live head");
        };
        let rebased =
            decrypt_plaintext(&dek, LISTS_COLLECTION, &list_id.to_string(), blob).unwrap();
        let SyncPlaintext::List(rebased) = rebased else {
            panic!("list");
        };
        assert_eq!(rebased.name.value, "Remote name");
        assert_eq!(rebased.color.value, "#00ff00");
    }

    #[test]
    fn undecryptable_conflict_current_keeps_the_local_outbox_head() {
        let list_id = uuid(5);
        let dek = [0x5d; KEY_LEN];
        let semantic_hlc = Hlc {
            wall_ms: 1_799_000_000_010,
            counter: 0,
            device_id: "local".to_string(),
        }
        .encode()
        .unwrap();
        let revision_hlc = Hlc {
            wall_ms: 1_799_000_000_011,
            counter: 0,
            device_id: "local".to_string(),
        }
        .encode()
        .unwrap();
        let stale_op_id = Uuid::now_v7();
        let mut store = FakeStore::default();
        store.outbox.push(LocalSyncOutboxEntry {
            op_id: stale_op_id,
            record_id: list_id,
            collection: SyncCollection::Lists,
            base_revision_hlc: None,
            revision_hlc: revision_hlc.clone(),
            state: EncryptedSyncState::Live {
                mutation_hlc: semantic_hlc,
                blob: vec![1, 2, 3],
            },
            created_at: 1,
        });
        let current = PullRecord {
            record_id: list_id,
            collection: SyncCollection::Lists,
            seq: 1,
            revision_hlc,
            state: EncryptedSyncState::Live {
                mutation_hlc: Hlc {
                    wall_ms: 1_799_000_000_009,
                    counter: 0,
                    device_id: "remote".to_string(),
                }
                .encode()
                .unwrap(),
                blob: vec![0xff; crate::envelope::ENVELOPE_MIN_LEN],
            },
        };
        let context = context_for(list_id, dek);
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        assert_eq!(
            apply_pull_record(&current, &context, &mut store, &mut now, &mut summary).unwrap(),
            ApplyDisposition::UpgradeRequired(0xff)
        );

        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].op_id, stale_op_id);
        assert_eq!(summary.decrypt_failed_count, 0);
    }

    #[test]
    fn pull_rejects_authenticated_task_with_unencodable_field_clock() {
        let list_id = uuid(50);
        let record_id = uuid(51);
        let dek = [0x51; KEY_LEN];
        let task = new_task(
            list_id,
            None,
            "invalid clock".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1,
        )
        .unwrap();
        let clock = Hlc {
            wall_ms: 1_799_000_000_050,
            counter: 0,
            device_id: "remote".to_string(),
        };
        let mut plaintext = SyncPlaintext::from_task(&task, clock.clone()).unwrap();
        let SyncPlaintext::Task(fields) = &mut plaintext else {
            panic!("task");
        };
        fields.note.hlc.device_id.clear();
        let plaintext_json = serde_json::to_vec(&plaintext).unwrap();
        let mut aad = Vec::new();
        aad.extend_from_slice(b"TDA5");
        aad.extend_from_slice(&taskveil_crypto::CRYPTO_SUITE_ID.to_be_bytes());
        aad.extend_from_slice(&1_u64.to_be_bytes());
        aad.extend_from_slice(test_tenant_id().as_bytes());
        aad.extend_from_slice(&(TASKS_COLLECTION.len() as u16).to_be_bytes());
        aad.extend_from_slice(TASKS_COLLECTION.as_bytes());
        aad.extend_from_slice(record_id.as_bytes());
        let mut blob = Vec::new();
        blob.extend_from_slice(b"TDE5");
        blob.extend_from_slice(&taskveil_crypto::CRYPTO_SUITE_ID.to_be_bytes());
        blob.extend_from_slice(&1_u64.to_be_bytes());
        let record_key = taskveil_crypto::key_hierarchy::derive_record_key(
            &dek,
            test_tenant_id(),
            1,
            TASKS_COLLECTION,
            record_id,
        )
        .unwrap();
        blob.extend_from_slice(
            &taskveil_crypto::encrypt(&record_key, &plaintext_json, &aad).unwrap(),
        );
        let record = PullRecord {
            record_id,
            collection: SyncCollection::Tasks,
            seq: 1,
            revision_hlc: Hlc {
                wall_ms: clock.wall_ms + 1,
                ..clock.clone()
            }
            .encode()
            .unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: clock.encode().unwrap(),
                blob,
            },
        };
        let context = context_for(list_id, dek);
        let mut store = FakeStore::default();
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        assert_eq!(
            apply_pull_record(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
            ApplyDisposition::Deferred(PullFailureReason::InvalidPlaintext, None)
        );
        assert_eq!(summary.decrypt_failed_count, 0);
        assert!(!store
            .record_states
            .contains_key(&(SyncCollection::Tasks, record_id)));
    }

    #[test]
    fn stale_response_does_not_apply_after_a_newer_local_head_replaces_its_op() {
        let list_id = uuid(6);
        let dek = [0x6e; KEY_LEN];
        let current = encrypted_list_record(
            &List {
                name: "Remote stale".to_string(),
                ..sample_list(list_id, false)
            },
            &dek,
        );
        let newer_op_id = Uuid::now_v7();
        let clock = Hlc {
            wall_ms: 1_799_000_000_020,
            counter: 0,
            device_id: "new-local".to_string(),
        }
        .encode()
        .unwrap();
        let mut store = FakeStore::default();
        store.lists.insert(
            list_id,
            List {
                name: "New local".to_string(),
                ..sample_list(list_id, false)
            },
        );
        store.outbox.push(LocalSyncOutboxEntry {
            op_id: newer_op_id,
            record_id: list_id,
            collection: SyncCollection::Lists,
            base_revision_hlc: None,
            revision_hlc: clock.clone(),
            state: EncryptedSyncState::Live {
                mutation_hlc: clock,
                blob: vec![1],
            },
            created_at: 1,
        });
        let context = context_for(list_id, dek);
        let mut now = ticking_now();
        let mut summary = SyncRunSummary::default();

        reconcile_nonaccepted_push_in_transaction(
            &current,
            Uuid::now_v7(),
            &context,
            &mut store,
            &mut now,
            &mut summary,
        )
        .unwrap();

        assert_eq!(store.lists[&list_id].name, "New local");
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].op_id, newer_op_id);
        assert_eq!(summary.applied_count, 0);
    }

    fn encrypted_list_record(list: &List, dek: &[u8; KEY_LEN]) -> PullRecord {
        let hlc = Hlc {
            wall_ms: list.updated_at,
            counter: 0,
            device_id: "remote".to_string(),
        };
        let plaintext = list_plaintext(list, hlc.clone());
        let blob = encrypt_plaintext(dek, LISTS_COLLECTION, &list.id.to_string(), &plaintext)
            .expect("test list plaintext encrypts");
        PullRecord {
            record_id: list.id,
            collection: SyncCollection::Lists,
            seq: 1,
            revision_hlc: hlc.encode().unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: hlc.encode().unwrap(),
                blob,
            },
        }
    }

    fn put_live_plaintext_state(
        store: &mut FakeStore,
        collection: SyncCollection,
        record_id: Uuid,
        plaintext: SyncPlaintext,
    ) {
        let mutation_hlc = plaintext.record_hlc().encode().unwrap();
        store.record_states.insert(
            (collection, record_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(mutation_hlc.clone()),
                state: LocalSyncSemanticState::Live {
                    mutation_hlc,
                    plaintext_json: serde_json::to_string(&plaintext).unwrap(),
                },
            },
        );
    }

    fn encrypted_task_record(
        record_id: Uuid,
        plaintext: &SyncPlaintext,
        dek: &[u8; KEY_LEN],
        hlc: &Hlc,
    ) -> PullRecord {
        PullRecord {
            record_id,
            collection: SyncCollection::Tasks,
            seq: 1,
            revision_hlc: hlc.encode().unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: hlc.encode().unwrap(),
                blob: encrypt_plaintext(dek, TASKS_COLLECTION, &record_id.to_string(), plaintext)
                    .unwrap(),
            },
        }
    }

    fn context_for(list_id: Uuid, dek: [u8; KEY_LEN]) -> ActiveSyncContext {
        context_with_keys(&[(list_id, dek)])
    }

    #[test]
    fn persisted_manifest_anchor_requires_complete_authenticated_successor_chain() {
        let tenant_id = test_tenant_id();
        let first = crate::KeyManifest::authenticate_personal(
            tenant_id,
            1,
            crate::RotationStatus::Active,
            1,
            [0; 32],
            Vec::new(),
            &[0x41; 32],
        )
        .unwrap();
        let prepared = crate::KeyManifest::authenticate_personal(
            tenant_id,
            2,
            crate::RotationStatus::Prepared,
            1,
            first.authenticated_hash().unwrap(),
            Vec::new(),
            &[0x41; 32],
        )
        .unwrap();
        let active = crate::KeyManifest::authenticate_personal(
            tenant_id,
            2,
            crate::RotationStatus::Active,
            2,
            prepared.authenticated_hash().unwrap(),
            Vec::new(),
            &[0x41; 32],
        )
        .unwrap();
        let mut store = FakeStore::default();
        store.settings.insert(
            manifest_anchor_key(),
            STANDARD.encode(first.authenticated_bytes().unwrap()),
        );
        let context = context_with_keys(&[]);
        let descriptor = crate::protocol::KeyManifestDescriptor {
            suite_id: taskveil_crypto::CRYPTO_SUITE_ID,
            generation: 2,
            status: crate::RotationStatus::Active,
            minimum_write_generation: 2,
            signed_manifest: STANDARD.encode(active.authenticated_bytes().unwrap()),
            predecessor_manifests: vec![STANDARD.encode(prepared.authenticated_bytes().unwrap())],
        };

        verify_manifest_anchor(&descriptor, &active, &context, &mut store).unwrap();
        let mut missing_predecessor = descriptor;
        missing_predecessor.predecessor_manifests.clear();
        assert!(
            verify_manifest_anchor(&missing_predecessor, &active, &context, &mut store).is_err()
        );
    }

    fn context_with_keys(keys: &[(Uuid, [u8; KEY_LEN])]) -> ActiveSyncContext {
        ActiveSyncContext {
            server_url: "http://localhost".to_string(),
            tenant_id: uuid(100),
            device_id: "local".to_string(),
            session_token: crate::SecretString::new("token"),
            keys: LocalSyncKeys {
                tenant_id: test_tenant_id(),
                tenant_root_dek: Some(Zeroizing::new(
                    keys.first().map_or([0x7b; KEY_LEN], |(_, key)| *key),
                )),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            manifest_auth_key: crate::derive_personal_manifest_auth_key(&[0x41; 32]).unwrap(),
        }
    }

    #[test]
    fn active_context_debug_never_renders_bearer_token() {
        let mut context = context_with_keys(&[]);
        context.session_token = crate::SecretString::new("bearer-secret");
        let rendered = format!("{context:?}");

        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("bearer-secret"));
    }

    fn test_hlc(counter: u32, device_id: &str) -> Hlc {
        Hlc {
            wall_ms: 1_799_000_000_000,
            counter,
            device_id: device_id.to_string(),
        }
    }

    fn sample_list(id: Uuid, is_default: bool) -> List {
        List {
            id,
            name: format!("List {id}"),
            color: "#ffffff".to_string(),
            icon: "list".to_string(),
            sort_order: "7fffffffffffffffffffffffffffffff".to_string(),
            is_default,
            archived_at: None,
            created_at: 1_799_000_000_000,
            updated_at: 1_799_000_000_000,
        }
    }

    fn sample_series(id: Uuid) -> TaskSeries {
        TaskSeries {
            id,
            config: TaskSeriesConfig {
                blueprint: TaskBlueprint {
                    schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                    nodes: vec![TaskBlueprintNode {
                        node_key: "root".into(),
                        parent_node_key: None,
                        sibling_order: 0,
                        content: TaskContent {
                            title: "Generated".into(),
                            note: String::new(),
                            priority: 0,
                            estimated_minutes: None,
                        },
                    }],
                },
                target_list_id: None,
                rrule: "FREQ=DAILY;COUNT=1".into(),
                starts_at: 1_800_000_000_000,
                time_zone: "UTC".into(),
                enabled: true,
                config_revision: "revision-a".into(),
                config_parent_revision: None,
                config_effective_from: 1,
                lineage: Vec::new(),
            },
            cursor: SeriesCursor::Pending(1_800_000_000_000),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn sample_template(id: Uuid) -> TaskTemplate {
        TaskTemplate {
            id,
            name: "Reusable".into(),
            default_list_id: None,
            blueprint: sample_series(uuid(999)).config.blueprint,
            blueprint_revision: "template-revision-a".into(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn ticking_now() -> impl FnMut() -> Result<i64, String> {
        let mut now = 1_799_000_000_000;
        move || {
            now += 1;
            Ok(now)
        }
    }

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }
}
#[test]
fn durable_upgrade_block_reopens_when_supported_versions_catch_up() {
    assert!(upgrade_block_is_active("9:5"));
    assert!(upgrade_block_is_active("8:6"));
    assert!(!upgrade_block_is_active("8:5"));
    assert!(!upgrade_block_is_active("6:5"));
    assert!(!upgrade_block_is_active("7:3"));
    assert!(!upgrade_block_is_active("0:0"));
    assert!(upgrade_block_is_active("invalid"));
}
