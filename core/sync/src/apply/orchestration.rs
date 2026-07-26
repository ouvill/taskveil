use super::*;

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
    // Re-authentication can assign a fresh server device UUID to an existing
    // encrypted profile. Assert the durable clock/outbox identity before any
    // network request so a partially completed login cannot push an old node.
    let mut identity_transaction = store.begin_write_transaction()?;
    rebind_local_device(&mut identity_transaction, &context.device_id, now_ms)?;
    identity_transaction.commit()?;

    let engine = SyncEngine::new(
        context.server_url.clone(),
        context.tenant_id,
        context.session_token.expose(),
    )
    .map_err(|_| "sync failed".to_string())?;
    let mut summary = SyncRunSummary::default();

    let durable_upgrade_block = store.get_setting(SYNC_UPGRADE_REQUIRED_METADATA_KEY)?;
    if durable_upgrade_block
        .as_deref()
        .is_some_and(upgrade_block_is_active)
    {
        return Err("upgrade required".to_string());
    }
    let since = store.get_cursor_seq(SYNC_CURSOR_NAME)?.unwrap_or(0);
    store.preflight_network_request()?;
    let preflight = match engine.preflight(since).await {
        Ok(preflight) => {
            if durable_upgrade_block.is_some() {
                store.set_setting(SYNC_UPGRADE_REQUIRED_METADATA_KEY, "0:0", now_ms()?)?;
            }
            preflight
        }
        Err(SyncEngineError::UpgradeRequired {
            protocol_version,
            envelope_version,
        }) => {
            store.set_setting(
                SYNC_UPGRADE_REQUIRED_METADATA_KEY,
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
        store.preflight_network_request()?;
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
            store.preflight_network_request()?;
            context.keys = key_refresher.refresh().await?;
        }
        if let Err(error) = replay_quarantine(&context, store, now_ms, &mut summary) {
            if let Some(envelope_version) = replay_upgrade_version(&error) {
                store.set_setting(
                    SYNC_UPGRADE_REQUIRED_METADATA_KEY,
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
        store.preflight_network_request()?;
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
        store.preflight_network_request()?;
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

pub(super) fn manifest_anchor_key() -> String {
    "key_manifest_anchor:tenant".to_string()
}

pub(super) fn verify_manifest_anchor<S: LocalSyncStore>(
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

pub(super) fn reconcile_canonical_inbox_in_transaction<S, N>(
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
        store.preflight_network_request()?;
        let page = engine
            .pull_page(since, 100)
            .await
            .map_err(sync_engine_error_to_string)?;
        match apply_pull_page(&page, context, store, now_ms, false) {
            Ok(page_summary) => merge_summary(summary, page_summary),
            Err(PageApplyError::MissingKey) => {
                store.preflight_network_request()?;
                context.keys = key_refresher.refresh().await?;
                refreshed = true;
                if let Err(error) = replay_quarantine(context, store, now_ms, summary) {
                    if let Some(envelope_version) = replay_upgrade_version(&error) {
                        store.set_setting(
                            SYNC_UPGRADE_REQUIRED_METADATA_KEY,
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
                    SYNC_UPGRADE_REQUIRED_METADATA_KEY,
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
            store.preflight_network_request()?;
            engine
                .ack_continuity(proof)
                .await
                .map_err(sync_engine_error_to_string)?;
            return Ok(refreshed);
        }
    }
}
