use std::path::PathBuf;

#[cfg(any(test, feature = "test-support"))]
use std::path::Path;

use taskveil_domain::{
    update_due, update_estimated_minutes, update_note, update_priority, update_scheduled_at,
    update_title, CompletedTimerSession, List, Task, TaskDue, TaskSeries, TaskTemplate, Uuid,
};
use taskveil_storage::{
    open_encrypted, NewSyncOutboxEntry, SqliteWriteTx, StorageError, SyncOutboxState,
    SyncRecordSemanticState, SyncRecordState, TaskUndoOperation,
};
use taskveil_sync::{
    enqueue_list_sync, enqueue_task_series_sync, enqueue_task_sync, enqueue_template_sync,
    enqueue_timer_session_sync, next_local_revision, EncryptedSyncState, LocalMutationSyncStore,
    LocalSyncKeys, LocalSyncRecordState, LocalSyncSemanticState, NewLocalSyncOutboxEntry,
    SyncCollection,
};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("storage operation failed: {0}")]
    Storage(StorageError),
    #[error("domain operation failed: {0}")]
    Domain(#[from] taskveil_domain::DomainError),
    #[error("recurrence operation failed: {0}")]
    Recurrence(#[from] taskveil_domain::RecurrenceError),
    #[error("local sync preparation failed")]
    Sync,
    #[error("profile I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("profile secret store failed")]
    KeyStore(#[source] taskveil_crypto::KeyStoreError),
    #[error("account request failed")]
    AccountRequest,
    #[error("remote account credential is unavailable")]
    CredentialUnavailable,
    #[error("a Pro entitlement is required")]
    EntitlementRequired,
    #[error("account-bound local sync keys are unavailable")]
    AccountBoundUnavailable,
    #[error("local profile is already account-bound")]
    ProfileAlreadyBound,
    #[error("local profile belongs to a different account")]
    ProfileIdentityMismatch,
    #[error("account state is incomplete")]
    IncompleteAccountState,
    #[error("sync failed")]
    SyncRun,
    #[error("upgrade required")]
    UpgradeRequired,
    #[error("local HLC is temporarily ahead of the server clock")]
    ClockSkewRetryable,
    #[error("client runtime state is unavailable")]
    RuntimeState,
    #[error("another account or sync operation is already running")]
    Busy,
    #[error("another operation is using the local profile with an incompatible lock")]
    ProfileBusy,
    #[error("operating-system profile locks are unsupported")]
    ProfileLockUnsupported,
    #[error("another process owns the local profile sync lease")]
    SyncLeaseBusy,
    #[error("the local profile sync lease was lost")]
    LeaseLost,
    #[error("the local profile database is busy")]
    DatabaseBusy,
    #[error("task priority must be between 0 and 3")]
    InvalidPriority,
    #[error("estimated minutes must be a positive multiple of 5")]
    InvalidEstimatedMinutes,
    #[error("local IANA time zone is unavailable")]
    LocalTimeZoneUnavailable,
    #[error("local key capsule state is invalid")]
    LocalKeyState,
    #[error("local key rotation could not recover either database generation")]
    LocalKeyRecoveryFailed,
    #[cfg(test)]
    #[error("injected device key rotation failure")]
    InjectedDeviceKeyRotationFailure,
    #[error("another active timer already exists: {0}")]
    ActiveTimerConflict(Uuid),
    #[error("calendar range must contain increasing civil-date and instant bounds")]
    InvalidCalendarRange,
}

impl From<StorageError> for ClientError {
    fn from(error: StorageError) -> Self {
        if error.is_database_busy() {
            Self::DatabaseBusy
        } else {
            Self::Storage(error)
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalMutationContext {
    pub device_id: String,
    pub keys: LocalSyncKeys,
}

#[derive(Debug, Clone)]
pub struct UpdateTaskInput {
    pub task_id: Uuid,
    pub title: String,
    pub note: String,
    pub priority: i32,
    pub due: Option<TaskDue>,
    pub scheduled_at: Option<i64>,
    pub estimated_minutes: Option<i32>,
    pub now_ms: i64,
}

/// Low-level SQLite mutation primitive exposed only through `test-support`.
///
/// Product frontends use `TaskveilClient`; this type exists for transactional
/// integration tests that must exercise mutation and outbox atomicity.
pub struct SqliteMutationService {
    pub(crate) db_path: PathBuf,
    pub(crate) db_key: Zeroizing<[u8; 32]>,
    pub(crate) expected_runtime_epoch: Option<i64>,
}

impl SqliteMutationService {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(db_path: impl Into<PathBuf>, db_key: [u8; 32]) -> Self {
        Self::new_secret(db_path, Zeroizing::new(db_key))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn new_secret(db_path: impl Into<PathBuf>, db_key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            db_path: db_path.into(),
            db_key,
            expected_runtime_epoch: None,
        }
    }

    pub(crate) fn new_secret_with_epoch(
        db_path: impl Into<PathBuf>,
        db_key: Zeroizing<[u8; 32]>,
        expected_runtime_epoch: i64,
    ) -> Self {
        Self {
            db_path: db_path.into(),
            db_key,
            expected_runtime_epoch: Some(expected_runtime_epoch),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn update_task(
        &self,
        input: UpdateTaskInput,
        sync: &LocalMutationContext,
    ) -> Result<Task, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let before = transaction.get_task(input.task_id)?;
        if sync.keys.tenant_root_dek.is_none()
            || sync.keys.tenant_id.is_nil()
            || sync.keys.tenant_generation == 0
        {
            return Err(ClientError::Sync);
        }
        let task = update_title(before.clone(), input.title, input.now_ms)?;
        let task = update_note(task, input.note, input.now_ms)?;
        let task = update_priority(task, input.priority, input.now_ms)?;
        let task = update_due(task, input.due, input.now_ms)?;
        let task = update_scheduled_at(task, input.scheduled_at, input.now_ms)?;
        let updated = update_estimated_minutes(task, input.estimated_minutes, input.now_ms)?;

        transaction.update_with_undo(
            before,
            updated.clone(),
            TaskUndoOperation::Edit,
            input.now_ms,
        )?;
        enqueue_task_in_transaction(&mut transaction, sync, &updated, false, input.now_ms)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub(crate) fn assert_runtime_epoch(
        &self,
        transaction: &SqliteWriteTx<'_>,
    ) -> Result<(), ClientError> {
        if let Some(expected) = self.expected_runtime_epoch {
            transaction.assert_profile_runtime_epoch(expected)?;
        }
        Ok(())
    }
}

pub(crate) fn enqueue_task_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    sync: &LocalMutationContext,
    task: &Task,
    deleted: bool,
    now_ms: i64,
) -> Result<(), ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    enqueue_task_sync(
        &mut store,
        &sync.keys,
        &sync.device_id,
        task,
        deleted,
        &mut now,
    )
    .map_err(map_mutation_sync_error)
}

pub(crate) fn enqueue_list_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    sync: &LocalMutationContext,
    list: &List,
    deleted: bool,
    now_ms: i64,
) -> Result<(), ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    enqueue_list_sync(
        &mut store,
        &sync.keys,
        &sync.device_id,
        list,
        deleted,
        &mut now,
    )
    .map_err(map_mutation_sync_error)
}

pub(crate) fn enqueue_timer_session_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    sync: &LocalMutationContext,
    session: &CompletedTimerSession,
    deleted: bool,
    now_ms: i64,
) -> Result<(), ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    enqueue_timer_session_sync(
        &mut store,
        &sync.keys,
        &sync.device_id,
        session,
        deleted,
        &mut now,
    )
    .map_err(map_mutation_sync_error)
}

pub(crate) fn next_revision_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    device_id: &str,
    now_ms: i64,
) -> Result<String, ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    next_local_revision(&mut store, device_id, &mut now).map_err(map_mutation_sync_error)
}

pub(crate) fn enqueue_template_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    sync: &LocalMutationContext,
    template: &TaskTemplate,
    deleted: bool,
    now_ms: i64,
) -> Result<(), ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    enqueue_template_sync(
        &mut store,
        &sync.keys,
        &sync.device_id,
        template,
        deleted,
        &mut now,
    )
    .map_err(map_mutation_sync_error)
}

pub(crate) fn enqueue_task_series_in_transaction(
    transaction: &mut SqliteWriteTx<'_>,
    sync: &LocalMutationContext,
    schedule: &TaskSeries,
    deleted: bool,
    now_ms: i64,
) -> Result<(), ClientError> {
    let mut store = TransactionalMutationStore { transaction };
    let mut now = || Ok(now_ms);
    enqueue_task_series_sync(
        &mut store,
        &sync.keys,
        &sync.device_id,
        schedule,
        deleted,
        &mut now,
    )
    .map_err(map_mutation_sync_error)
}

fn mutation_storage_error(error: StorageError) -> String {
    if error.is_database_busy() {
        "database busy".to_string()
    } else {
        error.to_string()
    }
}

fn map_mutation_sync_error(error: String) -> ClientError {
    if error == "database busy" {
        ClientError::DatabaseBusy
    } else {
        ClientError::Sync
    }
}

struct TransactionalMutationStore<'transaction, 'connection> {
    transaction: &'transaction mut SqliteWriteTx<'connection>,
}

impl LocalMutationSyncStore for TransactionalMutationStore<'_, '_> {
    fn has_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        self.transaction
            .has_outbox_head(collection.as_str(), record_id)
            .map_err(mutation_storage_error)
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        self.transaction
            .get_setting(key)
            .map_err(mutation_storage_error)
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String> {
        self.transaction
            .set_setting(key, value, updated_at)
            .map_err(mutation_storage_error)
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        self.transaction
            .put_outbox_head(NewSyncOutboxEntry {
                op_id: entry.op_id,
                record_id: entry.record_id,
                collection: entry.collection.to_string(),
                base_revision_hlc: entry.base_revision_hlc,
                revision_hlc: entry.revision_hlc,
                state: match entry.state {
                    EncryptedSyncState::Live { mutation_hlc, blob } => {
                        SyncOutboxState::Live { mutation_hlc, blob }
                    }
                    EncryptedSyncState::Tombstone { delete_hlc } => {
                        SyncOutboxState::Tombstone { delete_hlc }
                    }
                },
                created_at: entry.created_at,
            })
            .map(|_| ())
            .map_err(mutation_storage_error)
    }

    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String> {
        self.transaction
            .get_record_state(collection.as_str(), record_id)
            .map(|state| state.map(storage_record_to_local))
            .map_err(mutation_storage_error)
    }

    fn put_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
        state: LocalSyncRecordState,
        updated_at: i64,
    ) -> Result<(), String> {
        self.transaction
            .put_record_state(local_record_to_storage(
                collection, record_id, state, updated_at,
            ))
            .map_err(mutation_storage_error)
    }
}

fn storage_record_to_local(state: SyncRecordState) -> LocalSyncRecordState {
    LocalSyncRecordState {
        current_revision_hlc: state.current_revision_hlc,
        state: match state.state {
            SyncRecordSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            } => LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
            SyncRecordSemanticState::Tombstone { delete_hlc } => {
                LocalSyncSemanticState::Tombstone { delete_hlc }
            }
        },
    }
}

fn local_record_to_storage(
    collection: SyncCollection,
    record_id: Uuid,
    state: LocalSyncRecordState,
    updated_at: i64,
) -> SyncRecordState {
    SyncRecordState {
        record_id,
        collection: collection.to_string(),
        current_revision_hlc: state.current_revision_hlc,
        state: match state.state {
            LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            } => SyncRecordSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
            LocalSyncSemanticState::Tombstone { delete_hlc } => {
                SyncRecordSemanticState::Tombstone { delete_hlc }
            }
        },
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use taskveil_domain::{new_list, new_task};
    use taskveil_storage::{
        ListRepository, SettingsRepository, SqliteListRepository, SqliteSettingsRepository,
        SqliteSyncStateRepository, SqliteTaskRepository, SyncRecordSemanticState, SyncRecordState,
        SyncStateRepository, TaskRepository,
    };
    use taskveil_sync::{
        Hlc, LocalSyncKeys, SyncPlaintext, SYNC_LOCAL_HLC_SETTING_KEY, TASKS_COLLECTION,
    };
    use tempfile::TempDir;

    use super::*;

    const DB_KEY: [u8; 32] = [0x83; 32];
    const BASE_MS: i64 = 1_799_000_000_000;

    #[test]
    fn mutation_sync_boundary_preserves_database_busy() {
        assert!(matches!(
            map_mutation_sync_error("database busy".to_string()),
            ClientError::DatabaseBusy
        ));
        assert!(matches!(
            map_mutation_sync_error("sync failed".to_string()),
            ClientError::Sync
        ));
    }

    struct Fixture {
        _temp_dir: TempDir,
        mutation_service: SqliteMutationService,
        task: Task,
        sync: LocalMutationContext,
    }

    fn fixture() -> Fixture {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("client.sqlite3");
        let list = new_list(
            "Inbox".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let task = new_task(
            list.id,
            None,
            "before".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(list.clone()).unwrap();
        drop(lists);
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.insert(task.clone()).unwrap();
        drop(tasks);
        let baseline_clock = Hlc {
            wall_ms: BASE_MS - 1_000,
            counter: 0,
            device_id: "device-a".to_string(),
        };
        let baseline_hlc = baseline_clock.encode().unwrap();
        let baseline_plaintext =
            serde_json::to_string(&SyncPlaintext::from_task(&task, baseline_clock).unwrap())
                .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        let mut settings = SqliteSettingsRepository::new(connection);
        settings
            .set_setting(SYNC_LOCAL_HLC_SETTING_KEY, &baseline_hlc, BASE_MS)
            .unwrap();
        drop(settings);
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        let mut sync_state = SqliteSyncStateRepository::new(connection);
        sync_state
            .put_record_state(SyncRecordState {
                record_id: task.id,
                collection: TASKS_COLLECTION.to_string(),
                current_revision_hlc: None,
                state: SyncRecordSemanticState::Live {
                    mutation_hlc: baseline_hlc,
                    plaintext_json: baseline_plaintext,
                },
                updated_at: BASE_MS,
            })
            .unwrap();

        Fixture {
            _temp_dir: temp_dir,
            mutation_service: SqliteMutationService::new(db_path, DB_KEY),
            task,
            sync: LocalMutationContext {
                device_id: "device-a".to_string(),
                keys: LocalSyncKeys {
                    tenant_id: Uuid::from_u128(100),
                    tenant_root_dek: Some(Zeroizing::new([0x44; 32])),
                    tenant_generation: 1,
                    historical_tenant_root_deks: Vec::new(),
                },
            },
        }
    }

    fn update_input(task_id: Uuid) -> UpdateTaskInput {
        UpdateTaskInput {
            task_id,
            title: "after".to_string(),
            note: "atomic".to_string(),
            priority: 2,
            due: Some(TaskDue::date_time(BASE_MS + 60_000, "UTC").unwrap()),
            scheduled_at: Some(BASE_MS + 30_000),
            estimated_minutes: Some(45),
            now_ms: BASE_MS + 1_000,
        }
    }

    #[test]
    fn task_update_commits_domain_undo_hlc_outbox_and_record_state_together() {
        let fixture = fixture();
        let updated = fixture
            .mutation_service
            .update_task(update_input(fixture.task.id), &fixture.sync)
            .unwrap();
        assert_eq!(updated.content.title, "after");
        assert_eq!(updated.content.priority, 2);
        assert_eq!(updated.scheduled_at, Some(BASE_MS + 30_000));
        assert_eq!(updated.content.estimated_minutes, Some(45));

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(fixture.task.id).unwrap().content.title, "after");
        assert!(tasks.latest_unconsumed_undo().unwrap().is_some());
        drop(tasks);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let settings = SqliteSettingsRepository::new(connection);
        assert!(settings
            .get_setting(SYNC_LOCAL_HLC_SETTING_KEY)
            .unwrap()
            .is_some());
        drop(settings);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert_eq!(sync.list_outbox_heads(10).unwrap().len(), 1);
        let state = sync
            .get_record_state(TASKS_COLLECTION, fixture.task.id)
            .unwrap()
            .unwrap();
        let SyncRecordSemanticState::Live { plaintext_json, .. } = state.state else {
            panic!("expected live task state");
        };
        let SyncPlaintext::Task(plaintext) = serde_json::from_str(&plaintext_json).unwrap() else {
            panic!("expected task plaintext");
        };
        assert_eq!(plaintext.priority.value, 2);
        assert_eq!(plaintext.scheduled_at.value, Some(BASE_MS + 30_000));
        assert_eq!(plaintext.estimated_minutes.value, Some(45));
    }

    #[test]
    fn outbox_failure_rolls_back_domain_undo_hlc_and_record_state() {
        assert_atomic_rollback(
            "CREATE TRIGGER fail_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT, 'fail outbox'); END;",
        );
    }

    #[test]
    fn record_state_failure_rolls_back_domain_undo_hlc_and_outbox() {
        assert_atomic_rollback(
            "CREATE TRIGGER fail_state BEFORE UPDATE ON sync_record_states BEGIN SELECT RAISE(ABORT, 'fail state'); END;",
        );
    }

    fn assert_atomic_rollback(trigger_sql: &str) {
        let fixture = fixture();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        connection.execute_batch(trigger_sql).unwrap();
        drop(connection);

        assert!(fixture
            .mutation_service
            .update_task(update_input(fixture.task.id), &fixture.sync)
            .is_err());

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(fixture.task.id).unwrap().content.title, "before");
        assert!(tasks.latest_unconsumed_undo().unwrap().is_none());
        drop(tasks);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let settings = SqliteSettingsRepository::new(connection);
        let stored_hlc = settings
            .get_setting(SYNC_LOCAL_HLC_SETTING_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(Hlc::decode(&stored_hlc).unwrap().wall_ms, BASE_MS - 1_000);
        drop(settings);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert!(sync.list_outbox_heads(10).unwrap().is_empty());
        let state = sync
            .get_record_state(TASKS_COLLECTION, fixture.task.id)
            .unwrap()
            .unwrap();
        let SyncRecordSemanticState::Live { plaintext_json, .. } = state.state else {
            panic!("expected live state");
        };
        let SyncPlaintext::Task(plaintext) = serde_json::from_str(&plaintext_json).unwrap() else {
            panic!("expected task plaintext");
        };
        assert_eq!(plaintext.title.value, "before");
    }
}
