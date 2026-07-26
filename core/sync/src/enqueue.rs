use taskveil_crypto::key_hierarchy::KEY_LEN;
use taskveil_domain::{CompletedTimerSession, List, Task, TaskSeries, TaskTemplate, Uuid};

use crate::{
    encrypt_plaintext, EncryptedSyncState, Hlc, SyncCollection, SyncPlaintext,
    SYNC_LOCAL_HLC_METADATA_KEY,
};

use crate::keys::{tenant_root_dek, LocalSyncKeys};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: EncryptedSyncState,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLocalSyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: EncryptedSyncState,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSyncSemanticState {
    Live {
        mutation_hlc: String,
        plaintext_json: String,
    },
    Tombstone {
        delete_hlc: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSyncRecordState {
    pub current_revision_hlc: Option<String>,
    pub state: LocalSyncSemanticState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalListAlias {
    pub alias_list_id: Uuid,
    pub canonical_list_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullFailureReason {
    MissingDek,
    NoMatchingDek,
    AuthenticationFailed,
    CorruptEnvelope,
    InvalidPlaintext,
    MissingDependency,
}

impl PullFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingDek => "missing_dek",
            Self::NoMatchingDek => "no_matching_dek",
            Self::AuthenticationFailed => "authentication_failed",
            Self::CorruptEnvelope => "corrupt_envelope",
            Self::InvalidPlaintext => "invalid_plaintext",
            Self::MissingDependency => "missing_dependency",
        }
    }
}

impl std::str::FromStr for PullFailureReason {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "missing_dek" => Ok(Self::MissingDek),
            "no_matching_dek" => Ok(Self::NoMatchingDek),
            "authentication_failed" => Ok(Self::AuthenticationFailed),
            "corrupt_envelope" => Ok(Self::CorruptEnvelope),
            "invalid_plaintext" => Ok(Self::InvalidPlaintext),
            "missing_dependency" => Ok(Self::MissingDependency),
            _ => Err("invalid quarantine reason".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSyncQuarantineEntry {
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub seq: i64,
    pub revision_hlc: String,
    pub state: EncryptedSyncState,
    pub reason: PullFailureReason,
    pub required_list_id: Option<Uuid>,
    pub first_failed_at: i64,
    pub last_failed_at: i64,
    pub attempt_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFullResyncPhase {
    Base,
    BaseAwaitingAck,
    Delta,
    Sweep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFullResyncProgress {
    pub generation_id: Uuid,
    pub continuity_generation: i64,
    pub phase: LocalFullResyncPhase,
    pub base_seq: i64,
    pub base_cursor: Option<crate::StableCursor>,
    pub delta_cursor: i64,
    pub closure_high_water: Option<i64>,
    pub sweep_cursor: Option<crate::StableCursor>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalFullResyncSweepSummary {
    pub scanned_records: usize,
    pub swept_lists: usize,
    pub swept_tasks: usize,
    pub swept_templates: usize,
    pub swept_task_series: usize,
    pub swept_timer_sessions: usize,
    pub swept_record_states: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceRebindSummary {
    pub rebound_outbox_heads: usize,
}

/// The local persistence operations needed to prepare a domain mutation for sync.
///
/// Implementations must arrange for these writes and the corresponding domain
/// write to participate in the same transaction.
pub trait LocalMutationSyncStore {
    fn has_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String>;
    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String>;
    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String>;
    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String>;
    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String>;
    fn put_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
        state: LocalSyncRecordState,
        updated_at: i64,
    ) -> Result<(), String>;
}

/// The complete local persistence surface used by a sync run.
///
/// Pull/apply composes the mutation state with outbox draining, cursor updates,
/// and domain row application. Mutation-only callers should depend on
/// [`LocalMutationSyncStore`] instead.
pub trait LocalSyncStore: LocalMutationSyncStore {
    fn load_full_resync(&mut self) -> Result<Option<LocalFullResyncProgress>, String> {
        Ok(None)
    }
    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String>;
    /// Lists every pending head, including records currently quarantined.
    fn list_all_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        self.list_outbox_heads(limit)
    }
    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String>;
    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String>;
    fn get_cursor_seq(&mut self, name: &str) -> Result<Option<i64>, String>;
    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), String>;
    fn delete_cursor(&mut self, name: &str) -> Result<(), String>;
    fn put_quarantine(&mut self, _entry: LocalSyncQuarantineEntry) -> Result<(), String> {
        Err("durable quarantine is unavailable".to_string())
    }
    fn list_quarantine(&mut self, _limit: usize) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        Ok(Vec::new())
    }
    fn list_replayable_quarantine(
        &mut self,
        _after: Option<(i64, Uuid)>,
        _limit: usize,
    ) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        Err("replayable durable quarantine is unavailable".to_string())
    }
    fn delete_quarantine(&mut self, _record_id: Uuid) -> Result<bool, String> {
        Ok(false)
    }
    fn list_record_states(
        &mut self,
        collection: SyncCollection,
    ) -> Result<Vec<(Uuid, LocalSyncRecordState)>, String>;
    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String>;
    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String>;
    fn replace_list_aliases(
        &mut self,
        aliases: &[LocalListAlias],
        updated_at: i64,
    ) -> Result<(), String>;
    fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String>;
    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String>;
    fn default_list_id(&mut self) -> Result<Option<Uuid>, String>;
    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String>;
    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String>;
    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String>;
    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String>;
    fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String>;
    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String>;
    fn list_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<Vec<Task>, String> {
        Ok(self.get_task(task_id)?.into_iter().collect())
    }
    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String>;
    fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, String>;
    fn get_template(&mut self, _id: Uuid) -> Result<Option<TaskTemplate>, String> {
        Ok(None)
    }
    fn upsert_template_for_sync(&mut self, _template: TaskTemplate) -> Result<(), String> {
        Err("template storage is unavailable".to_string())
    }
    fn delete_template_for_sync(&mut self, _id: Uuid) -> Result<bool, String> {
        Ok(false)
    }
    fn get_series(&mut self, _id: Uuid) -> Result<Option<TaskSeries>, String> {
        Ok(None)
    }
    fn upsert_series_for_sync(&mut self, _series: TaskSeries) -> Result<(), String> {
        Err("task series storage is unavailable".to_string())
    }
    fn delete_series_for_sync(&mut self, _id: Uuid) -> Result<bool, String> {
        Ok(false)
    }
    fn get_timer_session(&mut self, _id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        Ok(None)
    }
    fn upsert_timer_session_for_sync(
        &mut self,
        _session: CompletedTimerSession,
    ) -> Result<(), String> {
        Err("timer session storage is unavailable".to_string())
    }
    fn delete_timer_session_for_sync(&mut self, _id: Uuid) -> Result<bool, String> {
        Ok(false)
    }
    fn list_timer_sessions_by_task(
        &mut self,
        _task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        Ok(Vec::new())
    }
    fn clear_active_timer_for_task(&mut self, _task_id: Uuid) -> Result<bool, String> {
        Ok(false)
    }
}

pub trait LocalSyncWriteTransaction: LocalSyncStore {
    fn start_full_resync(
        &mut self,
        _generation_id: Uuid,
        _continuity_generation: i64,
        _base_seq: i64,
        _now_ms: i64,
    ) -> Result<LocalFullResyncProgress, String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn mark_full_resync_record(
        &mut self,
        _generation_id: Uuid,
        _collection: SyncCollection,
        _record_id: Uuid,
    ) -> Result<(), String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn advance_full_resync_base(
        &mut self,
        _generation_id: Uuid,
        _next_cursor: Option<&crate::StableCursor>,
        _base_complete: bool,
        _now_ms: i64,
    ) -> Result<(), String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn advance_full_resync_delta(
        &mut self,
        _generation_id: Uuid,
        _delta_cursor: i64,
        _now_ms: i64,
    ) -> Result<(), String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn enter_full_resync_sweep(
        &mut self,
        _generation_id: Uuid,
        _closure_high_water: i64,
        _now_ms: i64,
    ) -> Result<(), String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn sweep_full_resync_batch(
        &mut self,
        _generation_id: Uuid,
        _limit: usize,
        _now_ms: i64,
    ) -> Result<LocalFullResyncSweepSummary, String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn finalize_full_resync(
        &mut self,
        _generation_id: Uuid,
        _cursor_name: &str,
        _now_ms: i64,
    ) -> Result<i64, String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn reset_full_resync(&mut self) -> Result<(), String> {
        Err("durable full resync is unavailable".to_string())
    }
    fn commit(self) -> Result<(), String>;
}

pub trait LocalSyncAtomicStore: LocalSyncStore {
    type WriteTransaction: LocalSyncWriteTransaction;

    /// Revalidates the active sync lease immediately before starting a remote
    /// request.
    ///
    /// A write fence after receiving a response is not sufficient: once a
    /// suspended run loses its lease it must not create any further remote side
    /// effects. Orchestrators therefore call this explicit gate at every
    /// network boundary, independently of local read behavior.
    fn preflight_network_request(&mut self) -> Result<(), String>;

    fn begin_write_transaction(&mut self) -> Result<Self::WriteTransaction, String>;
}

/// Rebinds the durable local HLC and every pending outbox revision to a newly
/// authenticated device identity.
///
/// The caller must pass a write transaction and commit it only together with
/// the account-device binding. On failure, dropping that transaction must roll
/// back the clock and every replaced outbox head.
pub fn rebind_local_device<S, N>(
    store: &mut S,
    new_device_id: &str,
    now_ms: &mut N,
) -> Result<DeviceRebindSummary, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let stored_clock = store
        .get_setting(SYNC_LOCAL_HLC_METADATA_KEY)?
        .filter(|encoded| !encoded.is_empty())
        .map(|encoded| Hlc::decode(&encoded).map_err(|_| "sync failed".to_string()))
        .transpose()?;
    let all_limit = usize::try_from(i64::MAX).unwrap_or(usize::MAX);
    let outbox = store.list_all_outbox_heads(all_limit)?;
    let outbox_clocks = outbox
        .iter()
        .map(|entry| Hlc::decode(&entry.revision_hlc).map_err(|_| "sync failed".to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    if stored_clock.as_ref().is_some_and(|clock| {
        clock.device_id == new_device_id
            && outbox_clocks.iter().all(|revision| {
                revision.device_id == new_device_id && compare_hlc(clock, revision).is_ge()
            })
    }) {
        return Ok(DeviceRebindSummary::default());
    }

    let maximum = stored_clock
        .iter()
        .chain(outbox_clocks.iter())
        .max_by(|left, right| compare_hlc(left, right));
    let mut clock = Hlc::new(new_device_id.to_string());
    if let Some(maximum) = maximum {
        clock
            .merge(maximum, now_ms()?)
            .map_err(|_| "sync failed".to_string())?;
    } else {
        clock
            .now(now_ms()?)
            .map_err(|_| "sync failed".to_string())?;
    }

    for entry in outbox {
        let revision = clock
            .now(now_ms()?)
            .and_then(|value| value.encode())
            .map_err(|_| "sync failed".to_string())?;
        store.put_outbox_head(NewLocalSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id: entry.record_id,
            collection: entry.collection,
            base_revision_hlc: entry.base_revision_hlc,
            revision_hlc: revision,
            state: entry.state,
            created_at: entry.created_at,
        })?;
    }
    store.set_setting(
        SYNC_LOCAL_HLC_METADATA_KEY,
        &clock.encode().map_err(|_| "sync failed".to_string())?,
        now_ms()?,
    )?;
    Ok(DeviceRebindSummary {
        rebound_outbox_heads: outbox_clocks.len(),
    })
}

fn compare_hlc(left: &Hlc, right: &Hlc) -> std::cmp::Ordering {
    left.wall_ms
        .cmp(&right.wall_ms)
        .then_with(|| left.counter.cmp(&right.counter))
        .then_with(|| left.device_id.cmp(&right.device_id))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackfillSummary {
    pub enqueued_lists: usize,
    pub enqueued_templates: usize,
    pub enqueued_task_series: usize,
    pub enqueued_tasks: usize,
    pub enqueued_timer_sessions: usize,
    pub skipped_existing_outbox: usize,
    pub skipped_missing_dek: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BackfillRecords<'a> {
    pub lists: &'a [List],
    pub templates: &'a [TaskTemplate],
    pub task_series: &'a [TaskSeries],
    pub tasks: &'a [Task],
    pub timer_sessions: &'a [CompletedTimerSession],
}

pub fn enqueue_backfill<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    records: BackfillRecords<'_>,
    now_ms: &mut N,
) -> Result<BackfillSummary, String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut summary = BackfillSummary::default();

    for list in records.lists {
        if store.has_outbox_head(SyncCollection::Lists, list.id)? {
            summary.skipped_existing_outbox += 1;
            continue;
        }
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_list_sync(store, keys, device_id, list, false, now_ms)?;
        summary.enqueued_lists += 1;
    }

    for template in records.templates {
        if store.has_outbox_head(SyncCollection::Templates, template.id)? {
            summary.skipped_existing_outbox += 1;
            continue;
        }
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_template_sync(store, keys, device_id, template, false, now_ms)?;
        summary.enqueued_templates += 1;
    }

    for series in records.task_series {
        if store.has_outbox_head(SyncCollection::TaskSeries, series.id)? {
            summary.skipped_existing_outbox += 1;
            continue;
        }
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_task_series_sync(store, keys, device_id, series, false, now_ms)?;
        summary.enqueued_task_series += 1;
    }

    let mut sorted_tasks = records.tasks.iter().collect::<Vec<_>>();
    sorted_tasks.sort_by_key(|task| (task.created_at, task.id));
    for task in sorted_tasks {
        if store.has_outbox_head(SyncCollection::Tasks, task.id)? {
            summary.skipped_existing_outbox += 1;
            continue;
        }
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_task_sync(store, keys, device_id, task, false, now_ms)?;
        summary.enqueued_tasks += 1;
    }

    let mut sorted_sessions = records.timer_sessions.iter().collect::<Vec<_>>();
    sorted_sessions.sort_by_key(|session| (session.created_at, session.id));
    for session in sorted_sessions {
        if store.has_outbox_head(SyncCollection::TimerSessions, session.id)? {
            summary.skipped_existing_outbox += 1;
            continue;
        }
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_timer_session_sync(store, keys, device_id, session, false, now_ms)?;
        summary.enqueued_timer_sessions += 1;
    }

    Ok(summary)
}

/// Re-encrypts every locally live head with the active key generation.
/// Existing outbox heads are intentionally replaced; tombstones have no
/// domain row and therefore remain untouched.
pub fn enqueue_rotation_backfill<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    records: BackfillRecords<'_>,
    now_ms: &mut N,
) -> Result<BackfillSummary, String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    keys.validate_for_write().map_err(str::to_string)?;
    if store
        .get_setting(crate::KEY_ROTATION_PENDING_METADATA_KEY)?
        .as_deref()
        .is_some_and(|value| value != "0" && value != keys.tenant_generation.to_string())
    {
        return Err("active key generation required".to_string());
    }
    store.set_setting(crate::KEY_ROTATION_PENDING_METADATA_KEY, "0", now_ms()?)?;
    let mut summary = BackfillSummary::default();
    for list in records.lists {
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_list_sync(store, keys, device_id, list, false, now_ms)?;
        summary.enqueued_lists += 1;
    }
    for template in records.templates {
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_template_sync(store, keys, device_id, template, false, now_ms)?;
        summary.enqueued_templates += 1;
    }
    for series in records.task_series {
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_task_series_sync(store, keys, device_id, series, false, now_ms)?;
        summary.enqueued_task_series += 1;
    }
    let mut sorted_tasks = records.tasks.iter().collect::<Vec<_>>();
    sorted_tasks.sort_by_key(|task| (task.created_at, task.id));
    for task in sorted_tasks {
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_task_sync(store, keys, device_id, task, false, now_ms)?;
        summary.enqueued_tasks += 1;
    }
    let mut sorted_sessions = records.timer_sessions.iter().collect::<Vec<_>>();
    sorted_sessions.sort_by_key(|session| (session.created_at, session.id));
    for session in sorted_sessions {
        if tenant_root_dek(keys).is_none() {
            summary.skipped_missing_dek += 1;
            continue;
        }
        enqueue_timer_session_sync(store, keys, device_id, session, false, now_ms)?;
        summary.enqueued_timer_sessions += 1;
    }
    store.set_setting(
        crate::KEY_ROTATION_PENDING_METADATA_KEY,
        &keys.tenant_generation.to_string(),
        now_ms()?,
    )?;
    Ok(summary)
}

pub fn enqueue_task_sync<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    task: &Task,
    deleted: bool,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    ensure_no_pending_rotation(store)?;
    let hlc = tick_local_hlc(store, device_id, now_ms)?;
    let encoded_hlc = hlc.encode().map_err(|_| "sync failed".to_string())?;
    let previous_state = store.get_record_state(SyncCollection::Tasks, task.id)?;
    let base_revision_hlc = previous_state
        .as_ref()
        .and_then(|state| state.current_revision_hlc.clone());
    let dek =
        tenant_root_dek(keys).ok_or_else(|| "missing Tenant Key for task sync".to_string())?;
    let generation = keys.tenant_generation;
    let plaintext = changed_task_plaintext(previous_state.as_ref(), task, hlc.clone())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: task.id,
            collection: SyncCollection::Tasks,
            deleted,
            plaintext: &plaintext,
            dek,
            tenant_id: keys.tenant_id,
            generation,
            revision_hlc: &hlc,
            semantic_hlc: &encoded_hlc,
            base_revision_hlc,
        },
        now_ms,
    )
}

pub fn enqueue_template_sync<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    template: &TaskTemplate,
    deleted: bool,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    ensure_no_pending_rotation(store)?;
    let dek = tenant_root_dek(keys)
        .ok_or_else(|| "missing Tenant Root DEK for template sync".to_string())?;
    if keys.tenant_id.is_nil() || keys.tenant_generation == 0 {
        return Err("missing active tenant key generation".to_string());
    }
    let hlc = tick_local_hlc(store, device_id, now_ms)?;
    let encoded_hlc = hlc.encode().map_err(|_| "sync failed".to_string())?;
    let previous_state = store.get_record_state(SyncCollection::Templates, template.id)?;
    let base_revision_hlc = previous_state
        .as_ref()
        .and_then(|state| state.current_revision_hlc.clone());
    let plaintext = changed_template_plaintext(previous_state.as_ref(), template, hlc.clone())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: template.id,
            collection: SyncCollection::Templates,
            deleted,
            plaintext: &plaintext,
            dek,
            tenant_id: keys.tenant_id,
            generation: keys.tenant_generation,
            revision_hlc: &hlc,
            semantic_hlc: &encoded_hlc,
            base_revision_hlc,
        },
        now_ms,
    )
}

pub fn enqueue_task_series_sync<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    series: &TaskSeries,
    deleted: bool,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    ensure_no_pending_rotation(store)?;
    let dek = tenant_root_dek(keys)
        .ok_or_else(|| "missing Tenant Root DEK for task series sync".to_string())?;
    if keys.tenant_id.is_nil() || keys.tenant_generation == 0 {
        return Err("missing active tenant key generation".to_string());
    }
    let hlc = tick_local_hlc(store, device_id, now_ms)?;
    let encoded_hlc = hlc.encode().map_err(|_| "sync failed".to_string())?;
    let previous_state = store.get_record_state(SyncCollection::TaskSeries, series.id)?;
    let base_revision_hlc = previous_state
        .as_ref()
        .and_then(|state| state.current_revision_hlc.clone());
    let plaintext = changed_task_series_plaintext(previous_state.as_ref(), series, hlc.clone())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: series.id,
            collection: SyncCollection::TaskSeries,
            deleted,
            plaintext: &plaintext,
            dek,
            tenant_id: keys.tenant_id,
            generation: keys.tenant_generation,
            revision_hlc: &hlc,
            semantic_hlc: &encoded_hlc,
            base_revision_hlc,
        },
        now_ms,
    )
}

pub fn enqueue_timer_session_sync<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    session: &CompletedTimerSession,
    deleted: bool,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    ensure_no_pending_rotation(store)?;
    let dek = tenant_root_dek(keys)
        .ok_or_else(|| "missing Tenant Root DEK for timer sync".to_string())?;
    if keys.tenant_id.is_nil() || keys.tenant_generation == 0 {
        return Err("missing active tenant key generation".to_string());
    }
    let hlc = tick_local_hlc(store, device_id, now_ms)?;
    let encoded_hlc = hlc.encode().map_err(|_| "sync failed".to_string())?;
    let previous_state = store.get_record_state(SyncCollection::TimerSessions, session.id)?;
    if let Some(LocalSyncRecordState {
        state: LocalSyncSemanticState::Live { plaintext_json, .. },
        ..
    }) = previous_state.as_ref()
    {
        let previous: SyncPlaintext =
            serde_json::from_str(plaintext_json).map_err(|_| "sync failed".to_string())?;
        let SyncPlaintext::TimerSession(previous) = previous else {
            return Err("sync failed".to_string());
        };
        if previous.value != *session {
            return Err("immutable timer session contents conflict".to_string());
        }
    }
    let base_revision_hlc = previous_state
        .as_ref()
        .and_then(|state| state.current_revision_hlc.clone());
    let plaintext = SyncPlaintext::from_timer_session(session, hlc.clone())
        .map_err(|_| "sync failed".to_string())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: session.id,
            collection: SyncCollection::TimerSessions,
            deleted,
            plaintext: &plaintext,
            dek,
            tenant_id: keys.tenant_id,
            generation: keys.tenant_generation,
            revision_hlc: &hlc,
            semantic_hlc: &encoded_hlc,
            base_revision_hlc,
        },
        now_ms,
    )
}

pub fn enqueue_list_sync<S, N>(
    store: &mut S,
    keys: &LocalSyncKeys,
    device_id: &str,
    list: &List,
    deleted: bool,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    ensure_no_pending_rotation(store)?;
    let hlc = tick_local_hlc(store, device_id, now_ms)?;
    let encoded_hlc = hlc.encode().map_err(|_| "sync failed".to_string())?;
    let previous_state = store.get_record_state(SyncCollection::Lists, list.id)?;
    let base_revision_hlc = previous_state
        .as_ref()
        .and_then(|state| state.current_revision_hlc.clone());
    let dek =
        tenant_root_dek(keys).ok_or_else(|| "missing Tenant Key for list sync".to_string())?;
    let generation = keys.tenant_generation;
    let plaintext = changed_list_plaintext(previous_state.as_ref(), list, hlc.clone())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: list.id,
            collection: SyncCollection::Lists,
            deleted,
            plaintext: &plaintext,
            dek,
            tenant_id: keys.tenant_id,
            generation,
            revision_hlc: &hlc,
            semantic_hlc: &encoded_hlc,
            base_revision_hlc,
        },
        now_ms,
    )
}

fn ensure_no_pending_rotation<S: LocalMutationSyncStore>(store: &mut S) -> Result<(), String> {
    if store
        .get_setting(crate::KEY_ROTATION_PENDING_METADATA_KEY)?
        .as_deref()
        .is_some_and(|value| value != "0")
    {
        return Err("active key generation required".to_string());
    }
    Ok(())
}

pub(crate) struct RebasePlaintextRequest<'a> {
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub plaintext: &'a SyncPlaintext,
    pub dek: &'a [u8; KEY_LEN],
    pub tenant_id: Uuid,
    pub generation: u64,
    pub device_id: &'a str,
    pub base_revision_hlc: &'a str,
}

pub(crate) fn enqueue_merged_plaintext<S, N>(
    store: &mut S,
    request: RebasePlaintextRequest<'_>,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let revision_hlc =
        tick_local_hlc_after(store, request.device_id, request.base_revision_hlc, now_ms)?;
    let mutation_hlc = request
        .plaintext
        .record_hlc()
        .encode()
        .map_err(|_| "sync failed".to_string())?;
    enqueue_plaintext(
        store,
        EnqueuePlaintextRequest {
            record_id: request.record_id,
            collection: request.collection,
            deleted: false,
            plaintext: request.plaintext,
            dek: request.dek,
            tenant_id: request.tenant_id,
            generation: request.generation,
            revision_hlc: &revision_hlc,
            semantic_hlc: &mutation_hlc,
            base_revision_hlc: Some(request.base_revision_hlc.to_string()),
        },
        now_ms,
    )
}

pub(crate) struct RebaseTombstoneRequest<'a> {
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub delete_hlc: &'a str,
    pub device_id: &'a str,
    pub base_revision_hlc: Option<&'a str>,
}

pub(crate) fn enqueue_rebased_tombstone<S, N>(
    store: &mut S,
    request: RebaseTombstoneRequest<'_>,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    Hlc::decode(request.delete_hlc).map_err(|_| "sync failed".to_string())?;
    let revision_base = request.base_revision_hlc.unwrap_or(request.delete_hlc);
    let revision_hlc = tick_local_hlc_after(store, request.device_id, revision_base, now_ms)?
        .encode()
        .map_err(|_| "sync failed".to_string())?;
    store.put_outbox_head(NewLocalSyncOutboxEntry {
        op_id: Uuid::now_v7(),
        record_id: request.record_id,
        collection: request.collection,
        base_revision_hlc: request.base_revision_hlc.map(str::to_string),
        revision_hlc,
        state: EncryptedSyncState::Tombstone {
            delete_hlc: request.delete_hlc.to_string(),
        },
        created_at: now_ms()?,
    })?;
    store.put_record_state(
        request.collection,
        request.record_id,
        LocalSyncRecordState {
            current_revision_hlc: request.base_revision_hlc.map(str::to_string),
            state: LocalSyncSemanticState::Tombstone {
                delete_hlc: request.delete_hlc.to_string(),
            },
        },
        now_ms()?,
    )
}

struct EnqueuePlaintextRequest<'a> {
    record_id: Uuid,
    collection: SyncCollection,
    deleted: bool,
    plaintext: &'a SyncPlaintext,
    dek: &'a [u8; KEY_LEN],
    tenant_id: Uuid,
    generation: u64,
    revision_hlc: &'a Hlc,
    semantic_hlc: &'a str,
    base_revision_hlc: Option<String>,
}

fn enqueue_plaintext<S, N>(
    store: &mut S,
    request: EnqueuePlaintextRequest<'_>,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let state = if request.deleted {
        EncryptedSyncState::Tombstone {
            delete_hlc: request.semantic_hlc.to_string(),
        }
    } else {
        let blob = encrypt_plaintext(
            request.dek,
            request.tenant_id,
            request.generation,
            request.collection.as_str(),
            request.record_id,
            request.plaintext,
        )
        .map_err(|_| "sync failed".to_string())?;
        EncryptedSyncState::Live {
            mutation_hlc: request.semantic_hlc.to_string(),
            blob,
        }
    };
    let revision_hlc = request
        .revision_hlc
        .encode()
        .map_err(|_| "sync failed".to_string())?;
    store.put_outbox_head(NewLocalSyncOutboxEntry {
        op_id: Uuid::now_v7(),
        record_id: request.record_id,
        collection: request.collection,
        base_revision_hlc: request.base_revision_hlc.clone(),
        revision_hlc,
        state,
        created_at: now_ms()?,
    })?;
    let semantic_state = if request.deleted {
        LocalSyncSemanticState::Tombstone {
            delete_hlc: request.semantic_hlc.to_string(),
        }
    } else {
        let plaintext_json =
            serde_json::to_string(request.plaintext).map_err(|_| "sync failed".to_string())?;
        LocalSyncSemanticState::Live {
            mutation_hlc: request.semantic_hlc.to_string(),
            plaintext_json,
        }
    };
    store.put_record_state(
        request.collection,
        request.record_id,
        LocalSyncRecordState {
            current_revision_hlc: request.base_revision_hlc,
            state: semantic_state,
        },
        now_ms()?,
    )
}

fn tick_local_hlc<S, N>(store: &mut S, device_id: &str, now_ms: &mut N) -> Result<Hlc, String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut clock = match store.get_setting(SYNC_LOCAL_HLC_METADATA_KEY)? {
        Some(encoded) if !encoded.is_empty() => {
            Hlc::decode(&encoded).unwrap_or_else(|_| Hlc::new(device_id.to_string()))
        }
        _ => Hlc::new(device_id.to_string()),
    };
    let hlc = clock
        .now(now_ms()?)
        .map_err(|_| "sync failed".to_string())?;
    store.set_setting(
        SYNC_LOCAL_HLC_METADATA_KEY,
        &hlc.encode().map_err(|_| "sync failed".to_string())?,
        now_ms()?,
    )?;
    Ok(hlc)
}

/// Reserves a durable HLC identity for a template Blueprint or TaskSeries config
/// revision in the transaction that persists that revision.
pub fn next_local_revision<S, N>(
    store: &mut S,
    device_id: &str,
    now_ms: &mut N,
) -> Result<String, String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    tick_local_hlc(store, device_id, now_ms)?
        .encode()
        .map_err(|_| "sync failed".to_string())
}

fn tick_local_hlc_after<S, N>(
    store: &mut S,
    device_id: &str,
    remote_revision_hlc: &str,
    now_ms: &mut N,
) -> Result<Hlc, String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut clock = match store.get_setting(SYNC_LOCAL_HLC_METADATA_KEY)? {
        Some(encoded) if !encoded.is_empty() => {
            Hlc::decode(&encoded).unwrap_or_else(|_| Hlc::new(device_id.to_string()))
        }
        _ => Hlc::new(device_id.to_string()),
    };
    let remote =
        Hlc::decode_observable(remote_revision_hlc).map_err(|_| "sync failed".to_string())?;
    let hlc = clock
        .merge(&remote, now_ms()?)
        .map_err(|_| "sync failed".to_string())?;
    store.set_setting(
        SYNC_LOCAL_HLC_METADATA_KEY,
        &hlc.encode().map_err(|_| "sync failed".to_string())?,
        now_ms()?,
    )?;
    Ok(hlc)
}

pub(crate) fn observe_remote_hlc<S, N>(
    store: &mut S,
    device_id: &str,
    remote_revision_hlc: &str,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalMutationSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut clock = match store.get_setting(SYNC_LOCAL_HLC_METADATA_KEY)? {
        Some(encoded) if !encoded.is_empty() => {
            Hlc::decode(&encoded).unwrap_or_else(|_| Hlc::new(device_id.to_string()))
        }
        _ => Hlc::new(device_id.to_string()),
    };
    let remote =
        Hlc::decode_observable(remote_revision_hlc).map_err(|_| "sync failed".to_string())?;
    let observed = clock
        .merge(&remote, now_ms()?)
        .map_err(|_| "sync failed".to_string())?;
    store.set_setting(
        SYNC_LOCAL_HLC_METADATA_KEY,
        &observed.encode().map_err(|_| "sync failed".to_string())?,
        now_ms()?,
    )
}

pub(crate) fn task_plaintext(task: &Task, hlc: Hlc) -> SyncPlaintext {
    SyncPlaintext::from_task(task, hlc).expect("domain task has a valid fixed-width rank")
}

pub(crate) fn list_plaintext(list: &List, hlc: Hlc) -> SyncPlaintext {
    SyncPlaintext::from_list(list, hlc).expect("domain list has a valid fixed-width rank")
}

pub(crate) fn template_plaintext(template: &TaskTemplate, hlc: Hlc) -> SyncPlaintext {
    SyncPlaintext::from_template(template, hlc).expect("domain template is valid")
}

pub(crate) fn task_series_plaintext(series: &TaskSeries, hlc: Hlc) -> SyncPlaintext {
    SyncPlaintext::from_series(series, hlc).expect("domain task series is valid")
}

fn changed_task_plaintext(
    previous: Option<&LocalSyncRecordState>,
    task: &Task,
    hlc: Hlc,
) -> Result<SyncPlaintext, String> {
    match previous.map(|state| &state.state) {
        Some(LocalSyncSemanticState::Live { plaintext_json, .. }) => {
            let previous: SyncPlaintext =
                serde_json::from_str(plaintext_json).map_err(|_| "sync failed".to_string())?;
            previous
                .stamp_task_changes(task, hlc)
                .map_err(|_| "sync failed".to_string())
        }
        _ => SyncPlaintext::from_task(task, hlc).map_err(|_| "sync failed".to_string()),
    }
}

fn changed_list_plaintext(
    previous: Option<&LocalSyncRecordState>,
    list: &List,
    hlc: Hlc,
) -> Result<SyncPlaintext, String> {
    match previous.map(|state| &state.state) {
        Some(LocalSyncSemanticState::Live { plaintext_json, .. }) => {
            let previous: SyncPlaintext =
                serde_json::from_str(plaintext_json).map_err(|_| "sync failed".to_string())?;
            previous
                .stamp_list_changes(list, hlc)
                .map_err(|_| "sync failed".to_string())
        }
        _ => SyncPlaintext::from_list(list, hlc).map_err(|_| "sync failed".to_string()),
    }
}

fn changed_template_plaintext(
    previous: Option<&LocalSyncRecordState>,
    template: &TaskTemplate,
    hlc: Hlc,
) -> Result<SyncPlaintext, String> {
    match previous.map(|state| &state.state) {
        Some(LocalSyncSemanticState::Live { plaintext_json, .. }) => {
            let previous: SyncPlaintext =
                serde_json::from_str(plaintext_json).map_err(|_| "sync failed".to_string())?;
            previous
                .stamp_template_changes(template, hlc)
                .map_err(|_| "sync failed".to_string())
        }
        _ => SyncPlaintext::from_template(template, hlc).map_err(|_| "sync failed".to_string()),
    }
}

fn changed_task_series_plaintext(
    previous: Option<&LocalSyncRecordState>,
    series: &TaskSeries,
    hlc: Hlc,
) -> Result<SyncPlaintext, String> {
    match previous.map(|state| &state.state) {
        Some(LocalSyncSemanticState::Live { plaintext_json, .. }) => {
            let previous: SyncPlaintext =
                serde_json::from_str(plaintext_json).map_err(|_| "sync failed".to_string())?;
            previous
                .stamp_series_changes(series, hlc)
                .map_err(|_| "sync failed".to_string())
        }
        _ => SyncPlaintext::from_series(series, hlc).map_err(|_| "sync failed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use taskveil_domain::{
        SeriesCursor, TaskBlueprint, TaskBlueprintNode, TaskContent, TaskSeriesConfig, TaskStatus,
        MAX_TASK_BLUEPRINT_BYTES, TASK_BLUEPRINT_SCHEMA_REVISION,
    };
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct FakeStore {
        outbox: Vec<LocalSyncOutboxEntry>,
        settings: HashMap<String, String>,
        record_states: HashMap<(SyncCollection, Uuid), LocalSyncRecordState>,
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

    #[test]
    fn backfill_enqueues_lists_before_tasks() {
        let list = sample_list(uuid(1), 10);
        let task = sample_task(uuid(2), list.id, 20);
        let mut store = FakeStore::default();
        let keys = sync_keys(&[list.id]);
        let mut now = ticking_now();

        let summary = enqueue_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: std::slice::from_ref(&list),
                templates: &[],
                task_series: &[],
                tasks: std::slice::from_ref(&task),
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        assert_eq!(summary.enqueued_lists, 1);
        assert_eq!(summary.enqueued_tasks, 1);
        assert_eq!(store.outbox[0].collection, SyncCollection::Lists);
        assert_eq!(store.outbox[0].record_id, list.id);
        assert_eq!(store.outbox[1].collection, SyncCollection::Tasks);
        assert_eq!(store.outbox[1].record_id, task.id);
    }

    #[test]
    fn backfill_encrypts_template_and_task_series_with_tenant_root_dek() {
        let template = sample_template(uuid(20));
        let series = sample_task_series(uuid(21), template.id);
        let mut store = FakeStore::default();
        let keys = tenant_sync_keys();
        let mut now = ticking_now();

        let summary = enqueue_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: &[],
                templates: std::slice::from_ref(&template),
                task_series: std::slice::from_ref(&series),
                tasks: &[],
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        assert_eq!(summary.enqueued_templates, 1);
        assert_eq!(summary.enqueued_task_series, 1);
        assert_eq!(store.outbox[0].collection, SyncCollection::Templates);
        assert_eq!(store.outbox[1].collection, SyncCollection::TaskSeries);
        for (entry, expected_collection) in [
            (&store.outbox[0], "templates"),
            (&store.outbox[1], "task_series"),
        ] {
            let EncryptedSyncState::Live { blob, .. } = &entry.state else {
                panic!("backfill emitted a tombstone")
            };
            let decoded = crate::decrypt_plaintext(
                &[8; KEY_LEN],
                keys.tenant_id,
                keys.tenant_generation,
                expected_collection,
                entry.record_id,
                blob,
            )
            .unwrap();
            decoded
                .validate_for_collection(expected_collection, &entry.record_id.to_string())
                .unwrap();
            assert!(crate::decrypt_plaintext(
                &[9; KEY_LEN],
                keys.tenant_id,
                keys.tenant_generation,
                expected_collection,
                entry.record_id,
                blob,
            )
            .is_err());
            let other_collection = if expected_collection == "templates" {
                "task_series"
            } else {
                "templates"
            };
            assert!(crate::decrypt_plaintext(
                &[8; KEY_LEN],
                keys.tenant_id,
                keys.tenant_generation,
                other_collection,
                entry.record_id,
                blob,
            )
            .is_err());
        }
    }

    #[test]
    fn near_limit_escape_heavy_blueprint_envelope_stays_below_transport_cap() {
        let mut low = 0usize;
        let mut high = 600usize;
        while low < high {
            let candidate = (low + high).div_ceil(2);
            if template_with_escape_note_size(uuid(22), candidate)
                .blueprint
                .validate()
                .is_ok()
            {
                low = candidate;
            } else {
                high = candidate - 1;
            }
        }
        let template = template_with_escape_note_size(uuid(22), low);
        let snapshot_bytes = template.blueprint.validate().unwrap();
        assert!(snapshot_bytes > 47 * 1024);
        assert!(snapshot_bytes <= MAX_TASK_BLUEPRINT_BYTES);
        let keys = tenant_sync_keys();
        let mut store = FakeStore::default();
        let mut now = ticking_now();

        enqueue_template_sync(&mut store, &keys, "device-a", &template, false, &mut now).unwrap();

        let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
            panic!("template enqueue emitted a tombstone")
        };
        assert!(blob.len() <= crate::MAX_ENCRYPTED_BLOB_LEN);
        assert!(crate::decrypt_plaintext(
            &[8; KEY_LEN],
            keys.tenant_id,
            keys.tenant_generation,
            "templates",
            template.id,
            blob,
        )
        .is_ok());
    }

    #[test]
    fn backfill_skips_records_that_already_have_outbox_rows() {
        let list = sample_list(uuid(3), 10);
        let task = sample_task(uuid(4), list.id, 20);
        let mut store = FakeStore::default();
        store
            .outbox
            .push(existing_outbox(SyncCollection::Tasks, task.id));
        let keys = sync_keys(&[list.id]);
        let mut now = ticking_now();

        let summary = enqueue_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: std::slice::from_ref(&list),
                templates: &[],
                task_series: &[],
                tasks: &[task],
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        assert_eq!(summary.enqueued_lists, 1);
        assert_eq!(summary.enqueued_tasks, 0);
        assert_eq!(summary.skipped_existing_outbox, 1);
        assert_eq!(store.outbox.len(), 2);
        assert_eq!(
            store
                .outbox
                .iter()
                .filter(|entry| entry.collection == SyncCollection::Tasks)
                .count(),
            1
        );
    }

    #[test]
    fn backfill_enqueues_tasks_by_created_at() {
        let list = sample_list(uuid(5), 10);
        let later = sample_task(uuid(6), list.id, 30);
        let earlier = sample_task(uuid(7), list.id, 20);
        let mut store = FakeStore::default();
        let keys = sync_keys(&[list.id]);
        let mut now = ticking_now();

        enqueue_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: &[list],
                templates: &[],
                task_series: &[],
                tasks: &[later.clone(), earlier.clone()],
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        let task_ids = store
            .outbox
            .iter()
            .filter(|entry| entry.collection == SyncCollection::Tasks)
            .map(|entry| entry.record_id)
            .collect::<Vec<_>>();
        assert_eq!(task_ids, vec![earlier.id, later.id]);
    }

    #[test]
    fn backfill_uses_one_tenant_key_for_records_from_multiple_lists() {
        let missing_list = sample_list(uuid(8), 10);
        let synced_list = sample_list(uuid(9), 11);
        let missing_task = sample_task(uuid(10), missing_list.id, 20);
        let synced_task = sample_task(uuid(11), synced_list.id, 21);
        let mut store = FakeStore::default();
        let keys = sync_keys(&[synced_list.id]);
        let mut now = ticking_now();

        let summary = enqueue_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: &[missing_list.clone(), synced_list.clone()],
                templates: &[],
                task_series: &[],
                tasks: &[missing_task.clone(), synced_task.clone()],
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        assert_eq!(summary.enqueued_lists, 2);
        assert_eq!(summary.enqueued_tasks, 2);
        assert_eq!(summary.skipped_missing_dek, 0);
        assert!(store
            .outbox
            .iter()
            .any(|entry| entry.record_id == missing_list.id));
        assert!(store
            .outbox
            .iter()
            .any(|entry| entry.record_id == missing_task.id));
        assert!(store
            .outbox
            .iter()
            .any(|entry| entry.record_id == synced_list.id));
        assert!(store
            .outbox
            .iter()
            .any(|entry| entry.record_id == synced_task.id));
    }

    #[test]
    fn rotation_backfill_replaces_live_outbox_with_new_generation_and_keeps_tombstones() {
        let list = sample_list(uuid(12), 10);
        let task = sample_task(uuid(13), list.id, 20);
        let tombstone_id = uuid(14);
        let mut store = FakeStore::default();
        store
            .outbox
            .push(existing_outbox(SyncCollection::Tasks, task.id));
        store.record_states.insert(
            (SyncCollection::Tasks, tombstone_id),
            LocalSyncRecordState {
                current_revision_hlc: Some(Hlc::new("remote").now(1).unwrap().encode().unwrap()),
                state: LocalSyncSemanticState::Tombstone {
                    delete_hlc: Hlc::new("remote").now(1).unwrap().encode().unwrap(),
                },
            },
        );
        let keys = LocalSyncKeys {
            tenant_id: Uuid::from_u128(100),
            tenant_root_dek: Some(Zeroizing::new([8; KEY_LEN])),
            tenant_generation: 2,
            historical_tenant_root_deks: Vec::new(),
        };
        let mut now = ticking_now();

        let summary = enqueue_rotation_backfill(
            &mut store,
            &keys,
            "device-a",
            BackfillRecords {
                lists: std::slice::from_ref(&list),
                templates: &[],
                task_series: &[],
                tasks: std::slice::from_ref(&task),
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();

        assert_eq!(summary.enqueued_lists, 1);
        assert_eq!(summary.enqueued_tasks, 1);
        assert_eq!(store.outbox.len(), 2);
        for entry in &store.outbox {
            let EncryptedSyncState::Live { blob, .. } = &entry.state else {
                panic!("rotation backfill emitted a tombstone")
            };
            assert_eq!(
                crate::parse_envelope_header(blob).unwrap().key_generation,
                2
            );
        }
        assert!(matches!(
            store
                .record_states
                .get(&(SyncCollection::Tasks, tombstone_id))
                .unwrap()
                .state,
            LocalSyncSemanticState::Tombstone { .. }
        ));
    }

    fn sync_keys(_list_ids: &[Uuid]) -> LocalSyncKeys {
        LocalSyncKeys {
            tenant_id: Uuid::from_u128(100),
            tenant_root_dek: Some(Zeroizing::new([7; KEY_LEN])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        }
    }

    fn tenant_sync_keys() -> LocalSyncKeys {
        LocalSyncKeys {
            tenant_id: Uuid::from_u128(100),
            tenant_root_dek: Some(Zeroizing::new([8; KEY_LEN])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        }
    }

    fn ticking_now() -> impl FnMut() -> Result<i64, String> {
        let mut now = 1_799_000_000_000;
        move || {
            now += 1;
            Ok(now)
        }
    }

    fn sample_list(id: Uuid, created_at: i64) -> List {
        List {
            id,
            name: format!("List {id}"),
            color: "#ffffff".to_string(),
            icon: "list".to_string(),
            sort_order: "7fffffffffffffffffffffffffffffff".to_string(),
            is_default: false,
            archived_at: None,
            created_at,
            updated_at: created_at,
        }
    }

    fn sample_task(id: Uuid, list_id: Uuid, created_at: i64) -> Task {
        Task {
            id,
            list_id,
            parent_task_id: None,
            content: TaskContent {
                title: format!("Task {id}"),
                note: String::new(),
                priority: 0,
                estimated_minutes: None,
            },
            status: TaskStatus::Todo,
            due: None,
            scheduled_at: None,
            sort_order: "7fffffffffffffffffffffffffffffff".to_string(),
            completed_at: None,
            closed_reason: None,
            deleted_at: None,
            assignee: None,
            series_occurrence: None,
            created_at,
            updated_at: created_at,
        }
    }

    fn sample_template(id: Uuid) -> TaskTemplate {
        TaskTemplate {
            id,
            name: "Template".to_string(),
            default_list_id: None,
            blueprint: TaskBlueprint {
                schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                nodes: vec![TaskBlueprintNode {
                    node_key: "root".to_string(),
                    parent_node_key: None,
                    sibling_order: 0,
                    content: TaskContent {
                        title: "Generated".to_string(),
                        note: String::new(),
                        priority: 0,
                        estimated_minutes: None,
                    },
                }],
            },
            blueprint_revision: "template-r1".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn sample_task_series(id: Uuid, blueprint_seed: Uuid) -> TaskSeries {
        TaskSeries {
            id,
            config: TaskSeriesConfig {
                blueprint: sample_template(blueprint_seed).blueprint,
                target_list_id: None,
                rrule: "FREQ=DAILY".to_string(),
                starts_at: 1_800_000_000_000,
                time_zone: "UTC".to_string(),
                enabled: true,
                config_revision: "series-r1".to_string(),
                config_parent_revision: None,
                config_effective_from: 1,
                lineage: Vec::new(),
            },
            cursor: SeriesCursor::Pending(1_800_000_000_000),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn template_with_escape_note_size(id: Uuid, note_size: usize) -> TaskTemplate {
        let mut template = sample_template(id);
        template.blueprint.nodes = (0..100)
            .map(|index| TaskBlueprintNode {
                node_key: format!("node-{index}"),
                parent_node_key: (index != 0).then(|| "node-0".to_string()),
                sibling_order: u32::try_from(index).unwrap(),
                content: TaskContent {
                    title: format!("Task {index}"),
                    note: "\"".repeat(note_size),
                    priority: 0,
                    estimated_minutes: None,
                },
            })
            .collect();
        template
    }

    fn existing_outbox(collection: SyncCollection, record_id: Uuid) -> LocalSyncOutboxEntry {
        LocalSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id,
            collection,
            base_revision_hlc: None,
            revision_hlc: Hlc::new("existing").now(1).unwrap().encode().unwrap(),
            state: EncryptedSyncState::Live {
                mutation_hlc: Hlc::new("existing").now(1).unwrap().encode().unwrap(),
                blob: vec![1],
            },
            created_at: 1,
        }
    }

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }
}
