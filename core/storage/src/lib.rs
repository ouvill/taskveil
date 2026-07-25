//! `taskveil-storage`: ローカルストレージアクセス層。
//!
//! SQLCipherで暗号化されたSQLite上に `ListRepository` / `TaskRepository` を実装する
//! （`docs/03_技術仕様書.md` §5）。

use std::{collections::HashSet, path::Path, str::FromStr, time::Duration};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use taskveil_domain::{
    fractional_index_after, new_default_list, restored_active_duration_ms,
    validate_active_timer_session, validate_active_timer_update, validate_completed_timer_session,
    ActiveTimerSession, CivilDate, CompletedTimerSession, DomainError, IanaTimeZone, List,
    RecurrenceError, SeriesCursor, SeriesOccurrenceRef, Task, TaskContent, TaskDue, TaskSeries,
    TaskSeriesConfig, TaskStatus, TaskTemplate, TimerFinishKind, TimerMode, TimerPhase,
    TimerRunState, UtcInstant, Uuid,
};
use thiserror::Error;

mod migrations;

#[cfg(test)]
use migrations::{
    add_lists_archived_at, add_lists_is_default, add_local_crypto_cache, add_performance_indexes,
    add_reminders, add_settings, add_sync_outbox_and_cursors, add_sync_record_states,
    apply_pending_migrations, rebuild_tasks_fts_triggers, set_user_version, table_columns,
    table_columns_raw, Migration, BASELINE_SCHEMA_VERSION, SCHEMA,
};
use migrations::{ensure_schema, read_user_version, MIGRATIONS};

pub const LATEST_SCHEMA_VERSION: i32 = 22;
const LOCAL_DB_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("record not found: {0}")]
    NotFound(Uuid),
    #[error("reminder time must be in the future")]
    ReminderTimeNotFuture,
    #[error("a reminder already exists at that time")]
    DuplicateReminderTime,
    #[error("a task can have at most {limit} reminders")]
    ReminderLimitReached { limit: usize },
    #[error("closed tasks cannot schedule or snooze reminders")]
    ReminderTaskClosed,
    #[error("another active timer already exists: {0}")]
    ActiveTimerConflict(Uuid),
    #[error("invalid active timer update: {0}")]
    InvalidActiveTimerUpdate(#[source] DomainError),
    #[error(
        "completed timer duration does not match durable active state: expected {expected_ms}, got {actual_ms}"
    )]
    CompletedTimerDurationMismatch { expected_ms: i64, actual_ms: i64 },
    #[error("invalid task status in database: {0}")]
    InvalidStatus(String),
    #[error("invalid undo operation in database: {0}")]
    InvalidUndoOperation(String),
    #[error("invalid sync state in database: {0}")]
    InvalidSyncState(String),
    #[error("invalid sync collection: {0}")]
    InvalidSyncCollection(String),
    #[error(
        "sync record {record_id} belongs to collection {existing}, not requested collection {requested}"
    )]
    SyncCollectionMismatch {
        record_id: Uuid,
        existing: String,
        requested: String,
    },
    #[error("invalid uuid in database: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("invalid task snapshot in database: {0}")]
    InvalidTaskSnapshot(#[from] serde_json::Error),
    #[error("invalid template or recurrence data: {0}")]
    InvalidRecurrence(#[from] RecurrenceError),
    #[error("undo entry already used: {0}")]
    UndoConsumed(Uuid),
    #[error("task changed after undo was created: {0}")]
    UndoConflict(Uuid),
    #[error("default list cannot be {operation}: {list_id}")]
    DefaultListProtected {
        operation: &'static str,
        list_id: Uuid,
    },
    #[error("database cannot be read with the provided SQLCipher key")]
    InvalidDatabaseKey,
    #[error("unsupported database schema version: found {found}, latest supported {latest}")]
    UnsupportedSchemaVersion { found: i32, latest: i32 },
    #[error("incompatible database schema: {0}")]
    IncompatibleSchema(String),
    #[error(
        "local profile is bound to tenant {bound_tenant_id}, not requested tenant {requested_tenant_id}"
    )]
    LocalProfileTenantMismatch {
        bound_tenant_id: Uuid,
        requested_tenant_id: Uuid,
    },
    #[error(
        "local profile is bound to user {bound_user_id}, not requested user {requested_user_id}"
    )]
    LocalProfileUserMismatch {
        bound_user_id: Uuid,
        requested_user_id: Uuid,
    },
    #[error("local crypto cache contains entries for a different tenant")]
    LocalCryptoCacheTenantMismatch,
    #[error(
        "failed to migrate database schema to version {target_version} ({migration}): {source}"
    )]
    MigrationFailed {
        target_version: i32,
        migration: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Undo対象のタスク操作種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskUndoOperation {
    Delete,
    Complete,
    Edit,
}

/// ローカル専用のタスクUndo履歴。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskUndoEntry {
    pub id: Uuid,
    pub operation_type: TaskUndoOperation,
    pub task_id: Uuid,
    pub list_id: Uuid,
    pub before_snapshot: Task,
    pub after_updated_at: i64,
    pub after_deleted_at: Option<i64>,
    pub after_completed_at: Option<i64>,
    pub created_at: i64,
    pub consumed_at: Option<i64>,
}

/// A task returned by the cross-list Home smart view, annotated with its
/// containing list name for UI context.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeTask {
    pub task: Task,
    pub list_name: String,
    pub is_home_target: bool,
}

/// Viewer-local calendar bounds represented without collapsing civil dates
/// into synthetic instants. Both dimensions use half-open `[start, end)`
/// intervals and are constructed by the caller from the same viewer timezone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRange {
    start_on: CivilDate,
    end_on: CivilDate,
    start_at: UtcInstant,
    end_at: UtcInstant,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CalendarRangeError {
    #[error("calendar civil-date range must be non-empty and increasing")]
    InvalidCivilDateRange,
    #[error("calendar instant range must be non-empty and increasing")]
    InvalidInstantRange,
}

impl CalendarRange {
    pub fn new(
        start_on: CivilDate,
        end_on: CivilDate,
        start_at: UtcInstant,
        end_at: UtcInstant,
    ) -> Result<Self, CalendarRangeError> {
        if start_on >= end_on {
            return Err(CalendarRangeError::InvalidCivilDateRange);
        }
        if start_at >= end_at {
            return Err(CalendarRangeError::InvalidInstantRange);
        }
        Ok(Self {
            start_on,
            end_on,
            start_at,
            end_at,
        })
    }

    pub fn start_on(&self) -> &CivilDate {
        &self.start_on
    }

    pub fn end_on(&self) -> &CivilDate {
        &self.end_on
    }

    pub fn start_at(&self) -> UtcInstant {
        self.start_at
    }

    pub fn end_at(&self) -> UtcInstant {
        self.end_at
    }
}

/// The semantic reason a task appears in a calendar range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarOccurrenceKind {
    DateDue {
        due_on: CivilDate,
    },
    DateTimeDue {
        due_at: UtcInstant,
        time_zone: IanaTimeZone,
    },
    Scheduled {
        scheduled_at: UtcInstant,
    },
    Completed {
        completed_at: UtcInstant,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarOccurrence {
    pub task: Task,
    pub list_name: String,
    pub list_archived: bool,
    pub kind: CalendarOccurrenceKind,
}

/// A local reminder scheduled on the device for a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub id: Uuid,
    pub task_id: Uuid,
    pub remind_at: i64,
    pub snoozed_until: Option<i64>,
    pub created_at: i64,
}

pub const MAX_REMINDERS_PER_TASK: usize = 5;

/// 未ACKのrecord headに保持する暗号化済みsemantic state。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutboxState {
    Live { mutation_hlc: String, blob: Vec<u8> },
    Tombstone { delete_hlc: String },
}

/// recordごとにcoalesceされた未ACKのpush head。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: String,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: String,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub created_at: i64,
}

/// 復号・mergeに使うlocal semantic state。tombstoneは平文を保持しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncRecordSemanticState {
    Live {
        mutation_hlc: String,
        plaintext_json: String,
    },
    Tombstone {
        delete_hlc: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRecordState {
    pub record_id: Uuid,
    pub collection: String,
    pub current_revision_hlc: Option<String>,
    pub state: SyncRecordSemanticState,
    pub updated_at: i64,
}

/// Device-local mapping from a non-canonical Inbox record to the current
/// canonical Inbox. Both list sync records remain independently durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListAlias {
    pub alias_list_id: Uuid,
    pub canonical_list_id: Uuid,
    pub updated_at: i64,
}

/// テナントDB内のpull cursor。ローカルDBはテナントごとに分離する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursor {
    pub name: String,
    pub seq: i64,
    pub updated_at: i64,
}

/// Durable phase of a fuzzy-scan full resync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullResyncPhase {
    Base,
    Delta,
    Sweep,
}

/// Stable-key cursor used by the current-state base scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullResyncStableCursor {
    pub collection: String,
    pub record_id: Uuid,
}

/// Crash-recoverable progress for the one active full resync generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullResyncProgress {
    pub generation_id: Uuid,
    pub continuity_generation: i64,
    pub phase: FullResyncPhase,
    pub base_seq: i64,
    pub base_cursor: Option<FullResyncStableCursor>,
    pub delta_cursor: i64,
    pub closure_high_water: Option<i64>,
    pub sweep_cursor: Option<FullResyncStableCursor>,
    pub started_at: i64,
    pub updated_at: i64,
}

/// Rows removed when a closed generation is finalized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FullResyncSweepSummary {
    pub scanned_records: usize,
    pub swept_lists: usize,
    pub swept_tasks: usize,
    pub swept_templates: usize,
    pub swept_task_series: usize,
    pub swept_timer_sessions: usize,
    pub swept_record_states: usize,
}

/// An encrypted remote head that could not yet be safely applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncQuarantineEntry {
    pub record_id: Uuid,
    pub collection: String,
    pub seq: i64,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub reason: String,
    pub required_list_id: Option<Uuid>,
    pub first_failed_at: i64,
    pub last_failed_at: i64,
    pub attempt_count: i64,
}

/// SQLCipher内に保持するaccount-bound local profile identity。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProfileBinding {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub bound_at: i64,
    pub updated_at: i64,
}

/// Master Keyでlocal-wrap済みのTenant Root DEK cache。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTenantRootKeyBundle {
    pub tenant_id: Uuid,
    pub generation: u64,
    pub wrapped_tenant_root_dek: Vec<u8>,
    pub updated_at: i64,
}

/// account bindingとMK-wrapped Tenant Root Key cacheの永続化を担うリポジトリ。
pub trait LocalCryptoRepository {
    fn load_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError>;
    fn bind_tenant_root(
        &mut self,
        binding: LocalProfileBinding,
        tenant_root: &LocalTenantRootKeyBundle,
    ) -> Result<(), StorageError>;
    fn load_tenant_root(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<LocalTenantRootKeyBundle>, StorageError>;
}

/// タスクの永続化を担うリポジトリ。
///
/// SQLite(SQLCipher)実装は [`SqliteTaskRepository`] を参照。同期シグネチャのみを定義する。
pub trait TaskRepository {
    fn get(&self, id: Uuid) -> Result<Task, StorageError>;
    fn insert(&mut self, task: Task) -> Result<(), StorageError>;
    fn update(&mut self, task: Task) -> Result<(), StorageError>;
    fn list_all_for_sync(&self) -> Result<Vec<Task>, StorageError>;
    fn list_active_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError>;
    fn list_home(
        &self,
        today_start_ms: i64,
        tomorrow_start_ms: i64,
    ) -> Result<Vec<HomeTask>, StorageError>;
    fn list_calendar_occurrences(
        &self,
        range: &CalendarRange,
    ) -> Result<Vec<CalendarOccurrence>, StorageError>;
    fn search_tasks(&self, query: &str) -> Result<Vec<Task>, StorageError>;
    fn count_descendants(&self, task_id: Uuid) -> Result<usize, StorageError>;
    fn delete_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError>;
}

/// Tenant-scoped templates and independent Task Series.
pub trait TemplateSeriesRepository {
    fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError>;
    fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError>;
    fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError>;
    fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError>;
    fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError>;
    fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError>;
    fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError>;
    fn upsert_series(&mut self, series: TaskSeries) -> Result<(), StorageError>;
    fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError>;
}

/// Device-local active Timer and immutable completed work sessions.
pub trait TimerSessionRepository {
    fn load_active(&self) -> Result<Option<ActiveTimerSession>, StorageError>;
    fn start_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError>;
    fn update_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError>;
    fn clear_active(&mut self, expected_session_id: Uuid) -> Result<bool, StorageError>;
    fn clear_active_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError>;
    fn get_completed(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError>;
    /// Returns false for an exact immutable replay and rejects differing data.
    fn insert_completed(&mut self, session: CompletedTimerSession) -> Result<bool, StorageError>;
    fn list_completed(&self) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn list_completed_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn list_completed_by_list(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn delete_completed(&mut self, id: Uuid) -> Result<bool, StorageError>;
}

/// リストの永続化を担うリポジトリ。
///
/// SQLite(SQLCipher)実装は [`SqliteListRepository`] を参照。
pub trait ListRepository {
    fn get(&self, id: Uuid) -> Result<List, StorageError>;
    fn insert(&mut self, list: List) -> Result<(), StorageError>;
    fn update(&mut self, list: List) -> Result<(), StorageError>;
    fn list_all(&self) -> Result<Vec<List>, StorageError>;
    fn list_archived(&self) -> Result<Vec<List>, StorageError>;
    fn get_default(&self) -> Result<Option<List>, StorageError>;
    fn ensure_default_list(&mut self, name: String, now_ms: i64) -> Result<List, StorageError>;
    fn count_tasks(&self, list_id: Uuid) -> Result<usize, StorageError>;
    fn delete_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError>;
}

/// 設定値の永続化を担うリポジトリ。
///
/// 値はSQLCipher暗号化DB内に保存し、キーごとの最新値だけを保持する。
pub trait SettingsRepository {
    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError>;
    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), StorageError>;
}

/// リマインダーの永続化を担うリポジトリ。
pub trait ReminderRepository {
    fn create_task_reminder(
        &mut self,
        task_id: Uuid,
        remind_at: i64,
        created_at: i64,
    ) -> Result<Reminder, StorageError>;
    fn update_reminder(
        &mut self,
        reminder_id: Uuid,
        remind_at: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError>;
    fn delete_reminder(&mut self, reminder_id: Uuid) -> Result<Reminder, StorageError>;
    fn clear_task_reminders(&mut self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_task_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_task_subtree_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_list_reminders(&self, list_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_pending_reminders(&self, now_ms: i64) -> Result<Vec<Reminder>, StorageError>;
    fn snooze_reminder(
        &mut self,
        reminder_id: Uuid,
        snoozed_until: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError>;
}

/// 同期outboxとpull cursorの永続化を担うリポジトリ。
pub trait SyncStateRepository {
    fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError>;
    fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError>;
    fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError>;
    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError>;
    fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError>;
    fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError>;
    fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError>;
    fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError>;
    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), StorageError>;
    fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError>;
    fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError>;
    fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError>;
    fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError>;
    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError>;
}

/// A short-lived SQLite write transaction shared by domain and sync-state writes.
///
/// The transaction starts with [`TransactionBehavior::Immediate`] so concurrent
/// desktop frontends serialize before reading and incrementing the local HLC.
/// Dropping this value without calling [`Self::commit`] rolls back every write.
pub struct SqliteWriteTx<'connection> {
    transaction: Transaction<'connection>,
}

impl<'connection> SqliteWriteTx<'connection> {
    pub fn begin(connection: &'connection mut Connection) -> Result<Self, StorageError> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(Self { transaction })
    }

    pub fn get_task(&self, id: Uuid) -> Result<Task, StorageError> {
        get_task_on(&self.transaction, id)
    }

    pub fn get_list(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(&self.transaction, self.resolve_list_alias(id)?)
    }

    pub fn default_list_id(&self) -> Result<Option<Uuid>, StorageError> {
        get_default_list_on(&self.transaction).map(|list| list.map(|list| list.id))
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(&self.transaction, list_id)
    }

    pub fn list_active_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(&self.transaction, self.resolve_list_alias(list_id)?)
    }

    /// Snapshots every task rooted at `task_id` inside this write transaction.
    ///
    /// Callers can use the returned domain records to prepare per-record sync
    /// tombstones before deleting the same subtree and committing atomically.
    pub fn list_task_subtree(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(&self.transaction, task_id)
    }

    /// Snapshots every task stored in `list_id` inside this write transaction.
    ///
    /// This intentionally includes records regardless of `deleted_at`: a
    /// physical list deletion must account for every persisted task record.
    pub fn list_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(&self.transaction, self.resolve_list_alias(list_id)?)
    }

    pub fn list_all_tasks_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(&self.transaction)
    }

    pub fn get_timer_session(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(&self.transaction, id)
    }

    pub fn start_active_timer_session(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        start_active_timer_session_on(&self.transaction, session, updated_at)
    }

    pub fn update_active_timer_session(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        update_active_timer_session_on(&self.transaction, session, updated_at)
    }

    pub fn list_timer_sessions_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.transaction,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    pub fn insert_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(&self.transaction, session)
    }

    pub fn finish_active_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        let active = self
            .transaction
            .query_row(
                "SELECT session_id, task_id, mode, phase, state, started_at,
                        last_resumed_at, accumulated_active_ms, target_duration_ms
                 FROM active_timer_session WHERE singleton = 1",
                [],
                row_to_active_timer_session,
            )
            .optional()?
            .transpose()?
            .ok_or_else(|| {
                StorageError::IncompatibleSchema("active timer session is missing".to_string())
            })?;
        if active.session_id != session.id
            || active.task_id != Some(session.task_id)
            || active.mode != session.mode
            || active.phase != TimerPhase::Work
            || active.started_at != session.started_at
        {
            return Err(StorageError::IncompatibleSchema(
                "completed timer session does not match active work".to_string(),
            ));
        }
        let expected_duration = restored_active_duration_ms(&active, session.ended_at)
            .map_err(StorageError::InvalidActiveTimerUpdate)?;
        if session.active_duration_ms != expected_duration {
            return Err(StorageError::CompletedTimerDurationMismatch {
                expected_ms: expected_duration,
                actual_ms: session.active_duration_ms,
            });
        }
        let inserted = insert_completed_timer_session_on(&self.transaction, session)?;
        self.transaction
            .execute("DELETE FROM active_timer_session WHERE singleton = 1", [])?;
        Ok(inserted)
    }

    pub fn delete_timer_session(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .transaction
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }

    pub fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.transaction.execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    pub fn list_lists_including_archived(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.transaction.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE NOT EXISTS (
                 SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
             )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(lists)
    }

    pub fn insert_task(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = self.resolve_list_alias(task.list_id)?;
        insert_task_on(&self.transaction, &task)
    }

    pub fn insert_list(&mut self, list: List) -> Result<(), StorageError> {
        insert_list_on(&self.transaction, &list)
    }

    pub fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(&self.transaction, id)
    }

    pub fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(&self.transaction)
    }

    pub fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.transaction, &template)
    }

    pub fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(&self.transaction, id)
    }

    pub fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(&self.transaction, id)
    }

    pub fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(&self.transaction)
    }

    pub fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(&self.transaction, now_ms)
    }

    pub fn upsert_series(&mut self, schedule: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.transaction, &schedule)
    }

    pub fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(&self.transaction, id)
    }

    pub fn update_task(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = self.resolve_list_alias(task.list_id)?;
        update_task_on(&self.transaction, &task)
    }

    pub fn update_list(&mut self, list: List) -> Result<(), StorageError> {
        update_list_on(&self.transaction, &list)
    }

    /// Physically deletes a task and all descendants in this write transaction.
    ///
    /// Related reminders and undo entries are removed by the same operation.
    /// Dropping the transaction without committing rolls every deletion back.
    pub fn delete_task_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        self.get_task(task_id)?;
        delete_task_subtree_on(&self.transaction, task_id)
    }

    /// Physically deletes a non-default List and moves its Tasks to the
    /// Tenant's default Inbox in this write transaction.
    ///
    /// Task history, reminders, and timer relations remain intact. Dropping the
    /// transaction without committing rolls every placement change back.
    pub fn delete_list_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = self.resolve_list_alias(list_id)?;
        let list = get_list_on(&self.transaction, list_id)?;
        if list.is_default {
            return Err(StorageError::DefaultListProtected {
                operation: "deleted",
                list_id,
            });
        }
        delete_list_and_rehome_tasks_for_sync_on(&self.transaction, list_id)
    }

    pub fn update_task_with_undo(
        &mut self,
        mut before: Task,
        mut after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        before.list_id = self.resolve_list_alias(before.list_id)?;
        after.list_id = self.resolve_list_alias(after.list_id)?;
        update_task_with_undo_on(&self.transaction, before, after, operation_type, created_at)
    }

    pub fn update_with_undo(
        &mut self,
        before: Task,
        after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        self.update_task_with_undo(before, after, operation_type, created_at)
    }

    pub fn undo_task_operation(
        &mut self,
        undo_id: Uuid,
        consumed_at: i64,
    ) -> Result<Task, StorageError> {
        undo_task_operation_on(&self.transaction, undo_id, consumed_at)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_setting_on(&self.transaction, key)
    }

    pub fn set_setting(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_setting_on(&self.transaction, key, value, updated_at)
    }

    pub fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(&self.transaction, entry)
    }

    pub fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(&self.transaction, limit)
    }

    pub fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(&self.transaction, collection, record_id)
    }

    pub fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(&self.transaction, op_id)
    }

    pub fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(&self.transaction, collection, record_id)
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.transaction, collection, record_id)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.transaction, state)
    }

    pub fn commit(self) -> Result<(), StorageError> {
        self.transaction.commit().map_err(StorageError::from)
    }
}

/// An owned `BEGIN IMMEDIATE` transaction for sync runs that must move across
/// adapter boundaries without borrowing or self-referencing a connection.
///
/// Calling [`Self::commit`] or [`Self::rollback`] returns the opened
/// connection. Dropping an unfinished value rolls every write back.
pub struct OwnedSqliteWriteTx {
    connection: Option<Connection>,
}

impl OwnedSqliteWriteTx {
    pub fn begin(connection: Connection) -> Result<Self, StorageError> {
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            connection: Some(connection),
        })
    }

    fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("active owned transaction always has a connection")
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_setting_on(self.connection(), key)
    }

    pub fn set_setting(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_setting_on(self.connection(), key, value, updated_at)
    }

    pub fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(self.connection(), entry)
    }

    pub fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(self.connection(), limit)
    }

    pub fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(self.connection(), collection, record_id)
    }

    pub fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(self.connection(), op_id)
    }

    pub fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(self.connection(), collection, record_id)
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(self.connection(), collection, record_id)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(self.connection(), state)
    }

    pub fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError> {
        get_cursor_on(self.connection(), name)
    }

    pub fn set_cursor(
        &mut self,
        name: &str,
        seq: i64,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_cursor_on(self.connection(), name, seq, updated_at)
    }

    pub fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError> {
        delete_cursor_on(self.connection(), name)
    }

    pub fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(self.connection(), entry)
    }

    pub fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(self.connection(), limit)
    }

    pub fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(self.connection(), after, limit)
    }

    pub fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(self.connection(), record_id)
    }

    /// Lists raw durable semantic states. Callers must perform typed plaintext
    /// validation before treating live rows as canonical Inbox candidates.
    pub fn list_record_states(
        &self,
        collection: &str,
    ) -> Result<Vec<SyncRecordState>, StorageError> {
        list_record_states_on(self.connection(), collection)
    }

    /// Returns true for any unresolved live head in the requested collection,
    /// including non-replayable corruption and unsupported plaintext failures.
    pub fn has_live_quarantine(&self, collection: &str) -> Result<bool, StorageError> {
        has_live_quarantine_on(self.connection(), collection)
    }

    pub fn list_list_aliases(&self) -> Result<Vec<ListAlias>, StorageError> {
        list_list_aliases_on(self.connection())
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(self.connection(), list_id)
    }

    pub fn materialize_canonical_list(
        &mut self,
        canonical_list_id: Uuid,
    ) -> Result<(), StorageError> {
        materialize_canonical_list_on(self.connection(), canonical_list_id)
    }

    pub fn replace_list_aliases(
        &mut self,
        canonical_list_id: Uuid,
        alias_list_ids: &[Uuid],
        updated_at: i64,
    ) -> Result<(), StorageError> {
        replace_list_aliases_on(
            self.connection(),
            canonical_list_id,
            alias_list_ids,
            updated_at,
        )
    }

    pub fn load_full_resync(&self) -> Result<Option<FullResyncProgress>, StorageError> {
        load_full_resync_on(self.connection())
    }

    pub fn start_full_resync(
        &mut self,
        generation_id: Uuid,
        continuity_generation: i64,
        base_seq: i64,
        now_ms: i64,
    ) -> Result<FullResyncProgress, StorageError> {
        start_full_resync_on(
            self.connection(),
            generation_id,
            continuity_generation,
            base_seq,
            now_ms,
        )
    }

    pub fn mark_full_resync_record(
        &mut self,
        generation_id: Uuid,
        collection: &str,
        record_id: Uuid,
    ) -> Result<(), StorageError> {
        mark_full_resync_record_on(self.connection(), generation_id, collection, record_id)
    }

    pub fn advance_full_resync_base(
        &mut self,
        generation_id: Uuid,
        next_cursor: Option<&FullResyncStableCursor>,
        base_complete: bool,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        advance_full_resync_base_on(
            self.connection(),
            generation_id,
            next_cursor,
            base_complete,
            now_ms,
        )
    }

    pub fn advance_full_resync_delta(
        &mut self,
        generation_id: Uuid,
        delta_cursor: i64,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        advance_full_resync_delta_on(self.connection(), generation_id, delta_cursor, now_ms)
    }

    pub fn enter_full_resync_sweep(
        &mut self,
        generation_id: Uuid,
        closure_high_water: i64,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        enter_full_resync_sweep_on(self.connection(), generation_id, closure_high_water, now_ms)
    }

    pub fn sweep_full_resync_batch(
        &mut self,
        generation_id: Uuid,
        limit: usize,
        now_ms: i64,
    ) -> Result<FullResyncSweepSummary, StorageError> {
        sweep_full_resync_batch_on(self.connection(), generation_id, limit, now_ms)
    }

    pub fn finalize_full_resync(
        &mut self,
        generation_id: Uuid,
        cursor_name: &str,
        now_ms: i64,
    ) -> Result<i64, StorageError> {
        finalize_full_resync_on(self.connection(), generation_id, cursor_name, now_ms)
    }

    pub fn default_list_id(&self) -> Result<Option<Uuid>, StorageError> {
        get_default_list_on(self.connection()).map(|list| list.map(|list| list.id))
    }

    pub fn get_list(&self, id: Uuid) -> Result<Option<List>, StorageError> {
        optional_not_found(get_list_on(self.connection(), id))
    }

    pub fn upsert_list_for_sync(&mut self, list: List) -> Result<(), StorageError> {
        upsert_list_for_sync_on(self.connection(), list)
    }

    pub fn delete_list_and_rehome_tasks_for_sync(
        &mut self,
        list_id: Uuid,
    ) -> Result<usize, StorageError> {
        delete_list_and_rehome_tasks_for_sync_on(self.connection(), list_id)
    }

    pub fn get_task(&self, id: Uuid) -> Result<Option<Task>, StorageError> {
        optional_not_found(get_task_on(self.connection(), id))
    }

    pub fn list_all_tasks_by_list_for_sync(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(self.connection(), list_id)
    }

    pub fn list_all_tasks_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(self.connection())
    }

    /// Raw per-list enumeration used by existing sync and deletion adapters.
    pub fn list_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        self.list_all_tasks_by_list_for_sync(list_id)
    }

    pub fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), StorageError> {
        upsert_task_for_sync_on(self.connection(), task)
    }

    pub fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        delete_task_subtree_on(self.connection(), task_id)
    }

    pub fn list_task_subtree_for_sync(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(self.connection(), task_id)
    }

    pub fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(self.connection(), id)
    }

    pub fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(self.connection())
    }

    pub fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(self.connection(), &template)
    }

    pub fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(self.connection(), id)
    }

    pub fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(self.connection(), id)
    }

    pub fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(self.connection())
    }

    pub fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(self.connection(), now_ms)
    }

    pub fn upsert_series(&mut self, schedule: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(self.connection(), &schedule)
    }

    pub fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(self.connection(), id)
    }

    pub fn list_timer_sessions_by_task_for_sync(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            self.connection(),
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    pub fn clear_active_timer_for_task_for_sync(
        &mut self,
        task_id: Uuid,
    ) -> Result<bool, StorageError> {
        Ok(self.connection().execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    pub fn get_timer_session(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(self.connection(), id)
    }

    pub fn insert_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(self.connection(), session)
    }

    pub fn delete_timer_session(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .connection()
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }

    pub fn commit(mut self) -> Result<Connection, StorageError> {
        self.connection().execute_batch("COMMIT")?;
        Ok(self
            .connection
            .take()
            .expect("committed owned transaction has a connection"))
    }

    pub fn rollback(mut self) -> Result<Connection, StorageError> {
        self.connection().execute_batch("ROLLBACK")?;
        Ok(self
            .connection
            .take()
            .expect("rolled back owned transaction has a connection"))
    }
}

impl Drop for OwnedSqliteWriteTx {
    fn drop(&mut self) {
        if let Some(connection) = &self.connection {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
}

fn optional_not_found<T>(result: Result<T, StorageError>) -> Result<Option<T>, StorageError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Opens a SQLCipher encrypted SQLite database and migrates it to the latest schema.
pub fn open_encrypted(path: &Path, key: &[u8; 32]) -> Result<Connection, StorageError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(LOCAL_DB_BUSY_TIMEOUT)?;
    apply_sqlcipher_key(&connection, key)?;
    ensure_schema(&mut connection, MIGRATIONS)?;
    Ok(connection)
}

/// Rekeys an existing SQLCipher database and returns immediately after the
/// SQLCipher operation has completed.
///
/// SQLCipher performs `PRAGMA rekey` atomically at the database-file boundary.
/// The active/pending capsule coordinator in `taskveil-client` deliberately
/// performs new-key reopen verification as a separate crash boundary.
pub fn rekey_encrypted_database(
    path: &Path,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<(), StorageError> {
    if old_key == new_key {
        return Ok(());
    }

    let connection = open_encrypted(path, old_key)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let new_key_hex = hex::encode(new_key);
    connection.execute_batch(&format!("PRAGMA rekey = \"x'{new_key_hex}'\";"))?;
    drop(connection);
    Ok(())
}

fn apply_sqlcipher_key(connection: &Connection, key: &[u8; 32]) -> Result<(), StorageError> {
    let key_hex = hex::encode(key);
    connection.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;
    Ok(())
}

/// SQLite-backed implementation of [`TaskRepository`].
pub struct SqliteTaskRepository {
    connection: Connection,
}

impl SqliteTaskRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn list_subtree_for_sync(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(&self.connection, task_id)
    }

    pub fn upsert_for_sync(&mut self, task: Task) -> Result<(), StorageError> {
        upsert_task_for_sync_on(&self.connection, task)
    }

    pub fn delete_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        let transaction = self.connection.transaction()?;
        let deleted = delete_task_subtree_on(&transaction, task_id)?;
        transaction.commit()?;
        Ok(deleted)
    }

    /// Updates a task and records the undo snapshot in the same SQLite transaction.
    pub fn update_with_undo(
        &mut self,
        mut before: Task,
        mut after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        let transaction = self.connection.transaction()?;
        before.list_id = resolve_list_alias_on(&transaction, before.list_id)?;
        after.list_id = resolve_list_alias_on(&transaction, after.list_id)?;
        let entry =
            update_task_with_undo_on(&transaction, before, after, operation_type, created_at)?;
        transaction.commit()?;

        Ok(entry)
    }

    pub fn latest_unconsumed_undo(&self) -> Result<Option<TaskUndoEntry>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, operation_type, task_id, list_id, before_snapshot,
                        after_updated_at, after_deleted_at, after_completed_at,
                        created_at, consumed_at
                 FROM task_undo_entries
                 WHERE consumed_at IS NULL
                   AND operation_type != 'delete'
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                [],
                row_to_task_undo_entry,
            )
            .optional()?
            .transpose()
    }

    pub fn undo_task_operation(
        &mut self,
        undo_id: Uuid,
        consumed_at: i64,
    ) -> Result<Task, StorageError> {
        let transaction = self.connection.transaction()?;
        let restored = undo_task_operation_on(&transaction, undo_id, consumed_at)?;
        transaction.commit()?;

        Ok(restored)
    }
}

impl TaskRepository for SqliteTaskRepository {
    fn get(&self, id: Uuid) -> Result<Task, StorageError> {
        get_task_on(&self.connection, id)
    }

    fn insert(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = resolve_list_alias_on(&self.connection, task.list_id)?;
        insert_task_on(&self.connection, &task)
    }

    fn update(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = resolve_list_alias_on(&self.connection, task.list_id)?;
        update_task_on(&self.connection, &task)
    }

    fn list_all_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(&self.connection)
    }

    fn list_active_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(
            &self.connection,
            resolve_list_alias_on(&self.connection, list_id)?,
        )
    }

    fn list_home(
        &self,
        today_start_ms: i64,
        tomorrow_start_ms: i64,
    ) -> Result<Vec<HomeTask>, StorageError> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE home_targets(id) AS (
                 SELECT tasks.id
                 FROM tasks
                 INNER JOIN lists ON lists.id = tasks.list_id
                 WHERE lists.archived_at IS NULL
                   AND (
                       (
                           tasks.status IN ('todo', 'in_progress')
                           AND (
                               tasks.due_kind IS NOT NULL
                               OR (
                                   tasks.scheduled_at >= ?1
                                   AND tasks.scheduled_at < ?2
                               )
                           )
                       )
                       OR (
                           tasks.status IN ('done', 'wont_do')
                           AND tasks.completed_at >= ?1
                           AND tasks.completed_at < ?2
                       )
                   )
             ),
             home_scope(id) AS (
                 SELECT id FROM home_targets
                 UNION
                 SELECT child.id
                 FROM tasks child
                 INNER JOIN home_scope parent ON child.parent_task_id = parent.id
             ),
             home_ancestors(id) AS (
                 SELECT tasks.parent_task_id
                 FROM tasks
                 INNER JOIN home_targets ON home_targets.id = tasks.id
                 WHERE tasks.parent_task_id IS NOT NULL
                 UNION
                 SELECT tasks.parent_task_id
                 FROM tasks
                 INNER JOIN home_ancestors ancestor ON ancestor.id = tasks.id
                 WHERE tasks.parent_task_id IS NOT NULL
             ),
             home_display_scope(id) AS (
                 SELECT id FROM home_scope
                 UNION
                 SELECT id FROM home_ancestors
             )
             SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
                    tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on, tasks.due_at_ms, tasks.due_time_zone,
                    tasks.scheduled_at, tasks.estimated_minutes, tasks.sort_order,
                    tasks.completed_at, tasks.closed_reason, tasks.deleted_at,
                    tasks.assignee, tasks.series_id,
                    tasks.series_revision, tasks.blueprint_node_key,
                    tasks.series_occurrence_at, tasks.created_at, tasks.updated_at,
                    lists.name,
                    EXISTS(SELECT 1 FROM home_targets WHERE home_targets.id = tasks.id)
             FROM tasks
             INNER JOIN lists ON lists.id = tasks.list_id
             INNER JOIN home_display_scope ON home_display_scope.id = tasks.id
             WHERE lists.archived_at IS NULL
             ORDER BY tasks.due_kind IS NULL ASC,
                      CASE tasks.due_kind WHEN 'datetime' THEN 0 WHEN 'date' THEN 1 ELSE 2 END ASC,
                      tasks.due_at_ms ASC,
                      tasks.due_on ASC,
                      tasks.sort_order ASC,
                      tasks.id ASC",
        )?;
        let tasks = statement
            .query_map(params![today_start_ms, tomorrow_start_ms], row_to_home_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    }

    fn list_calendar_occurrences(
        &self,
        range: &CalendarRange,
    ) -> Result<Vec<CalendarOccurrence>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
                    tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on, tasks.due_at_ms, tasks.due_time_zone,
                    tasks.scheduled_at, tasks.estimated_minutes, tasks.sort_order,
                    tasks.completed_at, tasks.closed_reason, tasks.deleted_at,
                    tasks.assignee, tasks.series_id,
                    tasks.series_revision, tasks.blueprint_node_key,
                    tasks.series_occurrence_at, tasks.created_at, tasks.updated_at,
                    lists.name, lists.archived_at
             FROM tasks
             INNER JOIN lists ON lists.id = tasks.list_id
             WHERE tasks.deleted_at IS NULL
               AND (
                    (tasks.status IN ('todo', 'in_progress') AND tasks.due_kind = 'date' AND tasks.due_on >= ?1 AND tasks.due_on < ?2)
                    OR (tasks.status IN ('todo', 'in_progress') AND tasks.due_kind = 'datetime' AND tasks.due_at_ms >= ?3 AND tasks.due_at_ms < ?4)
                    OR (tasks.status IN ('todo', 'in_progress') AND tasks.scheduled_at >= ?3 AND tasks.scheduled_at < ?4)
                    OR (tasks.status IN ('done', 'wont_do') AND tasks.completed_at >= ?3 AND tasks.completed_at < ?4)
               )
             ORDER BY tasks.sort_order ASC, tasks.id ASC",
        )?;
        let rows = statement
            .query_map(
                params![
                    range.start_on().as_str(),
                    range.end_on().as_str(),
                    range.start_at().as_millis(),
                    range.end_at().as_millis(),
                ],
                |row| {
                    Ok((
                        row_to_task(row)?,
                        row.get::<_, String>(24)?,
                        row.get::<_, Option<i64>>(25)?.is_some(),
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut occurrences = Vec::new();
        for (task, list_name, list_archived) in rows {
            if matches!(task.status, TaskStatus::Todo | TaskStatus::InProgress) {
                if let Some(due) = task.due.as_ref() {
                    let kind = match due {
                        TaskDue::Date { due_on }
                            if due_on >= range.start_on() && due_on < range.end_on() =>
                        {
                            Some(CalendarOccurrenceKind::DateDue {
                                due_on: due_on.clone(),
                            })
                        }
                        TaskDue::DateTime { due_at, time_zone }
                            if *due_at >= range.start_at() && *due_at < range.end_at() =>
                        {
                            Some(CalendarOccurrenceKind::DateTimeDue {
                                due_at: *due_at,
                                time_zone: time_zone.clone(),
                            })
                        }
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        occurrences.push(CalendarOccurrence {
                            task: task.clone(),
                            list_name: list_name.clone(),
                            list_archived,
                            kind,
                        });
                    }
                }
                if let Some(value) = task.scheduled_at {
                    let scheduled_at = UtcInstant::from_millis(value).map_err(|_| {
                        StorageError::IncompatibleSchema(format!(
                            "task {} contains invalid scheduled_at",
                            task.id
                        ))
                    })?;
                    if scheduled_at >= range.start_at() && scheduled_at < range.end_at() {
                        occurrences.push(CalendarOccurrence {
                            task: task.clone(),
                            list_name: list_name.clone(),
                            list_archived,
                            kind: CalendarOccurrenceKind::Scheduled { scheduled_at },
                        });
                    }
                }
            } else if let Some(value) = task.completed_at {
                let completed_at = UtcInstant::from_millis(value).map_err(|_| {
                    StorageError::IncompatibleSchema(format!(
                        "task {} contains invalid completed_at",
                        task.id
                    ))
                })?;
                if completed_at >= range.start_at() && completed_at < range.end_at() {
                    occurrences.push(CalendarOccurrence {
                        task,
                        list_name,
                        list_archived,
                        kind: CalendarOccurrenceKind::Completed { completed_at },
                    });
                }
            }
        }

        Ok(occurrences)
    }

    fn search_tasks(&self, query: &str) -> Result<Vec<Task>, StorageError> {
        let Some(match_query) = build_fts_prefix_query(query) else {
            return Ok(Vec::new());
        };

        let mut statement = self.connection.prepare(
            "SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
                    tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on, tasks.due_at_ms, tasks.due_time_zone,
                    tasks.scheduled_at, tasks.estimated_minutes, tasks.sort_order,
                    tasks.completed_at, tasks.closed_reason, tasks.deleted_at,
                    tasks.assignee, tasks.series_id,
                    tasks.series_revision, tasks.blueprint_node_key,
                    tasks.series_occurrence_at, tasks.created_at, tasks.updated_at
             FROM tasks_fts
             INNER JOIN tasks ON tasks.id = tasks_fts.task_id
             WHERE tasks_fts MATCH ?1
               AND tasks.deleted_at IS NULL
             ORDER BY bm25(tasks_fts) ASC,
                      tasks.updated_at DESC,
                      tasks.id ASC",
        )?;
        let tasks = statement
            .query_map([match_query], row_to_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    }

    fn count_descendants(&self, task_id: Uuid) -> Result<usize, StorageError> {
        count_task_descendants_on(&self.connection, task_id)
    }

    fn delete_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        self.get(task_id)?;
        let transaction = self.connection.transaction()?;
        let deleted = delete_task_subtree_on(&transaction, task_id)?;
        transaction.commit()?;
        Ok(deleted)
    }
}

/// SQLite-backed template and Task Series repository.
pub struct SqliteTemplateSeriesRepository {
    connection: Connection,
}

impl SqliteTemplateSeriesRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.connection, &template)
    }

    pub fn upsert_series_for_sync(&mut self, series: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.connection, &series)
    }
}

impl TemplateSeriesRepository for SqliteTemplateSeriesRepository {
    fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(&self.connection, id)
    }

    fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(&self.connection)
    }

    fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.connection, &template)
    }

    fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(&self.connection, id)
    }

    fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(&self.connection, id)
    }

    fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(&self.connection)
    }

    fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(&self.connection, now_ms)
    }

    fn upsert_series(&mut self, series: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.connection, &series)
    }

    fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(&self.connection, id)
    }
}

const TEMPLATE_SELECT: &str =
    "SELECT id, name, default_list_id, blueprint_json, blueprint_revision,
            created_at, updated_at FROM templates";
const SERIES_SELECT: &str =
    "SELECT id, blueprint_json, target_list_id, rrule, starts_at, time_zone,
            next_run_at, enabled, config_revision, config_parent_revision,
            config_effective_from, lineage_json, created_at, updated_at
       FROM task_series";

fn get_template_on(connection: &Connection, id: Uuid) -> Result<TaskTemplate, StorageError> {
    connection
        .query_row(
            &format!("{TEMPLATE_SELECT} WHERE id = ?1"),
            [id.to_string()],
            row_to_template,
        )
        .optional()?
        .ok_or(StorageError::NotFound(id))
}

fn list_templates_on(connection: &Connection) -> Result<Vec<TaskTemplate>, StorageError> {
    let mut statement = connection.prepare(&format!(
        "{TEMPLATE_SELECT} ORDER BY updated_at DESC, id ASC"
    ))?;
    let templates = statement
        .query_map([], row_to_template)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(templates)
}

fn upsert_template_on(
    connection: &Connection,
    template: &TaskTemplate,
) -> Result<(), StorageError> {
    template.validate()?;
    let blueprint_json = serde_json::to_string(&template.blueprint)?;
    connection.execute(
        "INSERT INTO templates (
             id, name, default_list_id, blueprint_json, blueprint_revision,
             created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name,
             default_list_id = excluded.default_list_id,
             blueprint_json = excluded.blueprint_json,
             blueprint_revision = excluded.blueprint_revision,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            template.id.to_string(),
            template.name,
            template.default_list_id.map(|id| id.to_string()),
            blueprint_json,
            template.blueprint_revision,
            template.created_at,
            template.updated_at,
        ],
    )?;
    Ok(())
}

fn delete_template_on(connection: &Connection, id: Uuid) -> Result<bool, StorageError> {
    Ok(connection.execute("DELETE FROM templates WHERE id = ?1", [id.to_string()])? == 1)
}

fn get_series_on(connection: &Connection, id: Uuid) -> Result<TaskSeries, StorageError> {
    connection
        .query_row(
            &format!("{SERIES_SELECT} WHERE id = ?1"),
            [id.to_string()],
            row_to_series,
        )
        .optional()?
        .ok_or(StorageError::NotFound(id))
}

fn list_series_on(connection: &Connection) -> Result<Vec<TaskSeries>, StorageError> {
    list_series_query_on(
        connection,
        &format!("{SERIES_SELECT} ORDER BY updated_at DESC, id ASC"),
        [],
    )
}

fn list_due_series_on(
    connection: &Connection,
    now_ms: i64,
) -> Result<Vec<TaskSeries>, StorageError> {
    list_series_query_on(
        connection,
        &format!(
            "{SERIES_SELECT}
             WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at, id"
        ),
        [now_ms],
    )
}

fn list_series_query_on<P>(
    connection: &Connection,
    query: &str,
    parameters: P,
) -> Result<Vec<TaskSeries>, StorageError>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(query)?;
    let series = statement
        .query_map(parameters, row_to_series)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(series)
}

fn upsert_series_on(connection: &Connection, series: &TaskSeries) -> Result<(), StorageError> {
    series.validate()?;
    let blueprint_json = serde_json::to_string(&series.config.blueprint)?;
    let lineage_json = serde_json::to_string(&series.config.lineage)?;
    connection.execute(
        "INSERT INTO task_series (
             id, blueprint_json, target_list_id, rrule, starts_at, time_zone,
             next_run_at, enabled, config_revision, config_parent_revision,
             config_effective_from, lineage_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(id) DO UPDATE SET
             blueprint_json = excluded.blueprint_json,
             target_list_id = excluded.target_list_id,
             rrule = excluded.rrule,
             starts_at = excluded.starts_at,
             time_zone = excluded.time_zone,
             next_run_at = excluded.next_run_at,
             enabled = excluded.enabled,
             config_revision = excluded.config_revision,
             config_parent_revision = excluded.config_parent_revision,
             config_effective_from = excluded.config_effective_from,
             lineage_json = excluded.lineage_json,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            series.id.to_string(),
            blueprint_json,
            series.config.target_list_id.map(|id| id.to_string()),
            series.config.rrule,
            series.config.starts_at,
            series.config.time_zone,
            series.cursor.next_run_at(),
            series.config.enabled,
            series.config.config_revision,
            series.config.config_parent_revision,
            series.config.config_effective_from,
            lineage_json,
            series.created_at,
            series.updated_at,
        ],
    )?;
    Ok(())
}

fn delete_series_on(connection: &Connection, id: Uuid) -> Result<bool, StorageError> {
    Ok(connection.execute("DELETE FROM task_series WHERE id = ?1", [id.to_string()])? == 1)
}

fn row_to_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTemplate> {
    let id: String = row.get(0)?;
    let default_list_id: Option<String> = row.get(2)?;
    let blueprint_json: String = row.get(3)?;
    let template = TaskTemplate {
        id: parse_uuid(id, 0)?,
        name: row.get(1)?,
        default_list_id: parse_optional_uuid(default_list_id, 2)?,
        blueprint: serde_json::from_str(&blueprint_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        blueprint_revision: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    };
    template.validate().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(template)
}

fn row_to_series(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSeries> {
    let id: String = row.get(0)?;
    let blueprint_json: String = row.get(1)?;
    let target_list_id: Option<String> = row.get(2)?;
    let next_run_at: Option<i64> = row.get(6)?;
    let lineage_json: String = row.get(11)?;
    let series = TaskSeries {
        id: parse_uuid(id, 0)?,
        config: TaskSeriesConfig {
            blueprint: serde_json::from_str(&blueprint_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            target_list_id: parse_optional_uuid(target_list_id, 2)?,
            rrule: row.get(3)?,
            starts_at: row.get(4)?,
            time_zone: row.get(5)?,
            enabled: row.get(7)?,
            config_revision: row.get(8)?,
            config_parent_revision: row.get(9)?,
            config_effective_from: row.get(10)?,
            lineage: serde_json::from_str(&lineage_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        },
        cursor: next_run_at.map_or(SeriesCursor::Exhausted, SeriesCursor::Pending),
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    };
    series.validate().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(series)
}

fn get_task_on(connection: &Connection, id: Uuid) -> Result<Task, StorageError> {
    let task = connection
        .query_row(
            "SELECT id, list_id, parent_task_id, title, note, status, priority,
                    due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                    completed_at, closed_reason, deleted_at, assignee,
                    series_id, series_revision,
                    blueprint_node_key, series_occurrence_at,
                    created_at, updated_at
             FROM tasks
             WHERE id = ?1",
            [id.to_string()],
            row_to_task,
        )
        .optional()?;

    task.ok_or(StorageError::NotFound(id))
}

fn upsert_task_for_sync_on(connection: &Connection, task: Task) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1",
            [task.id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        update_task_on(connection, &task)
    } else {
        insert_task_on(connection, &task)
    }
}

fn list_all_tasks_for_sync_on(connection: &Connection) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         ORDER BY created_at ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(tasks)
}

fn list_active_tasks_by_list_on(
    connection: &Connection,
    list_id: Uuid,
) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         WHERE list_id = ?1
         ORDER BY sort_order ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([list_id.to_string()], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tasks)
}

fn list_task_subtree_on(connection: &Connection, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE subtree(id) AS (
             SELECT id FROM tasks WHERE id = ?1
             UNION ALL
             SELECT tasks.id
             FROM tasks
             INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         WHERE id IN (SELECT id FROM subtree)
         ORDER BY sort_order ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([task_id.to_string()], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tasks)
}

fn insert_task_on(connection: &Connection, task: &Task) -> Result<(), StorageError> {
    let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
    connection.execute(
        "INSERT INTO tasks (
            id, list_id, parent_task_id, title, note, status, priority,
            due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
            completed_at, closed_reason, deleted_at, assignee,
            series_id, series_revision,
            blueprint_node_key, series_occurrence_at,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24
        )",
        params![
            task.id.to_string(),
            task.list_id.to_string(),
            task.parent_task_id.map(|id| id.to_string()),
            task.content.title,
            task.content.note,
            status_to_str(task.status),
            task.content.priority,
            due_kind,
            due_on,
            due_at_ms,
            due_time_zone,
            task.scheduled_at,
            task.content.estimated_minutes,
            task.sort_order,
            task.completed_at,
            task.closed_reason,
            task.deleted_at,
            task.assignee.map(|id| id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_revision.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.blueprint_node_key.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.occurrence_at),
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

fn update_task_on(connection: &Connection, task: &Task) -> Result<(), StorageError> {
    let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
    let changed = connection.execute(
        "UPDATE tasks
         SET list_id = ?2,
             parent_task_id = ?3,
             title = ?4,
             note = ?5,
             status = ?6,
             priority = ?7,
             due_kind = ?8,
             due_on = ?9,
             due_at_ms = ?10,
             due_time_zone = ?11,
             scheduled_at = ?12,
             estimated_minutes = ?13,
             sort_order = ?14,
             completed_at = ?15,
             closed_reason = ?16,
             deleted_at = ?17,
             assignee = ?18,
             series_id = ?19,
             series_revision = ?20,
             blueprint_node_key = ?21,
             series_occurrence_at = ?22,
             created_at = ?23,
             updated_at = ?24
         WHERE id = ?1",
        params![
            task.id.to_string(),
            task.list_id.to_string(),
            task.parent_task_id.map(|id| id.to_string()),
            task.content.title,
            task.content.note,
            status_to_str(task.status),
            task.content.priority,
            due_kind,
            due_on,
            due_at_ms,
            due_time_zone,
            task.scheduled_at,
            task.content.estimated_minutes,
            task.sort_order,
            task.completed_at,
            task.closed_reason,
            task.deleted_at,
            task.assignee.map(|id| id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_revision.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.blueprint_node_key.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.occurrence_at),
            task.created_at,
            task.updated_at,
        ],
    )?;

    if changed == 0 {
        return Err(StorageError::NotFound(task.id));
    }

    Ok(())
}

fn update_task_with_undo_on(
    connection: &Connection,
    before: Task,
    after: Task,
    operation_type: TaskUndoOperation,
    created_at: i64,
) -> Result<TaskUndoEntry, StorageError> {
    let entry = TaskUndoEntry {
        id: Uuid::now_v7(),
        operation_type,
        task_id: before.id,
        list_id: before.list_id,
        before_snapshot: before,
        after_updated_at: after.updated_at,
        after_deleted_at: after.deleted_at,
        after_completed_at: after.completed_at,
        created_at,
        consumed_at: None,
    };

    update_task_on(connection, &after)?;
    insert_task_undo_on(connection, &entry)?;
    Ok(entry)
}

fn undo_task_operation_on(
    connection: &Connection,
    undo_id: Uuid,
    consumed_at: i64,
) -> Result<Task, StorageError> {
    let entry = connection
        .query_row(
            "SELECT id, operation_type, task_id, list_id, before_snapshot,
                    after_updated_at, after_deleted_at, after_completed_at,
                    created_at, consumed_at
             FROM task_undo_entries
             WHERE id = ?1",
            [undo_id.to_string()],
            row_to_task_undo_entry,
        )
        .optional()?
        .transpose()?
        .ok_or(StorageError::NotFound(undo_id))?;

    if entry.consumed_at.is_some() {
        return Err(StorageError::UndoConsumed(undo_id));
    }

    let current = get_task_on(connection, entry.task_id)?;
    if current.updated_at != entry.after_updated_at
        || current.deleted_at != entry.after_deleted_at
        || current.completed_at != entry.after_completed_at
    {
        return Err(StorageError::UndoConflict(entry.task_id));
    }

    update_task_on(connection, &entry.before_snapshot)?;
    let changed = connection.execute(
        "UPDATE task_undo_entries
         SET consumed_at = ?2
         WHERE id = ?1 AND consumed_at IS NULL",
        params![undo_id.to_string(), consumed_at],
    )?;
    if changed == 0 {
        return Err(StorageError::UndoConsumed(undo_id));
    }

    Ok(entry.before_snapshot)
}

fn build_fts_prefix_query(query: &str) -> Option<String> {
    let terms = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn count_task_descendants_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<usize, StorageError> {
    let count: i64 = connection.query_row(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE parent_task_id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         SELECT count(*) FROM subtree",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|_| {
        StorageError::IncompatibleSchema("task descendant count exceeded usize".to_string())
    })
}

fn delete_task_subtree_on(connection: &Connection, task_id: Uuid) -> Result<usize, StorageError> {
    connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM task_undo_entries
         WHERE task_id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM reminders
         WHERE task_id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    let deleted = connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM tasks
         WHERE id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    Ok(deleted)
}

fn insert_task_undo_on(connection: &Connection, entry: &TaskUndoEntry) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO task_undo_entries (
            id, operation_type, task_id, list_id, before_snapshot,
            after_updated_at, after_deleted_at, after_completed_at,
            created_at, consumed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
        )",
        params![
            entry.id.to_string(),
            undo_operation_to_str(entry.operation_type),
            entry.task_id.to_string(),
            entry.list_id.to_string(),
            serde_json::to_string(&entry.before_snapshot)?,
            entry.after_updated_at,
            entry.after_deleted_at,
            entry.after_completed_at,
            entry.created_at,
            entry.consumed_at,
        ],
    )?;
    Ok(())
}

fn get_list_on(connection: &Connection, id: Uuid) -> Result<List, StorageError> {
    let list = connection
        .query_row(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE id = ?1",
            [id.to_string()],
            row_to_list,
        )
        .optional()?;
    list.ok_or(StorageError::NotFound(id))
}

fn get_default_list_on(connection: &Connection) -> Result<Option<List>, StorageError> {
    connection
        .query_row(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE is_default = 1
             LIMIT 1",
            [],
            row_to_list,
        )
        .optional()
        .map_err(StorageError::from)
}

fn insert_list_on(connection: &Connection, list: &List) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO lists (
            id, name, color, icon, sort_order, is_default, archived_at,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
        )",
        params![
            list.id.to_string(),
            list.name,
            list.color,
            list.icon,
            list.sort_order,
            list.is_default,
            list.archived_at,
            list.created_at,
            list.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_list_for_sync_on(connection: &Connection, list: List) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM lists WHERE id = ?1",
            [list.id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        update_list_on(connection, &list)
    } else {
        insert_list_on(connection, &list)
    }
}

fn delete_list_and_rehome_tasks_for_sync_on(
    connection: &Connection,
    list_id: Uuid,
) -> Result<usize, StorageError> {
    let list = get_list_on(connection, list_id)?;
    if list.is_default {
        return Err(StorageError::DefaultListProtected {
            operation: "deleted",
            list_id,
        });
    }
    let default_list = get_default_list_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema(
            "Tenant must have a default Inbox before deleting a List".to_string(),
        )
    })?;
    let task_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM tasks WHERE list_id = ?1",
            [list_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    connection.execute(
        "UPDATE tasks SET list_id = ?2 WHERE list_id = ?1",
        params![list_id.to_string(), default_list.id.to_string()],
    )?;
    connection.execute(
        "DELETE FROM list_aliases
         WHERE alias_list_id = ?1 OR canonical_list_id = ?1",
        [list_id.to_string()],
    )?;
    connection.execute("DELETE FROM lists WHERE id = ?1", [list_id.to_string()])?;
    usize::try_from(task_count)
        .map_err(|_| StorageError::IncompatibleSchema("list task count exceeded usize".to_string()))
}

fn update_list_on(connection: &Connection, list: &List) -> Result<(), StorageError> {
    if list.is_default && list.archived_at.is_some() {
        return Err(StorageError::DefaultListProtected {
            operation: "archived",
            list_id: list.id,
        });
    }
    let changed = connection.execute(
        "UPDATE lists
         SET name = ?2,
             color = ?3,
             icon = ?4,
             sort_order = ?5,
             is_default = ?6,
             archived_at = ?7,
             created_at = ?8,
             updated_at = ?9
         WHERE id = ?1",
        params![
            list.id.to_string(),
            list.name,
            list.color,
            list.icon,
            list.sort_order,
            list.is_default,
            list.archived_at,
            list.created_at,
            list.updated_at,
        ],
    )?;
    if changed == 0 {
        return Err(StorageError::NotFound(list.id));
    }
    Ok(())
}

/// SQLite-backed implementation of [`ListRepository`].
pub struct SqliteListRepository {
    connection: Connection,
}

impl SqliteListRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Raw sync-facing lookup that intentionally does not resolve a durable
    /// alias. The authenticated list record keeps its own identity.
    pub fn get_raw_for_sync(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(&self.connection, id)
    }

    /// Raw sync-facing enumeration including archived and aliased list rows.
    pub fn list_all_for_sync(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?;
        Ok(lists)
    }

    pub fn upsert_for_sync(&mut self, list: List) -> Result<(), StorageError> {
        upsert_list_for_sync_on(&self.connection, list)
    }

    pub fn delete_and_rehome_tasks_for_sync(
        &mut self,
        list_id: Uuid,
    ) -> Result<usize, StorageError> {
        let transaction = self.connection.transaction()?;
        let task_count = delete_list_and_rehome_tasks_for_sync_on(&transaction, list_id)?;
        transaction.commit()?;
        Ok(task_count)
    }
}

pub struct SqliteTimerSessionRepository {
    connection: Connection,
}

impl SqliteTimerSessionRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl TimerSessionRepository for SqliteTimerSessionRepository {
    fn load_active(&self) -> Result<Option<ActiveTimerSession>, StorageError> {
        self.connection
            .query_row(
                "SELECT session_id, task_id, mode, phase, state, started_at,
                        last_resumed_at, accumulated_active_ms, target_duration_ms
                 FROM active_timer_session WHERE singleton = 1",
                [],
                row_to_active_timer_session,
            )
            .optional()?
            .transpose()
    }

    fn start_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        start_active_timer_session_on(&self.connection, session, updated_at)
    }

    fn update_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        update_active_timer_session_on(&self.connection, session, updated_at)
    }

    fn clear_active(&mut self, expected_session_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM active_timer_session WHERE singleton = 1 AND session_id = ?1",
            [expected_session_id.to_string()],
        )? == 1)
    }

    fn clear_active_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    fn get_completed(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(&self.connection, id)
    }

    fn insert_completed(&mut self, session: CompletedTimerSession) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(&self.connection, session)
    }

    fn list_completed(&self) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions ORDER BY started_at, id",
            [],
        )
    }

    fn list_completed_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    fn list_completed_by_list(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT timer.id, timer.task_id, timer.mode, timer.finish_kind,
                    timer.started_at, timer.ended_at, timer.active_duration_ms, timer.created_at
             FROM timer_sessions timer
             INNER JOIN tasks ON tasks.id = timer.task_id
             WHERE tasks.list_id = ?1 ORDER BY timer.started_at, timer.id",
            [list_id.to_string()],
        )
    }

    fn delete_completed(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }
}

fn start_active_timer_session_on(
    connection: &Connection,
    session: ActiveTimerSession,
    updated_at: i64,
) -> Result<(), StorageError> {
    validate_active_timer_session(&session).map_err(StorageError::InvalidActiveTimerUpdate)?;
    if let Some(task_id) = session.task_id {
        require_existing_timer_task(connection, task_id)?;
    }
    let changed = connection.execute(
        "INSERT INTO active_timer_session (
             singleton, session_id, task_id, mode, phase, state, started_at,
             last_resumed_at, accumulated_active_ms, target_duration_ms, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(singleton) DO NOTHING",
        params![
            session.session_id.to_string(),
            session.task_id.map(|id| id.to_string()),
            timer_mode_str(session.mode),
            timer_phase_str(session.phase),
            timer_run_state_str(session.state),
            session.started_at,
            session.last_resumed_at,
            session.accumulated_active_ms,
            session.target_duration_ms,
            updated_at,
        ],
    )?;
    if changed == 0 {
        let existing = load_active_timer_session_on(connection)?.ok_or_else(|| {
            StorageError::IncompatibleSchema("active timer conflict without row".to_string())
        })?;
        return Err(StorageError::ActiveTimerConflict(existing.session_id));
    }
    Ok(())
}

fn update_active_timer_session_on(
    connection: &Connection,
    session: ActiveTimerSession,
    updated_at: i64,
) -> Result<(), StorageError> {
    let current = load_active_timer_session_on(connection)?
        .ok_or(StorageError::NotFound(session.session_id))?;
    validate_active_timer_update(&current, &session)
        .map_err(StorageError::InvalidActiveTimerUpdate)?;
    let changed = connection.execute(
        "UPDATE active_timer_session
         SET state = ?1, last_resumed_at = ?2, accumulated_active_ms = ?3,
             target_duration_ms = ?4, updated_at = ?5
         WHERE singleton = 1 AND session_id = ?6",
        params![
            timer_run_state_str(session.state),
            session.last_resumed_at,
            session.accumulated_active_ms,
            session.target_duration_ms,
            updated_at,
            session.session_id.to_string(),
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::NotFound(session.session_id));
    }
    Ok(())
}

fn load_active_timer_session_on(
    connection: &Connection,
) -> Result<Option<ActiveTimerSession>, StorageError> {
    connection
        .query_row(
            "SELECT session_id, task_id, mode, phase, state, started_at,
                    last_resumed_at, accumulated_active_ms, target_duration_ms
             FROM active_timer_session WHERE singleton = 1",
            [],
            row_to_active_timer_session,
        )
        .optional()?
        .transpose()
}

fn list_completed_timer_sessions_on<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<CompletedTimerSession>, StorageError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, row_to_completed_timer_session)?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row??);
    }
    Ok(sessions)
}

fn get_completed_timer_session_on(
    connection: &Connection,
    id: Uuid,
) -> Result<CompletedTimerSession, StorageError> {
    connection
        .query_row(
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE id = ?1",
            [id.to_string()],
            row_to_completed_timer_session,
        )
        .optional()?
        .transpose()?
        .ok_or(StorageError::NotFound(id))
}

fn insert_completed_timer_session_on(
    connection: &Connection,
    session: CompletedTimerSession,
) -> Result<bool, StorageError> {
    validate_completed_timer_session(&session)
        .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
    require_existing_timer_task(connection, session.task_id)?;
    match get_completed_timer_session_on(connection, session.id) {
        Ok(existing) if existing == session => return Ok(false),
        Ok(_) => {
            return Err(StorageError::IncompatibleSchema(
                "immutable timer session contents conflict".to_string(),
            ))
        }
        Err(StorageError::NotFound(_)) => {}
        Err(error) => return Err(error),
    }
    connection.execute(
        "INSERT INTO timer_sessions (
             id, task_id, mode, finish_kind, started_at, ended_at,
             active_duration_ms, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session.id.to_string(),
            session.task_id.to_string(),
            timer_mode_str(session.mode),
            timer_finish_kind_str(session.finish_kind),
            session.started_at,
            session.ended_at,
            session.active_duration_ms,
            session.created_at,
        ],
    )?;
    Ok(true)
}

fn row_to_active_timer_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ActiveTimerSession, StorageError>> {
    Ok((|| {
        let session = ActiveTimerSession {
            session_id: parse_uuid_column(row.get(0)?)?,
            task_id: row
                .get::<_, Option<String>>(1)?
                .map(parse_uuid_column)
                .transpose()?,
            mode: parse_timer_mode(&row.get::<_, String>(2)?)?,
            phase: parse_timer_phase(&row.get::<_, String>(3)?)?,
            state: parse_timer_run_state(&row.get::<_, String>(4)?)?,
            started_at: row.get(5)?,
            last_resumed_at: row.get(6)?,
            accumulated_active_ms: row.get(7)?,
            target_duration_ms: row.get(8)?,
        };
        validate_active_timer_session(&session)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        Ok(session)
    })())
}

fn row_to_completed_timer_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<CompletedTimerSession, StorageError>> {
    Ok((|| {
        let session = CompletedTimerSession {
            id: parse_uuid_column(row.get(0)?)?,
            task_id: parse_uuid_column(row.get(1)?)?,
            mode: parse_timer_mode(&row.get::<_, String>(2)?)?,
            finish_kind: parse_timer_finish_kind(&row.get::<_, String>(3)?)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            active_duration_ms: row.get(6)?,
            created_at: row.get(7)?,
        };
        validate_completed_timer_session(&session)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        Ok(session)
    })())
}

fn parse_uuid_column(value: String) -> Result<Uuid, StorageError> {
    value.parse().map_err(StorageError::from)
}

fn require_existing_timer_task(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StorageError::NotFound(task_id))
    }
}

fn timer_mode_str(value: TimerMode) -> &'static str {
    match value {
        TimerMode::Pomodoro => "pomodoro",
        TimerMode::Stopwatch => "stopwatch",
    }
}

fn parse_timer_mode(value: &str) -> Result<TimerMode, StorageError> {
    match value {
        "pomodoro" => Ok(TimerMode::Pomodoro),
        "stopwatch" => Ok(TimerMode::Stopwatch),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer mode".to_string(),
        )),
    }
}

fn timer_phase_str(value: TimerPhase) -> &'static str {
    match value {
        TimerPhase::Work => "work",
        TimerPhase::ShortBreak => "short_break",
        TimerPhase::LongBreak => "long_break",
    }
}

fn parse_timer_phase(value: &str) -> Result<TimerPhase, StorageError> {
    match value {
        "work" => Ok(TimerPhase::Work),
        "short_break" => Ok(TimerPhase::ShortBreak),
        "long_break" => Ok(TimerPhase::LongBreak),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer phase".to_string(),
        )),
    }
}

fn timer_run_state_str(value: TimerRunState) -> &'static str {
    match value {
        TimerRunState::Running => "running",
        TimerRunState::Paused => "paused",
    }
}

fn parse_timer_run_state(value: &str) -> Result<TimerRunState, StorageError> {
    match value {
        "running" => Ok(TimerRunState::Running),
        "paused" => Ok(TimerRunState::Paused),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer run state".to_string(),
        )),
    }
}

fn timer_finish_kind_str(value: TimerFinishKind) -> &'static str {
    match value {
        TimerFinishKind::Completed => "completed",
        TimerFinishKind::Interrupted => "interrupted",
    }
}

fn parse_timer_finish_kind(value: &str) -> Result<TimerFinishKind, StorageError> {
    match value {
        "completed" => Ok(TimerFinishKind::Completed),
        "interrupted" => Ok(TimerFinishKind::Interrupted),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer finish kind".to_string(),
        )),
    }
}

impl ListRepository for SqliteListRepository {
    fn get(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(
            &self.connection,
            resolve_list_alias_on(&self.connection, id)?,
        )
    }

    fn insert(&mut self, list: List) -> Result<(), StorageError> {
        insert_list_on(&self.connection, &list)
    }

    fn update(&mut self, list: List) -> Result<(), StorageError> {
        update_list_on(&self.connection, &list)
    }

    fn list_all(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE archived_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
               )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(lists)
    }

    fn list_archived(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE archived_at IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
               )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(lists)
    }

    fn get_default(&self) -> Result<Option<List>, StorageError> {
        get_default_list_on(&self.connection)
    }

    fn ensure_default_list(&mut self, name: String, now_ms: i64) -> Result<List, StorageError> {
        if let Some(list) = self.get_default()? {
            return Ok(list);
        }

        let last_rank: Option<String> =
            self.connection
                .query_row("SELECT max(sort_order) FROM lists", [], |row| row.get(0))?;
        let sort_order = fractional_index_after(last_rank.as_deref())
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        let list = new_default_list(name, sort_order, now_ms)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        self.insert(list.clone())?;
        Ok(list)
    }

    fn count_tasks(&self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = resolve_list_alias_on(&self.connection, list_id)?;
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM tasks WHERE list_id = ?1",
            [list_id.to_string()],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| {
            StorageError::IncompatibleSchema("list task count exceeded usize".to_string())
        })
    }

    fn delete_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = resolve_list_alias_on(&self.connection, list_id)?;
        let list = get_list_on(&self.connection, list_id)?;
        if list.is_default {
            return Err(StorageError::DefaultListProtected {
                operation: "deleted",
                list_id,
            });
        }
        let transaction = self.connection.transaction()?;
        let task_count = delete_list_and_rehome_tasks_for_sync_on(&transaction, list_id)?;
        transaction.commit()?;
        Ok(task_count)
    }
}

/// SQLite-backed implementation of [`SettingsRepository`].
pub struct SqliteSettingsRepository {
    connection: Connection,
}

impl SqliteSettingsRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl SettingsRepository for SqliteSettingsRepository {
    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_setting_on(&self.connection, key)
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), StorageError> {
        set_setting_on(&self.connection, key, value, updated_at)
    }
}

/// SQLCipher-backed local profile binding and wrapped Tenant Root Key cache.
pub struct SqliteLocalCryptoRepository {
    connection: Connection,
}

impl SqliteLocalCryptoRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl LocalCryptoRepository for SqliteLocalCryptoRepository {
    fn load_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError> {
        load_local_profile_binding_on(&self.connection)
    }

    fn bind_tenant_root(
        &mut self,
        binding: LocalProfileBinding,
        tenant_root: &LocalTenantRootKeyBundle,
    ) -> Result<(), StorageError> {
        if tenant_root.tenant_id != binding.tenant_id {
            return Err(StorageError::LocalProfileTenantMismatch {
                bound_tenant_id: binding.tenant_id,
                requested_tenant_id: tenant_root.tenant_id,
            });
        }
        if tenant_root.generation == 0 || tenant_root.wrapped_tenant_root_dek.is_empty() {
            return Err(StorageError::IncompatibleSchema(
                "invalid local Tenant Root DEK cache entry".to_string(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_local_profile_binding_on(&transaction)? {
            if existing.tenant_id != binding.tenant_id {
                return Err(StorageError::LocalProfileTenantMismatch {
                    bound_tenant_id: existing.tenant_id,
                    requested_tenant_id: binding.tenant_id,
                });
            }
            if existing.user_id != binding.user_id {
                return Err(StorageError::LocalProfileUserMismatch {
                    bound_user_id: existing.user_id,
                    requested_user_id: binding.user_id,
                });
            }
        }

        transaction.execute(
            "INSERT INTO local_profile_binding (
                 singleton, tenant_id, user_id, device_id, bound_at, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(singleton) DO UPDATE SET
                 device_id = excluded.device_id,
                 updated_at = excluded.updated_at",
            params![
                binding.tenant_id.to_string(),
                binding.user_id.to_string(),
                binding.device_id.to_string(),
                binding.bound_at,
                binding.updated_at,
            ],
        )?;
        transaction.execute("DELETE FROM local_tenant_root_key_cache", [])?;
        transaction.execute(
            "INSERT INTO local_tenant_root_key_cache (
                tenant_id, generation, wrapped_tenant_root_dek, updated_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                tenant_root.tenant_id.to_string(),
                i64::try_from(tenant_root.generation).map_err(|_| {
                    StorageError::IncompatibleSchema("invalid tenant key generation".to_string())
                })?,
                tenant_root.wrapped_tenant_root_dek,
                tenant_root.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn load_tenant_root(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<LocalTenantRootKeyBundle>, StorageError> {
        if let Some(binding) = load_local_profile_binding_on(&self.connection)? {
            if binding.tenant_id != tenant_id {
                return Err(StorageError::LocalProfileTenantMismatch {
                    bound_tenant_id: binding.tenant_id,
                    requested_tenant_id: tenant_id,
                });
            }
        }
        self.connection
            .query_row(
                "SELECT tenant_id, generation, wrapped_tenant_root_dek, updated_at
                 FROM local_tenant_root_key_cache WHERE tenant_id = ?1",
                [tenant_id.to_string()],
                |row| {
                    let tenant_id = row.get::<_, String>(0)?.parse::<Uuid>().map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(LocalTenantRootKeyBundle {
                        tenant_id,
                        generation: u64::try_from(row.get::<_, i64>(1)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        wrapped_tenant_root_dek: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }
}

fn load_local_profile_binding_on(
    connection: &Connection,
) -> Result<Option<LocalProfileBinding>, StorageError> {
    let row = connection
        .query_row(
            "SELECT tenant_id, user_id, device_id, bound_at, updated_at
             FROM local_profile_binding
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(tenant_id, user_id, device_id, bound_at, updated_at)| {
        Ok(LocalProfileBinding {
            tenant_id: Uuid::parse_str(&tenant_id)?,
            user_id: Uuid::parse_str(&user_id)?,
            device_id: Uuid::parse_str(&device_id)?,
            bound_at,
            updated_at,
        })
    })
    .transpose()
}

fn get_setting_on(connection: &Connection, key: &str) -> Result<Option<String>, StorageError> {
    connection
        .query_row(
            "SELECT value
             FROM settings
             WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
}

fn set_setting_on(
    connection: &Connection,
    key: &str,
    value: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO settings (key, value, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
        params![key, value, updated_at],
    )?;
    Ok(())
}

/// SQLite-backed implementation of [`ReminderRepository`].
pub struct SqliteReminderRepository {
    connection: Connection,
}

impl SqliteReminderRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl ReminderRepository for SqliteReminderRepository {
    fn create_task_reminder(
        &mut self,
        task_id: Uuid,
        remind_at: i64,
        created_at: i64,
    ) -> Result<Reminder, StorageError> {
        ensure_task_open_for_reminder(&self.connection, task_id)?;
        validate_reminder_time(remind_at, created_at)?;
        let reminder = Reminder {
            id: Uuid::now_v7(),
            task_id,
            remind_at,
            snoozed_until: None,
            created_at,
        };
        let transaction = self.connection.transaction()?;
        let reminder_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM reminders WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )?;
        if reminder_count >= MAX_REMINDERS_PER_TASK as i64 {
            return Err(StorageError::ReminderLimitReached {
                limit: MAX_REMINDERS_PER_TASK,
            });
        }
        ensure_unique_reminder_time_on(&transaction, task_id, remind_at, None)?;
        insert_reminder_on(&transaction, &reminder)?;
        transaction.commit()?;
        Ok(reminder)
    }

    fn update_reminder(
        &mut self,
        reminder_id: Uuid,
        remind_at: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        validate_reminder_time(remind_at, updated_at)?;
        let current = get_reminder_on(&self.connection, reminder_id)?;
        ensure_task_open_for_reminder(&self.connection, current.task_id)?;
        let transaction = self.connection.transaction()?;
        ensure_unique_reminder_time_on(
            &transaction,
            current.task_id,
            remind_at,
            Some(reminder_id),
        )?;
        transaction.execute(
            "UPDATE reminders
             SET remind_at = ?2, snoozed_until = NULL
             WHERE id = ?1",
            params![reminder_id.to_string(), remind_at],
        )?;
        let reminder = get_reminder_on(&transaction, reminder_id)?;
        transaction.commit()?;
        Ok(reminder)
    }

    fn delete_reminder(&mut self, reminder_id: Uuid) -> Result<Reminder, StorageError> {
        let reminder = get_reminder_on(&self.connection, reminder_id)?;
        self.connection.execute(
            "DELETE FROM reminders WHERE id = ?1",
            [reminder_id.to_string()],
        )?;
        Ok(reminder)
    }

    fn clear_task_reminders(&mut self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        let reminders = list_task_reminders_on(&self.connection, task_id)?;
        delete_task_reminders_on(&self.connection, task_id)?;
        Ok(reminders)
    }

    fn list_task_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        list_task_reminders_on(&self.connection, task_id)
    }

    fn list_task_subtree_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        ensure_task_exists(&self.connection, task_id)?;
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT id FROM tasks WHERE id = ?1
                 UNION ALL
                 SELECT tasks.id
                 FROM tasks
                 INNER JOIN subtree ON tasks.parent_task_id = subtree.id
             )
             SELECT id, task_id, remind_at, snoozed_until, created_at
             FROM reminders
             WHERE task_id IN (SELECT id FROM subtree)
             ORDER BY COALESCE(snoozed_until, remind_at) ASC, created_at ASC, id ASC",
        )?;
        let reminders = statement
            .query_map([task_id.to_string()], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn list_list_reminders(&self, list_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT reminders.id, reminders.task_id, reminders.remind_at,
                    reminders.snoozed_until, reminders.created_at
             FROM reminders
             INNER JOIN tasks ON tasks.id = reminders.task_id
             WHERE tasks.list_id = ?1
             ORDER BY COALESCE(reminders.snoozed_until, reminders.remind_at) ASC,
                      reminders.created_at ASC,
                      reminders.id ASC",
        )?;
        let reminders = statement
            .query_map([list_id.to_string()], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn list_pending_reminders(&self, now_ms: i64) -> Result<Vec<Reminder>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT reminders.id, reminders.task_id, reminders.remind_at,
                    reminders.snoozed_until, reminders.created_at
             FROM reminders
             INNER JOIN tasks ON tasks.id = reminders.task_id
             WHERE COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
               AND tasks.status IN ('todo', 'in_progress')
               AND tasks.deleted_at IS NULL
             ORDER BY COALESCE(reminders.snoozed_until, reminders.remind_at) ASC,
                      reminders.created_at ASC,
                      reminders.id ASC",
        )?;
        let reminders = statement
            .query_map([now_ms], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn snooze_reminder(
        &mut self,
        reminder_id: Uuid,
        snoozed_until: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        let reminder = get_reminder_on(&self.connection, reminder_id)?;
        ensure_task_open_for_reminder(&self.connection, reminder.task_id)?;
        validate_reminder_time(snoozed_until, updated_at)?;
        let changed = self.connection.execute(
            "UPDATE reminders
             SET snoozed_until = ?2
             WHERE id = ?1",
            params![reminder_id.to_string(), snoozed_until],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound(reminder_id));
        }
        self.connection
            .query_row(
                "SELECT id, task_id, remind_at, snoozed_until, created_at
                 FROM reminders
                 WHERE id = ?1",
                [reminder_id.to_string()],
                row_to_reminder,
            )
            .map_err(StorageError::from)
    }
}

/// SQLite-backed implementation of [`SyncStateRepository`].
pub struct SqliteSyncStateRepository {
    connection: Connection,
}

impl SqliteSyncStateRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.connection, collection, record_id)
    }

    pub fn list_record_states(
        &self,
        collection: &str,
    ) -> Result<Vec<SyncRecordState>, StorageError> {
        list_record_states_on(&self.connection, collection)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.connection, state)
    }

    pub fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(&self.connection, entry)
    }

    pub fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(&self.connection, limit)
    }

    pub fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(&self.connection, after, limit)
    }

    pub fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(&self.connection, record_id)
    }

    pub fn has_live_quarantine(&self, collection: &str) -> Result<bool, StorageError> {
        has_live_quarantine_on(&self.connection, collection)
    }

    pub fn list_list_aliases(&self) -> Result<Vec<ListAlias>, StorageError> {
        list_list_aliases_on(&self.connection)
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(&self.connection, list_id)
    }

    pub fn load_full_resync(&self) -> Result<Option<FullResyncProgress>, StorageError> {
        load_full_resync_on(&self.connection)
    }

    pub fn start_full_resync(
        &mut self,
        generation_id: Uuid,
        continuity_generation: i64,
        base_seq: i64,
        now_ms: i64,
    ) -> Result<FullResyncProgress, StorageError> {
        start_full_resync_on(
            &self.connection,
            generation_id,
            continuity_generation,
            base_seq,
            now_ms,
        )
    }
}

impl SyncStateRepository for SqliteSyncStateRepository {
    fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(&self.connection, entry)
    }

    fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(&self.connection, limit)
    }

    fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(&self.connection, collection, record_id)
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(&self.connection, op_id)
    }

    fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(&self.connection, collection, record_id)
    }

    fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.connection, collection, record_id)
    }

    fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.connection, state)
    }

    fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError> {
        get_cursor_on(&self.connection, name)
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), StorageError> {
        set_cursor_on(&self.connection, name, seq, updated_at)
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError> {
        delete_cursor_on(&self.connection, name)
    }

    fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(&self.connection, entry)
    }

    fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(&self.connection, limit)
    }

    fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(&self.connection, after, limit)
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(&self.connection, record_id)
    }
}

fn load_full_resync_on(
    connection: &Connection,
) -> Result<Option<FullResyncProgress>, StorageError> {
    let row = connection
        .query_row(
            "SELECT generation_id, continuity_generation, phase, base_seq,
                    base_cursor_collection, base_cursor_record_id,
                    delta_cursor, closure_high_water,
                    sweep_cursor_collection, sweep_cursor_record_id,
                    started_at, updated_at
             FROM sync_full_resync_state
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        generation_id,
        continuity_generation,
        phase,
        base_seq,
        base_cursor_collection,
        base_cursor_record_id,
        delta_cursor,
        closure_high_water,
        sweep_cursor_collection,
        sweep_cursor_record_id,
        started_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    let phase = match phase.as_str() {
        "base" => FullResyncPhase::Base,
        "delta" => FullResyncPhase::Delta,
        "sweep" => FullResyncPhase::Sweep,
        _ => return Err(StorageError::InvalidSyncState(phase)),
    };
    Ok(Some(FullResyncProgress {
        generation_id: Uuid::parse_str(&generation_id)?,
        continuity_generation,
        phase,
        base_seq,
        base_cursor: parse_full_resync_cursor(base_cursor_collection, base_cursor_record_id)?,
        delta_cursor,
        closure_high_water,
        sweep_cursor: parse_full_resync_cursor(sweep_cursor_collection, sweep_cursor_record_id)?,
        started_at,
        updated_at,
    }))
}

fn parse_full_resync_cursor(
    collection: Option<String>,
    record_id: Option<String>,
) -> Result<Option<FullResyncStableCursor>, StorageError> {
    match (collection, record_id) {
        (None, None) => Ok(None),
        (Some(collection), Some(record_id)) => {
            validate_sync_collection(&collection)?;
            Ok(Some(FullResyncStableCursor {
                collection,
                record_id: Uuid::parse_str(&record_id)?,
            }))
        }
        _ => Err(StorageError::IncompatibleSchema(
            "full resync cursor is incomplete".to_string(),
        )),
    }
}

fn start_full_resync_on(
    connection: &Connection,
    generation_id: Uuid,
    continuity_generation: i64,
    base_seq: i64,
    now_ms: i64,
) -> Result<FullResyncProgress, StorageError> {
    if base_seq < 0 || continuity_generation <= 0 {
        return Err(StorageError::IncompatibleSchema(
            "full resync base sequence is negative".to_string(),
        ));
    }
    if let Some(progress) = load_full_resync_on(connection)? {
        return Ok(progress);
    }
    connection.execute(
        "INSERT INTO sync_full_resync_state (
             singleton, generation_id, continuity_generation, phase, base_seq,
             base_cursor_collection, base_cursor_record_id,
             delta_cursor, closure_high_water,
             sweep_cursor_collection, sweep_cursor_record_id,
             started_at, updated_at
         ) VALUES (1, ?1, ?2, 'base', ?3, NULL, NULL, ?3, NULL, NULL, NULL, ?4, ?4)",
        params![
            generation_id.to_string(),
            continuity_generation,
            base_seq,
            now_ms
        ],
    )?;
    load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state was not persisted".to_string())
    })
}

fn require_full_resync(
    connection: &Connection,
    generation_id: Uuid,
    expected_phase: FullResyncPhase,
) -> Result<FullResyncProgress, StorageError> {
    let progress = load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state is missing".to_string())
    })?;
    if progress.generation_id != generation_id || progress.phase != expected_phase {
        return Err(StorageError::IncompatibleSchema(
            "full resync generation or phase mismatch".to_string(),
        ));
    }
    Ok(progress)
}

fn mark_full_resync_record_on(
    connection: &Connection,
    generation_id: Uuid,
    collection: &str,
    record_id: Uuid,
) -> Result<(), StorageError> {
    validate_sync_collection(collection)?;
    let progress = load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state is missing".to_string())
    })?;
    if progress.generation_id != generation_id || progress.phase == FullResyncPhase::Sweep {
        return Err(StorageError::IncompatibleSchema(
            "full resync mark does not belong to an active scan".to_string(),
        ));
    }
    connection.execute(
        "INSERT OR IGNORE INTO sync_full_resync_marks (
             generation_id, collection, record_id
         ) VALUES (?1, ?2, ?3)",
        params![generation_id.to_string(), collection, record_id.to_string()],
    )?;
    Ok(())
}

fn advance_full_resync_base_on(
    connection: &Connection,
    generation_id: Uuid,
    next_cursor: Option<&FullResyncStableCursor>,
    base_complete: bool,
    now_ms: i64,
) -> Result<(), StorageError> {
    require_full_resync(connection, generation_id, FullResyncPhase::Base)?;
    if let Some(cursor) = next_cursor {
        validate_sync_collection(&cursor.collection)?;
    }
    if !base_complete && next_cursor.is_none() {
        return Err(StorageError::IncompatibleSchema(
            "incomplete base page has no continuation cursor".to_string(),
        ));
    }
    let (collection, record_id) = next_cursor
        .map(|cursor| {
            (
                Some(cursor.collection.as_str()),
                Some(cursor.record_id.to_string()),
            )
        })
        .unwrap_or((None, None));
    let phase = if base_complete { "delta" } else { "base" };
    connection.execute(
        "UPDATE sync_full_resync_state
         SET phase = ?2,
             base_cursor_collection = ?3,
             base_cursor_record_id = ?4,
             updated_at = ?5
         WHERE singleton = 1 AND generation_id = ?1",
        params![
            generation_id.to_string(),
            phase,
            collection,
            record_id,
            now_ms
        ],
    )?;
    Ok(())
}

fn advance_full_resync_delta_on(
    connection: &Connection,
    generation_id: Uuid,
    delta_cursor: i64,
    now_ms: i64,
) -> Result<(), StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Delta)?;
    if delta_cursor < progress.delta_cursor {
        return Err(StorageError::IncompatibleSchema(
            "full resync delta cursor moved backwards".to_string(),
        ));
    }
    connection.execute(
        "UPDATE sync_full_resync_state
         SET delta_cursor = ?2, updated_at = ?3
         WHERE singleton = 1 AND generation_id = ?1",
        params![generation_id.to_string(), delta_cursor, now_ms],
    )?;
    Ok(())
}

fn enter_full_resync_sweep_on(
    connection: &Connection,
    generation_id: Uuid,
    closure_high_water: i64,
    now_ms: i64,
) -> Result<(), StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Delta)?;
    if closure_high_water < 0 || progress.delta_cursor != closure_high_water {
        return Err(StorageError::IncompatibleSchema(
            "full resync delta has not reached closure high-water".to_string(),
        ));
    }
    connection.execute(
        "UPDATE sync_full_resync_state
         SET phase = 'sweep', closure_high_water = ?2,
             sweep_cursor_collection = NULL, sweep_cursor_record_id = NULL,
             updated_at = ?3
         WHERE singleton = 1 AND generation_id = ?1",
        params![generation_id.to_string(), closure_high_water, now_ms],
    )?;
    Ok(())
}

fn sweep_full_resync_batch_on(
    connection: &Connection,
    generation_id: Uuid,
    limit: usize,
    now_ms: i64,
) -> Result<FullResyncSweepSummary, StorageError> {
    if limit == 0 {
        return Err(StorageError::IncompatibleSchema(
            "full resync sweep batch limit is zero".to_string(),
        ));
    }
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Sweep)?;
    let (after_order, after_record_id) = progress
        .sweep_cursor
        .as_ref()
        .map(|cursor| {
            Ok::<_, StorageError>((
                Some(local_sweep_collection_order(&cursor.collection)?),
                Some(cursor.record_id.to_string()),
            ))
        })
        .transpose()?
        .unwrap_or((None, None));
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("sweep limit exceeded i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT collection, record_id
         FROM sync_record_states
         WHERE ?1 IS NULL
            OR CASE collection
                   WHEN 'timer_sessions' THEN 0
                   WHEN 'tasks' THEN 1
                   WHEN 'task_series' THEN 2
                   WHEN 'templates' THEN 3
                   WHEN 'lists' THEN 4
               END > ?1
            OR (
                CASE collection
                    WHEN 'timer_sessions' THEN 0
                    WHEN 'tasks' THEN 1
                    WHEN 'task_series' THEN 2
                    WHEN 'templates' THEN 3
                    WHEN 'lists' THEN 4
                END = ?1
                AND record_id > ?2
            )
         ORDER BY CASE collection
                      WHEN 'timer_sessions' THEN 0
                      WHEN 'tasks' THEN 1
                      WHEN 'task_series' THEN 2
                      WHEN 'templates' THEN 3
                      WHEN 'lists' THEN 4
                  END ASC,
                  record_id ASC
         LIMIT ?3",
    )?;
    let records = statement
        .query_map(params![after_order, after_record_id, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut summary = FullResyncSweepSummary {
        scanned_records: records.len(),
        ..FullResyncSweepSummary::default()
    };
    for (collection, record_id) in &records {
        let marked: bool = connection.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_full_resync_marks
                 WHERE generation_id = ?1 AND collection = ?2 AND record_id = ?3
             )",
            params![generation_id.to_string(), collection, record_id],
            |row| row.get(0),
        )?;
        if marked || never_synced_record_is_valid(connection, generation_id, collection, record_id)?
        {
            continue;
        }
        let record_uuid = Uuid::parse_str(record_id)?;
        match collection.as_str() {
            "tasks" => {
                connection.execute(
                    "DELETE FROM task_undo_entries WHERE task_id = ?1",
                    [record_id],
                )?;
                connection.execute("DELETE FROM reminders WHERE task_id = ?1", [record_id])?;
                summary.swept_tasks +=
                    connection.execute("DELETE FROM tasks WHERE id = ?1", [record_id])?;
            }
            "lists" => {
                connection.execute(
                    "DELETE FROM sync_quarantine
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                connection.execute(
                    "DELETE FROM sync_outbox
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                connection.execute(
                    "DELETE FROM sync_record_origins
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                summary.swept_record_states += connection.execute(
                    "DELETE FROM sync_record_states
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                let list_existed: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM lists WHERE id = ?1)",
                    [record_id],
                    |row| row.get(0),
                )?;
                summary.swept_tasks +=
                    delete_list_and_rehome_tasks_for_sync_on(connection, record_uuid)?;
                summary.swept_lists += usize::from(list_existed);
            }
            "templates" => {
                summary.swept_templates +=
                    connection.execute("DELETE FROM templates WHERE id = ?1", [record_id])?;
            }
            "task_series" => {
                summary.swept_task_series +=
                    connection.execute("DELETE FROM task_series WHERE id = ?1", [record_id])?;
            }
            "timer_sessions" => {
                summary.swept_timer_sessions +=
                    connection.execute("DELETE FROM timer_sessions WHERE id = ?1", [record_id])?;
            }
            other => return Err(StorageError::InvalidSyncCollection(other.to_string())),
        }
        connection.execute(
            "DELETE FROM sync_quarantine WHERE record_id = ?1",
            [record_uuid.to_string()],
        )?;
        connection.execute(
            "DELETE FROM sync_outbox WHERE collection = ?1 AND record_id = ?2",
            params![collection, record_id],
        )?;
        connection.execute(
            "DELETE FROM sync_record_origins WHERE record_id = ?1",
            [record_id],
        )?;
        summary.swept_record_states += connection.execute(
            "DELETE FROM sync_record_states WHERE collection = ?1 AND record_id = ?2",
            params![collection, record_id],
        )?;
    }
    if let Some((collection, record_id)) = records.last() {
        connection.execute(
            "UPDATE sync_full_resync_state
             SET sweep_cursor_collection = ?2, sweep_cursor_record_id = ?3, updated_at = ?4
             WHERE singleton = 1 AND generation_id = ?1",
            params![generation_id.to_string(), collection, record_id, now_ms],
        )?;
    }
    Ok(summary)
}

fn never_synced_record_is_valid(
    connection: &Connection,
    generation_id: Uuid,
    collection: &str,
    record_id: &str,
) -> Result<bool, StorageError> {
    match collection {
        "lists" => connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM sync_record_origins origin
                     JOIN sync_outbox outbox ON outbox.record_id = origin.record_id
                     WHERE origin.record_id = ?1 AND origin.collection = 'lists'
                       AND origin.origin_kind = 'never_synced'
                 )",
                [record_id],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        "tasks" => connection
            .query_row(
                "WITH RECURSIVE ancestors(id, parent_task_id, valid) AS (
                     SELECT task.id, task.parent_task_id, 1
                     FROM tasks task WHERE task.id = ?1
                     UNION ALL
                     SELECT parent.id, parent.parent_task_id,
                            EXISTS (
                                SELECT 1 FROM sync_record_origins parent_origin
                                JOIN sync_outbox parent_outbox
                                  ON parent_outbox.record_id = parent_origin.record_id
                                WHERE parent_origin.record_id = parent.id
                                  AND parent_origin.collection = 'tasks'
                                  AND parent_origin.origin_kind = 'never_synced'
                                UNION ALL
                                SELECT 1 FROM sync_full_resync_marks parent_mark
                                WHERE parent_mark.generation_id = ?2
                                  AND parent_mark.collection = 'tasks'
                                  AND parent_mark.record_id = parent.id
                            )
                     FROM tasks parent
                     JOIN ancestors child ON child.parent_task_id = parent.id
                 )
                 SELECT EXISTS (
                     SELECT 1 FROM tasks task
                     JOIN sync_record_origins task_origin
                       ON task_origin.record_id = task.id
                      AND task_origin.collection = 'tasks'
                      AND task_origin.origin_kind = 'never_synced'
                     JOIN sync_outbox task_outbox ON task_outbox.record_id = task.id
                     WHERE task.id = ?1
                       AND EXISTS (
                           SELECT 1 FROM sync_record_origins list_origin
                           JOIN sync_outbox list_outbox
                             ON list_outbox.record_id = list_origin.record_id
                           WHERE list_origin.record_id = task.list_id
                             AND list_origin.collection = 'lists'
                             AND list_origin.origin_kind = 'never_synced'
                           UNION ALL
                           SELECT 1 FROM sync_full_resync_marks list_mark
                           WHERE list_mark.generation_id = ?2
                             AND list_mark.collection = 'lists'
                             AND list_mark.record_id = task.list_id
                       )
                       AND NOT EXISTS (SELECT 1 FROM ancestors WHERE valid = 0)
                       AND NOT EXISTS (
                           SELECT 1 FROM ancestors child
                           WHERE child.parent_task_id IS NOT NULL
                             AND NOT EXISTS (
                                 SELECT 1 FROM ancestors parent
                                 WHERE parent.id = child.parent_task_id
                             )
                       )
                 )",
                params![record_id, generation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        "templates" => never_synced_record_has_outbox(connection, collection, record_id),
        "task_series" => never_synced_record_has_outbox(connection, collection, record_id),
        "timer_sessions" => connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM timer_sessions timer
                     JOIN sync_record_origins origin
                       ON origin.record_id = timer.id
                      AND origin.collection = 'timer_sessions'
                      AND origin.origin_kind = 'never_synced'
                     JOIN sync_outbox outbox
                       ON outbox.record_id = timer.id
                      AND outbox.collection = 'timer_sessions'
                     WHERE timer.id = ?1
                       AND EXISTS (
                           SELECT 1 FROM sync_record_origins task_origin
                           JOIN sync_outbox task_outbox
                             ON task_outbox.record_id = task_origin.record_id
                            AND task_outbox.collection = 'tasks'
                           WHERE task_origin.record_id = timer.task_id
                             AND task_origin.collection = 'tasks'
                             AND task_origin.origin_kind = 'never_synced'
                           UNION ALL
                           SELECT 1 FROM sync_full_resync_marks task_mark
                           WHERE task_mark.generation_id = ?2
                             AND task_mark.collection = 'tasks'
                             AND task_mark.record_id = timer.task_id
                       )
                 )",
                params![record_id, generation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

fn never_synced_record_has_outbox(
    connection: &Connection,
    collection: &str,
    record_id: &str,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_record_origins origin
                 JOIN sync_outbox outbox
                   ON outbox.record_id = origin.record_id
                  AND outbox.collection = origin.collection
                 WHERE origin.record_id = ?1
                   AND origin.collection = ?2
                   AND origin.origin_kind = 'never_synced'
             )",
            params![record_id, collection],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

fn local_sweep_collection_order(collection: &str) -> Result<i64, StorageError> {
    match collection {
        "timer_sessions" => Ok(0),
        "tasks" => Ok(1),
        "task_series" => Ok(2),
        "templates" => Ok(3),
        "lists" => Ok(4),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

fn finalize_full_resync_on(
    connection: &Connection,
    generation_id: Uuid,
    cursor_name: &str,
    now_ms: i64,
) -> Result<i64, StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Sweep)?;
    let high_water = progress.closure_high_water.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync closure high-water is missing".to_string())
    })?;
    let (after_order, after_record_id) = progress
        .sweep_cursor
        .as_ref()
        .map(|cursor| {
            Ok::<_, StorageError>((
                local_sweep_collection_order(&cursor.collection)?,
                cursor.record_id.to_string(),
            ))
        })
        .transpose()?
        .unwrap_or((-1, String::new()));
    let has_more: bool = connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM sync_record_states
             WHERE CASE collection
                       WHEN 'timer_sessions' THEN 0
                       WHEN 'tasks' THEN 1
                       WHEN 'task_series' THEN 2
                       WHEN 'templates' THEN 3
                       WHEN 'lists' THEN 4
                   END > ?1
                OR (
                    CASE collection
                        WHEN 'timer_sessions' THEN 0
                        WHEN 'tasks' THEN 1
                        WHEN 'task_series' THEN 2
                        WHEN 'templates' THEN 3
                        WHEN 'lists' THEN 4
                    END = ?1
                    AND record_id > ?2
                )
         )",
        params![after_order, after_record_id],
        |row| row.get(0),
    )?;
    if has_more {
        return Err(StorageError::IncompatibleSchema(
            "full resync sweep is incomplete".to_string(),
        ));
    }
    set_cursor_on(connection, cursor_name, high_water, now_ms)?;
    connection.execute(
        "DELETE FROM sync_full_resync_marks WHERE generation_id = ?1",
        [generation_id.to_string()],
    )?;
    connection.execute(
        "DELETE FROM sync_full_resync_state WHERE singleton = 1 AND generation_id = ?1",
        [generation_id.to_string()],
    )?;
    Ok(high_water)
}

fn get_cursor_on(connection: &Connection, name: &str) -> Result<Option<SyncCursor>, StorageError> {
    connection
        .query_row(
            "SELECT name, seq, updated_at
             FROM sync_cursors
             WHERE name = ?1",
            [name],
            row_to_sync_cursor,
        )
        .optional()
        .map_err(StorageError::from)
}

fn set_cursor_on(
    connection: &Connection,
    name: &str,
    seq: i64,
    updated_at: i64,
) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO sync_cursors (name, seq, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
             seq = excluded.seq,
             updated_at = excluded.updated_at",
        params![name, seq, updated_at],
    )?;
    Ok(())
}

fn delete_cursor_on(connection: &Connection, name: &str) -> Result<(), StorageError> {
    connection.execute("DELETE FROM sync_cursors WHERE name = ?1", [name])?;
    Ok(())
}

fn list_record_states_on(
    connection: &Connection,
    collection: &str,
) -> Result<Vec<SyncRecordState>, StorageError> {
    validate_sync_collection(collection)?;
    let mut statement = connection.prepare(
        "SELECT record_id, collection, current_revision_hlc, state_kind,
                semantic_hlc, plaintext_json, updated_at
         FROM sync_record_states
         WHERE collection = ?1
         ORDER BY record_id ASC",
    )?;
    let nested_states = statement
        .query_map([collection], row_to_sync_record_state)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let states = nested_states
        .into_iter()
        .collect::<Result<Vec<_>, StorageError>>()?;
    Ok(states)
}

fn has_live_quarantine_on(connection: &Connection, collection: &str) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_quarantine
                 WHERE collection = ?1 AND state_kind = 'live'
             )",
            [collection],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

fn list_list_aliases_on(connection: &Connection) -> Result<Vec<ListAlias>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT alias_list_id, canonical_list_id, updated_at
         FROM list_aliases ORDER BY alias_list_id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(alias_list_id, canonical_list_id, updated_at)| {
            Ok(ListAlias {
                alias_list_id: Uuid::parse_str(&alias_list_id)?,
                canonical_list_id: Uuid::parse_str(&canonical_list_id)?,
                updated_at,
            })
        })
        .collect()
}

fn resolve_list_alias_on(connection: &Connection, list_id: Uuid) -> Result<Uuid, StorageError> {
    // Raw legacy fixtures may use repositories before `open_encrypted` runs
    // migrations. Alias resolution is a no-op until schema v19 exists; a v19
    // database missing the table still fails below as incompatible storage.
    if read_user_version(connection)? < 19 {
        return Ok(list_id);
    }
    let canonical = connection
        .query_row(
            "SELECT canonical_list_id FROM list_aliases WHERE alias_list_id = ?1",
            [list_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(canonical) = canonical else {
        return Ok(list_id);
    };
    let canonical = Uuid::parse_str(&canonical)?;
    let canonical_is_alias: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM list_aliases WHERE alias_list_id = ?1)",
        [canonical.to_string()],
        |row| row.get(0),
    )?;
    if canonical_is_alias {
        return Err(StorageError::IncompatibleSchema(
            "list alias chain is not allowed".to_string(),
        ));
    }
    let canonical_exists: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM lists WHERE id = ?1)",
        [canonical.to_string()],
        |row| row.get(0),
    )?;
    if !canonical_exists {
        return Err(StorageError::IncompatibleSchema(
            "list alias points to a missing canonical list".to_string(),
        ));
    }
    Ok(canonical)
}

fn replace_list_aliases_on(
    connection: &Connection,
    canonical_list_id: Uuid,
    alias_list_ids: &[Uuid],
    updated_at: i64,
) -> Result<(), StorageError> {
    let canonical = get_list_on(connection, canonical_list_id)?;
    if !canonical.is_default {
        return Err(StorageError::IncompatibleSchema(
            "canonical list must be materialized before aliases are replaced".to_string(),
        ));
    }
    let mut unique = HashSet::with_capacity(alias_list_ids.len());
    for alias_list_id in alias_list_ids {
        if *alias_list_id == canonical_list_id || !unique.insert(*alias_list_id) {
            return Err(StorageError::IncompatibleSchema(
                "invalid canonical Inbox alias set".to_string(),
            ));
        }
        get_list_on(connection, *alias_list_id)?;
    }

    connection.execute("DELETE FROM list_aliases", [])?;
    for alias_list_id in alias_list_ids {
        connection.execute(
            "INSERT INTO list_aliases (alias_list_id, canonical_list_id, updated_at)
             VALUES (?1, ?2, ?3)",
            params![
                alias_list_id.to_string(),
                canonical_list_id.to_string(),
                updated_at,
            ],
        )?;
    }
    Ok(())
}

fn materialize_canonical_list_on(
    connection: &Connection,
    canonical_list_id: Uuid,
) -> Result<(), StorageError> {
    let canonical = get_list_on(connection, canonical_list_id)?;
    if canonical.archived_at.is_some() {
        return Err(StorageError::DefaultListProtected {
            operation: "archived",
            list_id: canonical_list_id,
        });
    }
    // Demote first so the existing partial unique index never observes two
    // default rows during a canonical switch.
    connection.execute("UPDATE lists SET is_default = 0 WHERE is_default = 1", [])?;
    let changed = connection.execute(
        "UPDATE lists SET is_default = 1 WHERE id = ?1",
        [canonical_list_id.to_string()],
    )?;
    if changed != 1 {
        return Err(StorageError::NotFound(canonical_list_id));
    }
    Ok(())
}

fn get_record_state_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<Option<SyncRecordState>, StorageError> {
    validate_sync_collection(collection)?;
    let state = connection
        .query_row(
            "SELECT record_id, collection, current_revision_hlc, state_kind,
                    semantic_hlc, plaintext_json, updated_at
             FROM sync_record_states
             WHERE record_id = ?1",
            [record_id.to_string()],
            row_to_sync_record_state,
        )
        .optional()?
        .transpose()?;
    if let Some(state) = &state {
        ensure_requested_collection(record_id, &state.collection, collection)?;
    }
    Ok(state)
}

fn put_record_state_on(
    connection: &Connection,
    state: SyncRecordState,
) -> Result<(), StorageError> {
    validate_sync_collection(&state.collection)?;
    ensure_sync_collection_matches(connection, state.record_id, &state.collection)?;
    let record_id = state.record_id.to_string();
    let collection = state.collection.clone();
    let current_revision_hlc = state.current_revision_hlc.clone();
    let updated_at = state.updated_at;
    let (state_kind, semantic_hlc, plaintext_json) = match state.state {
        SyncRecordSemanticState::Live {
            mutation_hlc,
            plaintext_json,
        } => ("live", mutation_hlc, Some(plaintext_json)),
        SyncRecordSemanticState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_record_states (
             record_id, collection, current_revision_hlc, state_kind,
             semantic_hlc, plaintext_json, updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7
         )
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             current_revision_hlc = excluded.current_revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             plaintext_json = excluded.plaintext_json,
             updated_at = excluded.updated_at",
        params![
            record_id,
            collection,
            current_revision_hlc,
            state_kind,
            semantic_hlc,
            plaintext_json,
            updated_at,
        ],
    )?;
    let origin_kind = if current_revision_hlc.is_some() {
        "server_seen"
    } else {
        "never_synced"
    };
    connection.execute(
        "INSERT INTO sync_record_origins (record_id, collection, origin_kind, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             origin_kind = CASE
                 WHEN sync_record_origins.origin_kind = 'server_seen' THEN 'server_seen'
                 ELSE excluded.origin_kind
             END,
             updated_at = excluded.updated_at",
        params![record_id, collection, origin_kind, updated_at],
    )?;
    Ok(())
}

fn put_outbox_head_on(
    connection: &Connection,
    entry: NewSyncOutboxEntry,
) -> Result<SyncOutboxEntry, StorageError> {
    validate_sync_collection(&entry.collection)?;
    ensure_sync_collection_matches(connection, entry.record_id, &entry.collection)?;
    let origin_record_id = entry.record_id.to_string();
    let origin_collection = entry.collection.clone();
    let origin_updated_at = entry.created_at;
    let (state_kind, semantic_hlc, blob) = match entry.state {
        SyncOutboxState::Live { mutation_hlc, blob } => ("live", mutation_hlc, Some(blob)),
        SyncOutboxState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_outbox (
             record_id, collection, op_id, base_revision_hlc, revision_hlc,
             state_kind, semantic_hlc, blob, created_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
         )
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             op_id = excluded.op_id,
             base_revision_hlc = excluded.base_revision_hlc,
             revision_hlc = excluded.revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             blob = excluded.blob,
             created_at = excluded.created_at",
        params![
            entry.record_id.to_string(),
            entry.collection,
            entry.op_id.to_string(),
            entry.base_revision_hlc,
            entry.revision_hlc,
            state_kind,
            semantic_hlc,
            blob,
            entry.created_at,
        ],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO sync_record_origins
             (record_id, collection, origin_kind, updated_at)
         VALUES (?1, ?2, 'never_synced', ?3)",
        params![origin_record_id, origin_collection, origin_updated_at],
    )?;
    connection
        .query_row(
            "SELECT op_id, record_id, collection, base_revision_hlc,
                    revision_hlc, state_kind, semantic_hlc, blob, created_at
             FROM sync_outbox
             WHERE record_id = ?1",
            [entry.record_id.to_string()],
            row_to_sync_outbox_entry,
        )
        .map_err(StorageError::from)?
}

fn list_outbox_heads_on(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<SyncOutboxEntry>, StorageError> {
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("outbox limit exceeded i64".to_string()))?;
    let mut statement = connection.prepare(
        "SELECT op_id, record_id, collection, base_revision_hlc,
                revision_hlc, state_kind, semantic_hlc, blob, created_at
         FROM sync_outbox AS outbox
         WHERE NOT EXISTS (
             SELECT 1 FROM sync_quarantine AS quarantine
             WHERE quarantine.record_id = outbox.record_id
         )
         ORDER BY CASE collection WHEN 'lists' THEN 0 ELSE 1 END ASC,
                  created_at ASC, record_id ASC
         LIMIT ?1",
    )?;
    let entries = statement
        .query_map([limit], row_to_sync_outbox_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    entries.into_iter().collect()
}

fn put_quarantine_on(
    connection: &Connection,
    entry: SyncQuarantineEntry,
) -> Result<(), StorageError> {
    validate_sync_collection(&entry.collection)?;
    ensure_sync_collection_matches(connection, entry.record_id, &entry.collection)?;
    let existing = connection
        .query_row(
            "SELECT seq, revision_hlc FROM sync_quarantine WHERE record_id = ?1",
            [entry.record_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((existing_seq, existing_revision_hlc)) = existing {
        if entry.seq < existing_seq {
            return Ok(());
        }
        if entry.seq == existing_seq {
            if entry.revision_hlc != existing_revision_hlc {
                return Err(StorageError::IncompatibleSchema(
                    "quarantine revision changed at the same server sequence".to_string(),
                ));
            }
            connection.execute(
                "UPDATE sync_quarantine
                 SET reason = ?2,
                     required_list_id = ?3,
                     last_failed_at = ?4,
                     attempt_count = attempt_count + 1
                 WHERE record_id = ?1",
                params![
                    entry.record_id.to_string(),
                    entry.reason,
                    entry.required_list_id.map(|id| id.to_string()),
                    entry.last_failed_at,
                ],
            )?;
            return Ok(());
        }
    }
    let (state_kind, semantic_hlc, blob) = match entry.state {
        SyncOutboxState::Live { mutation_hlc, blob } => ("live", mutation_hlc, Some(blob)),
        SyncOutboxState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_quarantine (
             record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
             blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             seq = excluded.seq,
             revision_hlc = excluded.revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             blob = excluded.blob,
             reason = excluded.reason,
             required_list_id = excluded.required_list_id,
             first_failed_at = excluded.first_failed_at,
             last_failed_at = excluded.last_failed_at,
             attempt_count = excluded.attempt_count",
        params![
            entry.record_id.to_string(),
            entry.collection,
            entry.seq,
            entry.revision_hlc,
            state_kind,
            semantic_hlc,
            blob,
            entry.reason,
            entry.required_list_id.map(|id| id.to_string()),
            entry.first_failed_at,
            entry.last_failed_at,
            entry.attempt_count,
        ],
    )?;
    Ok(())
}

fn list_quarantine_on(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("quarantine limit exceeded i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
                blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         FROM sync_quarantine ORDER BY seq ASC, record_id ASC LIMIT ?1",
    )?;
    let entries = statement
        .query_map([limit], row_to_sync_quarantine)?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();
    entries
}

fn list_replayable_quarantine_on(
    connection: &Connection,
    after: Option<(i64, Uuid)>,
    limit: usize,
) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
    let limit = i64::try_from(limit).map_err(|_| {
        StorageError::IncompatibleSchema("quarantine replay limit exceeded i64".into())
    })?;
    let (after_seq, after_record_id) = after
        .map(|(seq, record_id)| (Some(seq), Some(record_id.to_string())))
        .unwrap_or((None, None));
    let mut statement = connection.prepare(
        "SELECT record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
                blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         FROM sync_quarantine
         WHERE reason IN ('missing_dek', 'no_matching_dek', 'missing_dependency')
           AND (?1 IS NULL OR seq > ?1 OR (seq = ?1 AND record_id > ?2))
         ORDER BY seq ASC, record_id ASC
         LIMIT ?3",
    )?;
    let entries = statement
        .query_map(
            params![after_seq, after_record_id, limit],
            row_to_sync_quarantine,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();
    entries
}

fn delete_quarantine_on(connection: &Connection, record_id: Uuid) -> Result<bool, StorageError> {
    Ok(connection.execute(
        "DELETE FROM sync_quarantine WHERE record_id = ?1",
        [record_id.to_string()],
    )? == 1)
}

fn row_to_sync_quarantine(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncQuarantineEntry, StorageError>> {
    let record_id = match Uuid::parse_str(&row.get::<_, String>(0)?) {
        Ok(value) => value,
        Err(error) => return Ok(Err(StorageError::InvalidUuid(error))),
    };
    let collection: String = row.get(1)?;
    let state_kind: String = row.get(4)?;
    let semantic_hlc: String = row.get(5)?;
    let blob: Option<Vec<u8>> = row.get(6)?;
    let state = match (state_kind.as_str(), blob) {
        ("live", Some(blob)) => SyncOutboxState::Live {
            mutation_hlc: semantic_hlc,
            blob,
        },
        ("tombstone", None) => SyncOutboxState::Tombstone {
            delete_hlc: semantic_hlc,
        },
        _ => return Ok(Err(StorageError::InvalidSyncState(state_kind))),
    };
    let required_list_id = match row.get::<_, Option<String>>(8)? {
        Some(value) => match Uuid::parse_str(&value) {
            Ok(value) => Some(value),
            Err(error) => return Ok(Err(StorageError::InvalidUuid(error))),
        },
        None => None,
    };
    Ok(Ok(SyncQuarantineEntry {
        record_id,
        collection,
        seq: row.get(2)?,
        revision_hlc: row.get(3)?,
        state,
        reason: row.get(7)?,
        required_list_id,
        first_failed_at: row.get(9)?,
        last_failed_at: row.get(10)?,
        attempt_count: row.get(11)?,
    }))
}

fn has_outbox_head_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    let existing = connection
        .query_row(
            "SELECT collection
             FROM sync_outbox
             WHERE record_id = ?1",
            [record_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        ensure_requested_collection(record_id, &existing, collection)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn ack_outbox_op_on(connection: &Connection, op_id: Uuid) -> Result<bool, StorageError> {
    let acked: Option<(String, String)> = connection
        .query_row(
            "SELECT record_id, collection FROM sync_outbox WHERE op_id = ?1",
            [op_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let changed = connection.execute(
        "DELETE FROM sync_outbox WHERE op_id = ?1",
        [op_id.to_string()],
    )?;
    if let Some((record_id, collection)) = acked {
        connection.execute(
            "INSERT INTO sync_record_origins (record_id, collection, origin_kind, updated_at)
             VALUES (?1, ?2, 'server_seen', unixepoch('subsec') * 1000)
             ON CONFLICT(record_id) DO UPDATE SET
                 origin_kind = 'server_seen', collection = excluded.collection,
                 updated_at = excluded.updated_at",
            params![record_id, collection],
        )?;
    }
    Ok(changed == 1)
}

fn delete_outbox_head_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    ensure_sync_collection_matches(connection, record_id, collection)?;
    let changed = connection.execute(
        "DELETE FROM sync_outbox WHERE collection = ?1 AND record_id = ?2",
        params![collection, record_id.to_string()],
    )?;
    Ok(changed == 1)
}

fn validate_sync_collection(collection: &str) -> Result<(), StorageError> {
    match collection {
        "lists" | "tasks" | "templates" | "task_series" | "timer_sessions" => Ok(()),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

fn ensure_sync_collection_matches(
    connection: &Connection,
    record_id: Uuid,
    requested: &str,
) -> Result<(), StorageError> {
    let existing = connection
        .query_row(
            "SELECT collection FROM sync_record_states WHERE record_id = ?1
             UNION ALL
             SELECT collection FROM sync_outbox WHERE record_id = ?1
             UNION ALL
             SELECT collection FROM sync_quarantine WHERE record_id = ?1
             LIMIT 1",
            [record_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        ensure_requested_collection(record_id, &existing, requested)?;
    }
    Ok(())
}

fn ensure_requested_collection(
    record_id: Uuid,
    existing: &str,
    requested: &str,
) -> Result<(), StorageError> {
    if existing == requested {
        Ok(())
    } else {
        Err(StorageError::SyncCollectionMismatch {
            record_id,
            existing: existing.to_string(),
            requested: requested.to_string(),
        })
    }
}

fn ensure_task_exists(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1 LIMIT 1",
            [task_id.to_string()],
            |_| Ok(()),
        )
        .optional()?;
    exists.ok_or(StorageError::NotFound(task_id))
}

fn list_task_reminders_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<Vec<Reminder>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, task_id, remind_at, snoozed_until, created_at
         FROM reminders
         WHERE task_id = ?1
         ORDER BY COALESCE(snoozed_until, remind_at) ASC, created_at ASC, id ASC",
    )?;
    let reminders = statement
        .query_map([task_id.to_string()], row_to_reminder)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(reminders)
}

fn get_reminder_on(connection: &Connection, reminder_id: Uuid) -> Result<Reminder, StorageError> {
    connection
        .query_row(
            "SELECT id, task_id, remind_at, snoozed_until, created_at
             FROM reminders
             WHERE id = ?1",
            [reminder_id.to_string()],
            row_to_reminder,
        )
        .optional()?
        .ok_or(StorageError::NotFound(reminder_id))
}

fn ensure_task_open_for_reminder(
    connection: &Connection,
    task_id: Uuid,
) -> Result<(), StorageError> {
    let state = connection
        .query_row(
            "SELECT status, deleted_at FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?
        .ok_or(StorageError::NotFound(task_id))?;
    if matches!(state.0.as_str(), "todo" | "in_progress") && state.1.is_none() {
        Ok(())
    } else {
        Err(StorageError::ReminderTaskClosed)
    }
}

fn validate_reminder_time(remind_at: i64, now_ms: i64) -> Result<(), StorageError> {
    if remind_at > now_ms {
        Ok(())
    } else {
        Err(StorageError::ReminderTimeNotFuture)
    }
}

fn ensure_unique_reminder_time_on(
    connection: &Connection,
    task_id: Uuid,
    remind_at: i64,
    excluding_id: Option<Uuid>,
) -> Result<(), StorageError> {
    let duplicate: bool = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM reminders
             WHERE task_id = ?1
               AND remind_at = ?2
               AND (?3 IS NULL OR id != ?3)
         )",
        params![
            task_id.to_string(),
            remind_at,
            excluding_id.map(|id| id.to_string())
        ],
        |row| row.get(0),
    )?;
    if duplicate {
        Err(StorageError::DuplicateReminderTime)
    } else {
        Ok(())
    }
}

fn insert_reminder_on(connection: &Connection, reminder: &Reminder) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO reminders (id, task_id, remind_at, snoozed_until, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            reminder.id.to_string(),
            reminder.task_id.to_string(),
            reminder.remind_at,
            reminder.snoozed_until,
            reminder.created_at,
        ],
    )?;
    Ok(())
}

fn delete_task_reminders_on(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    connection.execute(
        "DELETE FROM reminders WHERE task_id = ?1",
        [task_id.to_string()],
    )?;
    Ok(())
}

fn row_to_list(row: &rusqlite::Row<'_>) -> rusqlite::Result<List> {
    let id: String = row.get(0)?;

    Ok(List {
        id: parse_uuid(id, 0)?,
        name: row.get(1)?,
        color: row.get(2)?,
        icon: row.get(3)?,
        sort_order: row.get(4)?,
        archived_at: row.get(5)?,
        is_default: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let list_id: String = row.get(1)?;
    let parent_task_id: Option<String> = row.get(2)?;
    let status: String = row.get(5)?;
    let due_kind: Option<String> = row.get(7)?;
    let due_on: Option<String> = row.get(8)?;
    let due_at_ms: Option<i64> = row.get(9)?;
    let due_time_zone: Option<String> = row.get(10)?;
    let assignee: Option<String> = row.get(17)?;
    let series_id: Option<String> = row.get(18)?;
    let series_revision: Option<String> = row.get(19)?;
    let blueprint_node_key: Option<String> = row.get(20)?;
    let series_occurrence_at: Option<i64> = row.get(21)?;

    Ok(Task {
        id: parse_uuid(id, 0)?,
        list_id: parse_uuid(list_id, 1)?,
        parent_task_id: parse_optional_uuid(parent_task_id, 2)?,
        content: TaskContent {
            title: row.get(3)?,
            note: row.get(4)?,
            priority: row.get(6)?,
            estimated_minutes: row.get(12)?,
        },
        status: status_from_str(&status).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        due: task_due_from_columns(due_kind, due_on, due_at_ms, due_time_zone)?,
        scheduled_at: row.get(11)?,
        sort_order: row.get(13)?,
        completed_at: row.get(14)?,
        closed_reason: row.get(15)?,
        deleted_at: row.get(16)?,
        assignee: parse_optional_uuid(assignee, 17)?,
        series_occurrence: series_occurrence_from_columns(
            series_id,
            series_revision,
            blueprint_node_key,
            series_occurrence_at,
        )?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
    })
}

fn series_occurrence_from_columns(
    series_id: Option<String>,
    series_revision: Option<String>,
    blueprint_node_key: Option<String>,
    occurrence_at: Option<i64>,
) -> rusqlite::Result<Option<SeriesOccurrenceRef>> {
    match (
        series_id,
        series_revision,
        blueprint_node_key,
        occurrence_at,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(series_id), Some(series_revision), Some(blueprint_node_key), Some(occurrence_at)) => {
            Ok(Some(SeriesOccurrenceRef {
                series_id: parse_uuid(series_id, 18)?,
                series_revision,
                occurrence_at,
                blueprint_node_key,
            }))
        }
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            18,
            rusqlite::types::Type::Text,
            Box::new(StorageError::IncompatibleSchema(
                "partial task series occurrence provenance".to_string(),
            )),
        )),
    }
}

fn row_to_home_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<HomeTask> {
    Ok(HomeTask {
        task: row_to_task(row)?,
        list_name: row.get(24)?,
        is_home_target: row.get(25)?,
    })
}

fn task_due_parts(
    due: Option<&TaskDue>,
) -> (Option<&str>, Option<&str>, Option<i64>, Option<&str>) {
    match due {
        None => (None, None, None, None),
        Some(TaskDue::Date { due_on }) => (Some("date"), Some(due_on.as_str()), None, None),
        Some(TaskDue::DateTime { due_at, time_zone }) => (
            Some("datetime"),
            None,
            Some(due_at.as_millis()),
            Some(time_zone.as_str()),
        ),
    }
}

fn task_due_from_columns(
    kind: Option<String>,
    due_on: Option<String>,
    due_at_ms: Option<i64>,
    due_time_zone: Option<String>,
) -> rusqlite::Result<Option<TaskDue>> {
    let invalid_shape = || {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid task due column shape",
            )),
        )
    };
    match (kind.as_deref(), due_on, due_at_ms, due_time_zone) {
        (None, None, None, None) => Ok(None),
        (Some("date"), Some(value), None, None) => CivilDate::from_str(&value)
            .map(|due_on| Some(TaskDue::Date { due_on }))
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            }),
        (Some("datetime"), None, Some(value), Some(zone)) => {
            let due_at = UtcInstant::from_millis(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            let time_zone = IanaTimeZone::from_str(&zone).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(Some(TaskDue::DateTime { due_at, time_zone }))
        }
        _ => Err(invalid_shape()),
    }
}

fn row_to_reminder(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reminder> {
    let id: String = row.get(0)?;
    let task_id: String = row.get(1)?;
    Ok(Reminder {
        id: parse_uuid(id, 0)?,
        task_id: parse_uuid(task_id, 1)?,
        remind_at: row.get(2)?,
        snoozed_until: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn row_to_sync_outbox_entry(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncOutboxEntry, StorageError>> {
    let op_id: String = row.get(0)?;
    let record_id: String = row.get(1)?;
    let state_kind: String = row.get(5)?;
    let semantic_hlc: String = row.get(6)?;
    let blob: Option<Vec<u8>> = row.get(7)?;
    Ok((|| {
        let state = match (state_kind.as_str(), blob) {
            ("live", Some(blob)) => SyncOutboxState::Live {
                mutation_hlc: semantic_hlc,
                blob,
            },
            ("tombstone", None) => SyncOutboxState::Tombstone {
                delete_hlc: semantic_hlc,
            },
            (kind, _) => return Err(StorageError::InvalidSyncState(kind.to_string())),
        };
        Ok(SyncOutboxEntry {
            op_id: Uuid::from_str(&op_id)?,
            record_id: Uuid::from_str(&record_id)?,
            collection: row.get(2)?,
            base_revision_hlc: row.get(3)?,
            revision_hlc: row.get(4)?,
            state,
            created_at: row.get(8)?,
        })
    })())
}

fn row_to_sync_record_state(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncRecordState, StorageError>> {
    let record_id: String = row.get(0)?;
    let state_kind: String = row.get(3)?;
    let semantic_hlc: String = row.get(4)?;
    let plaintext_json: Option<String> = row.get(5)?;
    Ok((|| {
        let state = match (state_kind.as_str(), plaintext_json) {
            ("live", Some(plaintext_json)) => SyncRecordSemanticState::Live {
                mutation_hlc: semantic_hlc,
                plaintext_json,
            },
            ("tombstone", None) => SyncRecordSemanticState::Tombstone {
                delete_hlc: semantic_hlc,
            },
            (kind, _) => return Err(StorageError::InvalidSyncState(kind.to_string())),
        };
        Ok(SyncRecordState {
            record_id: Uuid::from_str(&record_id)?,
            collection: row.get(1)?,
            current_revision_hlc: row.get(2)?,
            state,
            updated_at: row.get(6)?,
        })
    })())
}

fn row_to_sync_cursor(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncCursor> {
    Ok(SyncCursor {
        name: row.get(0)?,
        seq: row.get(1)?,
        updated_at: row.get(2)?,
    })
}

fn row_to_task_undo_entry(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<TaskUndoEntry, StorageError>> {
    let id: String = row.get(0)?;
    let operation_type: String = row.get(1)?;
    let task_id: String = row.get(2)?;
    let list_id: String = row.get(3)?;
    let before_snapshot: String = row.get(4)?;

    Ok((|| {
        Ok(TaskUndoEntry {
            id: Uuid::from_str(&id)?,
            operation_type: undo_operation_from_str(&operation_type)?,
            task_id: Uuid::from_str(&task_id)?,
            list_id: Uuid::from_str(&list_id)?,
            before_snapshot: serde_json::from_str(&before_snapshot)?,
            after_updated_at: row.get(5)?,
            after_deleted_at: row.get(6)?,
            after_completed_at: row.get(7)?,
            created_at: row.get(8)?,
            consumed_at: row.get(9)?,
        })
    })())
}

fn parse_uuid(value: String, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_optional_uuid(value: Option<String>, column: usize) -> rusqlite::Result<Option<Uuid>> {
    value.map(|value| parse_uuid(value, column)).transpose()
}

fn status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::WontDo => "wont_do",
    }
}

fn status_from_str(value: &str) -> Result<TaskStatus, StorageError> {
    match value {
        "todo" => Ok(TaskStatus::Todo),
        "in_progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        "wont_do" => Ok(TaskStatus::WontDo),
        other => Err(StorageError::InvalidStatus(other.to_string())),
    }
}

fn undo_operation_to_str(operation_type: TaskUndoOperation) -> &'static str {
    match operation_type {
        TaskUndoOperation::Delete => "delete",
        TaskUndoOperation::Complete => "complete",
        TaskUndoOperation::Edit => "edit",
    }
}

fn undo_operation_from_str(value: &str) -> Result<TaskUndoOperation, StorageError> {
    match value {
        "delete" => Ok(TaskUndoOperation::Delete),
        "complete" => Ok(TaskUndoOperation::Complete),
        "edit" => Ok(TaskUndoOperation::Edit),
        other => Err(StorageError::InvalidUndoOperation(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskveil_crypto::{derive_local_db_key, ensure_device_key, InMemoryDeviceKeyStore};
    use taskveil_domain::{
        new_list, new_task, transition_task, update_title, TaskBlueprint, TaskBlueprintNode,
        TaskContent, TaskSeriesConfig, TASK_BLUEPRINT_SCHEMA_REVISION,
    };
    use tempfile::NamedTempFile;

    const KEY: [u8; 32] = [0x11; 32];
    const WRONG_KEY: [u8; 32] = [0x22; 32];

    fn local_tenant_root_bundle(tenant_id: Uuid, updated_at: i64) -> LocalTenantRootKeyBundle {
        LocalTenantRootKeyBundle {
            tenant_id,
            generation: 1,
            wrapped_tenant_root_dek: vec![0x5a; 48],
            updated_at,
        }
    }

    fn sample_task() -> Task {
        Task {
            id: Uuid::now_v7(),
            list_id: Uuid::now_v7(),
            parent_task_id: Some(Uuid::now_v7()),
            content: TaskContent {
                title: "Buy milk".to_string(),
                note: "Organic whole milk".to_string(),
                priority: 2,
                estimated_minutes: Some(15),
            },
            status: TaskStatus::Todo,
            due: Some(TaskDue::date_time(1_800_000_000_000, "UTC").unwrap()),
            scheduled_at: Some(1_799_900_000_000),
            sort_order: "a0".to_string(),
            completed_at: None,
            closed_reason: None,
            deleted_at: None,
            assignee: Some(Uuid::now_v7()),
            series_occurrence: None,
            created_at: 1_799_000_000_000,
            updated_at: 1_799_000_000_000,
        }
    }

    fn insert_task_pre_v20(connection: &Connection, task: &Task) {
        let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
        connection
            .execute(
                "INSERT INTO tasks (
                     id, list_id, parent_task_id, title, note, status, priority,
                     due_kind, due_on, due_at_ms, due_time_zone, scheduled_at,
                     estimated_minutes, sort_order, completed_at, closed_reason,
                     deleted_at, assignee, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                 )",
                params![
                    task.id.to_string(),
                    task.list_id.to_string(),
                    task.parent_task_id.map(|id| id.to_string()),
                    task.content.title,
                    task.content.note,
                    status_to_str(task.status),
                    task.content.priority,
                    due_kind,
                    due_on,
                    due_at_ms,
                    due_time_zone,
                    task.scheduled_at,
                    task.content.estimated_minutes,
                    task.sort_order,
                    task.completed_at,
                    task.closed_reason,
                    task.deleted_at,
                    task.assignee.map(|id| id.to_string()),
                    task.created_at,
                    task.updated_at,
                ],
            )
            .unwrap();
    }

    fn sample_list(sort_order: &str) -> List {
        List {
            id: Uuid::now_v7(),
            name: format!("List {sort_order}"),
            color: "#4F8EF7".to_string(),
            icon: "list".to_string(),
            sort_order: sort_order.to_string(),
            is_default: false,
            archived_at: None,
            created_at: 1_799_000_000_000,
            updated_at: 1_799_000_000_000,
        }
    }

    fn sample_template() -> TaskTemplate {
        TaskTemplate {
            id: Uuid::now_v7(),
            name: "Template".to_string(),
            default_list_id: None,
            blueprint: TaskBlueprint {
                schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                nodes: vec![TaskBlueprintNode {
                    node_key: "root".to_string(),
                    parent_node_key: None,
                    sibling_order: 0,
                    content: TaskContent {
                        title: "Generated task".to_string(),
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

    fn sample_schedule(template_id: Uuid) -> TaskSeries {
        TaskSeries {
            id: Uuid::now_v7(),
            config: TaskSeriesConfig {
                blueprint: {
                    let mut blueprint = sample_template().blueprint;
                    blueprint.nodes[0].node_key = template_id.to_string();
                    blueprint
                },
                target_list_id: None,
                rrule: "FREQ=DAILY".to_string(),
                starts_at: 1_800_000_000_000,
                time_zone: "UTC".to_string(),
                enabled: true,
                config_revision: "schedule-r1".to_string(),
                config_parent_revision: None,
                config_effective_from: 1,
                lineage: Vec::new(),
            },
            cursor: SeriesCursor::Pending(1_800_000_000_000),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn new_live_outbox(
        record_id: Uuid,
        collection: &str,
        op_id: Uuid,
        base_revision_hlc: Option<&str>,
        revision_hlc: &str,
        mutation_hlc: &str,
        blob: Vec<u8>,
    ) -> NewSyncOutboxEntry {
        NewSyncOutboxEntry {
            op_id,
            record_id,
            collection: collection.to_string(),
            base_revision_hlc: base_revision_hlc.map(str::to_string),
            revision_hlc: revision_hlc.to_string(),
            state: SyncOutboxState::Live {
                mutation_hlc: mutation_hlc.to_string(),
                blob,
            },
            created_at: 1_799_000_000_000,
        }
    }

    fn live_record_state(
        record_id: Uuid,
        collection: &str,
        current_revision_hlc: Option<&str>,
        mutation_hlc: &str,
        plaintext_json: &str,
        updated_at: i64,
    ) -> SyncRecordState {
        SyncRecordState {
            record_id,
            collection: collection.to_string(),
            current_revision_hlc: current_revision_hlc.map(str::to_string),
            state: SyncRecordSemanticState::Live {
                mutation_hlc: mutation_hlc.to_string(),
                plaintext_json: plaintext_json.to_string(),
            },
            updated_at,
        }
    }

    fn open_raw_encrypted(path: &Path, key: &[u8; 32]) -> Connection {
        let connection = Connection::open(path).unwrap();
        apply_sqlcipher_key(&connection, key).unwrap();
        connection
    }

    fn create_baseline_v1_database(path: &Path, key: &[u8; 32], set_version: bool) {
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(SCHEMA).unwrap();
        if set_version {
            set_user_version(&transaction, BASELINE_SCHEMA_VERSION).unwrap();
        }
        transaction.commit().unwrap();
    }

    fn insert_baseline_v1_list(connection: &Connection, list: &List) {
        connection
            .execute(
                "INSERT INTO lists (
                    id, name, color, icon, org_id, sort_order, created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
                )",
                params![
                    list.id.to_string(),
                    list.name,
                    list.color,
                    list.icon,
                    Option::<String>::None,
                    list.sort_order,
                    list.created_at,
                    list.updated_at,
                ],
            )
            .unwrap();
    }

    fn create_v2_database(path: &Path, key: &[u8; 32]) {
        create_baseline_v1_database(path, key, true);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_lists_archived_at(&transaction).unwrap();
        set_user_version(&transaction, 2).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v3_database(path: &Path, key: &[u8; 32]) {
        create_v2_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_lists_is_default(&transaction).unwrap();
        set_user_version(&transaction, 3).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v4_database(path: &Path, key: &[u8; 32]) {
        create_v3_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        rebuild_tasks_fts_triggers(&transaction).unwrap();
        set_user_version(&transaction, 4).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v5_database(path: &Path, key: &[u8; 32]) {
        create_v4_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_settings(&transaction).unwrap();
        set_user_version(&transaction, 5).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v6_database(path: &Path, key: &[u8; 32]) {
        create_v5_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_reminders(&transaction).unwrap();
        set_user_version(&transaction, 6).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v7_database(path: &Path, key: &[u8; 32]) {
        create_v6_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_performance_indexes(&transaction).unwrap();
        set_user_version(&transaction, 7).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v9_database(path: &Path, key: &[u8; 32]) {
        create_v7_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_sync_outbox_and_cursors(&transaction).unwrap();
        set_user_version(&transaction, 8).unwrap();
        add_sync_record_states(&transaction).unwrap();
        set_user_version(&transaction, 9).unwrap();
        transaction.commit().unwrap();
    }

    fn create_v10_database(path: &Path, key: &[u8; 32]) {
        create_v9_database(path, key);
        let mut connection = open_raw_encrypted(path, key);
        let transaction = connection.transaction().unwrap();
        add_local_crypto_cache(&transaction).unwrap();
        set_user_version(&transaction, 10).unwrap();
        transaction.commit().unwrap();
    }

    fn insert_v2_list(connection: &Connection, list: &List) {
        connection
            .execute(
                "INSERT INTO lists (
                    id, name, color, icon, org_id, sort_order, archived_at,
                    created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
                )",
                params![
                    list.id.to_string(),
                    list.name,
                    list.color,
                    list.icon,
                    Option::<String>::None,
                    list.sort_order,
                    list.archived_at,
                    list.created_at,
                    list.updated_at,
                ],
            )
            .unwrap();
    }

    fn list_column(connection: &Connection, target: &str) -> Option<(String, i32, String)> {
        let mut statement = connection.prepare("PRAGMA table_info(lists)").unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .find_map(|(name, column_type, not_null, default_value)| {
                (name == target).then_some((column_type, not_null, default_value))
            })
    }

    fn archived_at_column(connection: &Connection) -> Option<(String, i32)> {
        list_column(connection, "archived_at")
            .map(|(column_type, not_null, _)| (column_type, not_null))
    }

    fn is_default_column(connection: &Connection) -> Option<(String, i32, String)> {
        list_column(connection, "is_default")
    }

    fn index_exists(connection: &Connection, index_name: &str) -> bool {
        connection
            .query_row(
                "SELECT 1
                 FROM sqlite_master
                 WHERE type = 'index' AND name = ?1
                 LIMIT 1",
                [index_name],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some()
    }

    fn setting_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
        let mut statement = connection.prepare("PRAGMA table_info(settings)").unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .find_map(|(name, column_type, not_null)| {
                (name == target).then_some((column_type, not_null))
            })
    }

    fn reminder_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
        let mut statement = connection.prepare("PRAGMA table_info(reminders)").unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .find_map(|(name, column_type, not_null)| {
                (name == target).then_some((column_type, not_null))
            })
    }

    fn sync_outbox_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
        let mut statement = connection
            .prepare("PRAGMA table_info(sync_outbox)")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .find_map(|(name, column_type, not_null)| {
                (name == target).then_some((column_type, not_null))
            })
    }

    fn sync_cursor_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
        let mut statement = connection
            .prepare("PRAGMA table_info(sync_cursors)")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .find_map(|(name, column_type, not_null)| {
                (name == target).then_some((column_type, not_null))
            })
    }

    fn count_archived_at_columns(connection: &Connection) -> usize {
        let mut statement = connection.prepare("PRAGMA table_info(lists)").unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .filter(|column| column == "archived_at")
            .count()
    }

    fn schema_version(connection: &Connection) -> i32 {
        connection
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .unwrap()
    }

    #[derive(Clone, Copy)]
    enum PerformanceSeedSchema {
        Latest,
        V3,
    }

    struct PerformanceSeed {
        list_ids: Vec<Uuid>,
        today_start_ms: i64,
        tomorrow_start_ms: i64,
        task_count: usize,
        due_task_count: usize,
        closed_task_count: usize,
    }

    fn seed_performance_database(
        path: &Path,
        key: &[u8; 32],
        schema: PerformanceSeedSchema,
    ) -> PerformanceSeed {
        match schema {
            PerformanceSeedSchema::Latest => {
                let mut connection = open_encrypted(path, key).unwrap();
                insert_performance_seed(&mut connection)
            }
            PerformanceSeedSchema::V3 => {
                create_v3_database(path, key);
                let mut connection = open_raw_encrypted(path, key);
                insert_performance_seed(&mut connection)
            }
        }
    }

    fn insert_performance_seed(connection: &mut Connection) -> PerformanceSeed {
        const LIST_COUNT: usize = 10;
        const TASKS_PER_LIST: usize = 1_000;
        const ROOT_TASKS_PER_LIST: usize = 700;
        const CHILD_TASKS_PER_LIST: usize = 220;

        let today_start_ms = 1_788_220_800_000;
        let tomorrow_start_ms = today_start_ms + 86_400_000;
        let mut list_ids = Vec::with_capacity(LIST_COUNT);
        let mut due_task_count = 0;
        let mut closed_task_count = 0;
        let transaction = connection.transaction().unwrap();

        {
            let mut insert_list = transaction
                .prepare(
                    "INSERT INTO lists (
                        id, name, color, icon, sort_order, is_default,
                        archived_at, created_at, updated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
                    )",
                )
                .unwrap();
            for list_index in 0..LIST_COUNT {
                let id = Uuid::now_v7();
                list_ids.push(id);
                insert_list
                    .execute(params![
                        id.to_string(),
                        format!("Performance List {}", list_index + 1),
                        "#4F8EF7",
                        "list",
                        format!("a{list_index:02}"),
                        list_index == 0,
                        Option::<i64>::None,
                        today_start_ms - 86_400_000,
                        today_start_ms - 86_400_000,
                    ])
                    .unwrap();
            }
        }

        {
            let mut insert_task = transaction
                .prepare(
                    "INSERT INTO tasks (
                        id, list_id, parent_task_id, title, note, status, priority,
                        due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                        completed_at, closed_reason, deleted_at, assignee,
                        created_at, updated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                    )",
                )
                .unwrap();

            for (list_index, list_id) in list_ids.iter().copied().enumerate() {
                let mut root_ids = Vec::with_capacity(ROOT_TASKS_PER_LIST);
                let mut child_ids = Vec::with_capacity(CHILD_TASKS_PER_LIST);
                for task_index in 0..TASKS_PER_LIST {
                    let id = Uuid::now_v7();
                    let parent_task_id = if task_index < ROOT_TASKS_PER_LIST {
                        root_ids.push(id);
                        None
                    } else if task_index < ROOT_TASKS_PER_LIST + CHILD_TASKS_PER_LIST {
                        let parent_id =
                            root_ids[(task_index - ROOT_TASKS_PER_LIST) % root_ids.len()];
                        child_ids.push(id);
                        Some(parent_id)
                    } else {
                        Some(
                            child_ids[(task_index - ROOT_TASKS_PER_LIST - CHILD_TASKS_PER_LIST)
                                % child_ids.len()],
                        )
                    };
                    let global_index = (list_index * TASKS_PER_LIST) + task_index;
                    let status = match global_index % 10 {
                        0 => "done",
                        1 => "wont_do",
                        2 | 3 => "in_progress",
                        _ => "todo",
                    };
                    let due_at = match global_index % 6 {
                        0 => None,
                        1 => Some(today_start_ms - 86_400_000),
                        2 => Some(today_start_ms + ((global_index % 12) as i64 * 3_600_000)),
                        3 => Some(tomorrow_start_ms + ((global_index % 8) as i64 * 3_600_000)),
                        4 => Some(tomorrow_start_ms + 7 * 86_400_000),
                        _ => None,
                    };
                    if due_at.is_some() {
                        due_task_count += 1;
                    }
                    let is_closed = status == "done" || status == "wont_do";
                    let completed_at = if is_closed {
                        closed_task_count += 1;
                        if global_index.is_multiple_of(4) {
                            Some(today_start_ms + ((global_index % 10) as i64 * 600_000))
                        } else {
                            Some(today_start_ms - 2 * 86_400_000)
                        }
                    } else {
                        None
                    };
                    let keyword = if global_index.is_multiple_of(17) {
                        "alpha"
                    } else if global_index.is_multiple_of(19) {
                        "日本語"
                    } else {
                        "routine"
                    };

                    insert_task
                        .execute(params![
                            id.to_string(),
                            list_id.to_string(),
                            parent_task_id.map(|parent_id| parent_id.to_string()),
                            format!("Task {global_index:05} {keyword}"),
                            format!("Seeded note {global_index:05} for {keyword} project"),
                            status,
                            (global_index % 4) as i32,
                            due_at.map(|_| "datetime"),
                            Option::<String>::None,
                            due_at,
                            due_at.map(|_| "UTC"),
                            due_at.map(|value| value - 3_600_000),
                            Some(15 + (global_index % 6) as i32 * 10),
                            format!("a{task_index:04}"),
                            completed_at,
                            (status == "wont_do").then_some("not_now".to_string()),
                            Option::<i64>::None,
                            Option::<String>::None,
                            today_start_ms - 86_400_000 + global_index as i64,
                            today_start_ms - 43_200_000 + global_index as i64,
                        ])
                        .unwrap();
                }
            }
        }

        transaction.commit().unwrap();

        PerformanceSeed {
            list_ids,
            today_start_ms,
            tomorrow_start_ms,
            task_count: LIST_COUNT * TASKS_PER_LIST,
            due_task_count,
            closed_task_count,
        }
    }

    fn default_list_ids(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT id FROM lists WHERE is_default = 1 ORDER BY id ASC")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn failing_archived_at_migration(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
        transaction.execute_batch(
            "ALTER TABLE lists ADD COLUMN archived_at INTEGER NULL;
             SELECT value FROM missing_failure_injection_table;",
        )
    }

    #[test]
    fn encrypted_database_reopens_with_correct_key() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            insert_task_pre_v20(&connection, &task);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        assert_eq!(repository.get(task.id).unwrap(), task);
    }

    #[test]
    fn encrypted_database_rejects_wrong_key_on_query() {
        let file = NamedTempFile::new().unwrap();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteTaskRepository::new(connection);
            repository.insert(sample_task()).unwrap();
        }

        let result = open_encrypted(file.path(), &WRONG_KEY);

        match result {
            Err(StorageError::InvalidDatabaseKey) => {}
            Err(error) => panic!("expected invalid database key error, got {error}"),
            Ok(_) => panic!("database opened with wrong key"),
        }
    }

    #[test]
    fn sqlcipher_rekey_preserves_data_and_rejects_the_old_key() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteTaskRepository::new(connection)
                .insert(task.clone())
                .unwrap();
        }

        rekey_encrypted_database(file.path(), &KEY, &WRONG_KEY).unwrap();

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::InvalidDatabaseKey)
        ));
        let repository =
            SqliteTaskRepository::new(open_encrypted(file.path(), &WRONG_KEY).unwrap());
        assert_eq!(repository.get(task.id).unwrap(), task);
    }

    #[test]
    fn device_key_store_derived_key_reopens_database_and_rejects_other_device_key() {
        let file = NamedTempFile::new().unwrap();
        let mut store = InMemoryDeviceKeyStore::new();
        let task = sample_task();

        {
            let device_key = ensure_device_key(&mut store).unwrap();
            let db_key = derive_local_db_key(&device_key);
            let connection = open_encrypted(file.path(), &db_key).unwrap();
            insert_task_pre_v20(&connection, &task);
        }

        {
            let device_key = ensure_device_key(&mut store).unwrap();
            let db_key = derive_local_db_key(&device_key);
            let connection = open_encrypted(file.path(), &db_key).unwrap();
            let repository = SqliteTaskRepository::new(connection);
            assert_eq!(repository.get(task.id).unwrap(), task);
        }

        let mut other_store = InMemoryDeviceKeyStore::new();
        let other_device_key = ensure_device_key(&mut other_store).unwrap();
        let other_db_key = derive_local_db_key(&other_device_key);

        assert!(open_encrypted(file.path(), &other_db_key).is_err());
    }

    #[test]
    fn encrypted_database_is_not_plain_sqlite() {
        let file = NamedTempFile::new().unwrap();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteTaskRepository::new(connection);
            repository.insert(sample_task()).unwrap();
        }

        let plain = Connection::open(file.path()).unwrap();
        let result: rusqlite::Result<i64> =
            plain.query_row("SELECT count(*) FROM tasks", [], |row| row.get(0));

        assert!(result.is_err());
    }

    #[test]
    fn fts5_search_matches_title_and_note() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let mut task = sample_task();
        task.content.title = "Plan Kyoto trip".to_string();
        task.content.note = "Book shinkansen tickets".to_string();

        repository.insert(task.clone()).unwrap();

        assert_eq!(
            repository.search_tasks("kyoto").unwrap(),
            vec![task.clone()]
        );
        assert_eq!(repository.search_tasks("shinkansen").unwrap(), vec![task]);
        assert!(repository.search_tasks("").unwrap().is_empty());
    }

    #[test]
    fn fts5_search_tracks_title_note_updates_and_deleted_at() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let mut task = sample_task();
        task.content.title = "Draft itinerary".to_string();
        task.content.note = "Reserve hotel".to_string();
        repository.insert(task.clone()).unwrap();

        assert_eq!(
            repository.search_tasks("hotel").unwrap(),
            vec![task.clone()]
        );

        let mut updated = task.clone();
        updated.content.title = "Final packing list".to_string();
        updated.content.note = "Bring passport".to_string();
        updated.updated_at += 1;
        repository.update(updated.clone()).unwrap();

        assert!(repository.search_tasks("hotel").unwrap().is_empty());
        assert_eq!(
            repository.search_tasks("passport").unwrap(),
            vec![updated.clone()]
        );

        let mut deleted = updated.clone();
        deleted.deleted_at = Some(updated.updated_at + 1);
        deleted.updated_at += 1;
        repository.update(deleted.clone()).unwrap();

        assert!(repository.search_tasks("passport").unwrap().is_empty());

        let mut restored = deleted.clone();
        restored.deleted_at = None;
        restored.updated_at += 1;
        repository.update(restored.clone()).unwrap();

        assert_eq!(repository.search_tasks("passport").unwrap(), vec![restored]);
    }

    #[test]
    fn fts5_search_tracks_task_delete_and_list_rehoming() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut default_list = sample_list("a1");
        default_list.is_default = true;
        let mut kept = sample_task();
        kept.list_id = list.id;
        kept.content.title = "Keep searchable".to_string();
        kept.content.note = "retained".to_string();
        kept.sort_order = "a0".to_string();
        let mut task_deleted_by_subtree = sample_task();
        task_deleted_by_subtree.list_id = list.id;
        task_deleted_by_subtree.content.title = "Delete searchable subtree".to_string();
        task_deleted_by_subtree.content.note = "temporary".to_string();
        task_deleted_by_subtree.sort_order = "a1".to_string();
        let mut task_deleted_by_list = sample_task();
        task_deleted_by_list.list_id = list.id;
        task_deleted_by_list.content.title = "Delete searchable list".to_string();
        task_deleted_by_list.content.note = "temporary".to_string();
        task_deleted_by_list.sort_order = "a2".to_string();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
            list_repository.insert(default_list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(kept.clone()).unwrap();
            task_repository
                .insert(task_deleted_by_subtree.clone())
                .unwrap();
            task_repository
                .insert(task_deleted_by_list.clone())
                .unwrap();
            assert_eq!(task_repository.search_tasks("searchable").unwrap().len(), 3);

            task_repository
                .delete_subtree(task_deleted_by_subtree.id)
                .unwrap();
            let titles = task_repository
                .search_tasks("searchable")
                .unwrap()
                .into_iter()
                .map(|task| task.content.title)
                .collect::<Vec<_>>();
            let mut titles = titles;
            titles.sort();
            assert_eq!(titles, vec!["Delete searchable list", "Keep searchable"]);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.delete_and_rehome_tasks(list.id).unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let task_repository = SqliteTaskRepository::new(connection);
        let mut remaining = task_repository
            .search_tasks("searchable")
            .unwrap()
            .into_iter()
            .map(|task| (task.content.title, task.list_id))
            .collect::<Vec<_>>();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                ("Delete searchable list".to_string(), default_list.id),
                ("Keep searchable".to_string(), default_list.id),
            ]
        );
    }

    #[test]
    fn fts5_search_supports_english_and_japanese_prefix_queries() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let mut english = sample_task();
        english.content.title = "Buy milk".to_string();
        english.content.note = "Organic whole milk".to_string();
        english.updated_at = 1_799_000_000_000;
        let mut japanese = sample_task();
        japanese.content.title = "牛乳を買う".to_string();
        japanese.content.note = "明日の朝".to_string();
        japanese.updated_at = 1_799_000_001_000;
        repository.insert(english.clone()).unwrap();
        repository.insert(japanese.clone()).unwrap();

        assert_eq!(repository.search_tasks("milk").unwrap(), vec![english]);
        assert_eq!(
            repository.search_tasks("牛乳").unwrap(),
            vec![japanese.clone()]
        );
        assert_eq!(repository.search_tasks("明日").unwrap(), vec![japanese]);
        assert!(repository.search_tasks("乳").unwrap().is_empty());
    }

    #[test]
    fn v3_database_migrates_to_v4_and_backfills_tasks_fts() {
        let file = NamedTempFile::new().unwrap();
        create_v3_database(file.path(), &KEY);
        let mut task = sample_task();
        task.content.title = "Legacy searchable task".to_string();
        task.content.note = "Backfill target".to_string();
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_task_pre_v20(&connection, &task);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        task.sort_order = "00000000000000010000000000000000".to_string();

        assert_eq!(
            read_user_version(repository.connection()).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(repository.search_tasks("backfill").unwrap(), vec![task]);
    }

    #[test]
    fn v6_database_migrates_to_v7_and_adds_performance_indexes() {
        let file = NamedTempFile::new().unwrap();
        create_v6_database(file.path(), &KEY);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert!(index_exists(&connection, "idx_tasks_list_sort_order"));
        assert!(index_exists(&connection, "idx_tasks_home_targets"));
    }

    #[test]
    fn v7_database_migrates_to_latest_sync_state_tables() {
        let file = NamedTempFile::new().unwrap();
        create_v7_database(file.path(), &KEY);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            sync_outbox_column(&connection, "record_id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "collection"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "op_id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "base_revision_hlc"),
            Some(("TEXT".to_string(), 0))
        );
        assert_eq!(
            sync_outbox_column(&connection, "revision_hlc"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "state_kind"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "blob"),
            Some(("BLOB".to_string(), 0))
        );
        assert_eq!(
            sync_cursor_column(&connection, "seq"),
            Some(("INTEGER".to_string(), 1))
        );
        assert!(index_exists(&connection, "idx_sync_outbox_stable_order"));
    }

    #[test]
    fn v9_database_migrates_to_latest_and_adds_local_crypto_cache() {
        let file = NamedTempFile::new().unwrap();
        create_v9_database(file.path(), &KEY);
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        let connection = open_raw_encrypted(file.path(), &KEY);
        for (key, value, updated_at) in [
            ("account_tenant_id", tenant_id.to_string(), 100),
            ("account_user_id", user_id.to_string(), 200),
            ("account_device_id", device_id.to_string(), 300),
        ] {
            connection
                .execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
                    params![key, value, updated_at],
                )
                .unwrap();
        }
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            table_columns(&connection, "local_profile_binding").unwrap(),
            vec![
                "singleton",
                "tenant_id",
                "user_id",
                "device_id",
                "bound_at",
                "updated_at",
            ]
        );
        assert!(table_columns(&connection, "local_list_key_bundles")
            .unwrap()
            .is_empty());
        assert!(!table_columns(&connection, "lists")
            .unwrap()
            .iter()
            .any(|column| column == "org_id"));
        assert!(!index_exists(
            &connection,
            "idx_local_list_key_bundles_tenant"
        ));
        assert_eq!(
            SqliteLocalCryptoRepository::new(connection)
                .load_binding()
                .unwrap(),
            Some(LocalProfileBinding {
                tenant_id,
                user_id,
                device_id,
                bound_at: 100,
                updated_at: 300,
            })
        );
    }

    #[test]
    fn v10_migration_discards_v1_sync_metadata_but_preserves_domain_rows() {
        let file = NamedTempFile::new().unwrap();
        create_v10_database(file.path(), &KEY);
        let list = sample_list("a0");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            let mut repository = SqliteListRepository::new(connection);
            repository.insert(list.clone()).unwrap();
        }
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_task_pre_v20(&connection, &task);
        }
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            connection
                .execute(
                    "INSERT INTO sync_outbox (
                         record_id, collection, hlc, deleted, blob, created_at
                     ) VALUES (?1, 'tasks', 'v1-hlc', 0, X'01', 1)",
                    [task.id.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO sync_record_states (
                         record_id, collection, plaintext_json, updated_at
                     ) VALUES (?1, 'tasks', '{}', 1)",
                    [task.id.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO sync_cursors (name, seq, updated_at)
                     VALUES ('default', 99, 1)",
                    [],
                )
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut migrated_list = list.clone();
        migrated_list.sort_order = "00000000000000010000000000000000".to_string();
        let mut migrated_task = task.clone();
        migrated_task.sort_order = "00000000000000010000000000000000".to_string();
        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(get_list_on(&connection, list.id).unwrap(), migrated_list);
        assert_eq!(get_task_on(&connection, task.id).unwrap(), migrated_task);
        let repository = SqliteSyncStateRepository::new(connection);
        assert!(repository.list_outbox_heads(10).unwrap().is_empty());
        assert_eq!(repository.get_record_state("tasks", task.id).unwrap(), None);
        assert_eq!(repository.get_cursor("default").unwrap(), None);
    }

    #[test]
    fn local_crypto_cache_roundtrips_tenant_root_and_rejects_rebinding() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let tenant_id = Uuid::now_v7();
        let other_tenant_id = Uuid::now_v7();
        let binding = LocalProfileBinding {
            tenant_id,
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            bound_at: 10,
            updated_at: 10,
        };
        let mut repository = SqliteLocalCryptoRepository::new(connection);
        repository
            .bind_tenant_root(binding.clone(), &local_tenant_root_bundle(tenant_id, 10))
            .unwrap();

        assert_eq!(repository.load_binding().unwrap(), Some(binding));
        assert_eq!(
            repository.load_tenant_root(tenant_id).unwrap(),
            Some(local_tenant_root_bundle(tenant_id, 10))
        );
        assert!(matches!(
            repository.load_tenant_root(other_tenant_id),
            Err(StorageError::LocalProfileTenantMismatch { .. })
        ));
    }

    #[test]
    fn timer_v18_repository_restores_singleton_and_rejects_immutable_conflicts() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
        let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
        let task_id = task.id;
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteListRepository::new(connection).insert(list).unwrap();
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteTaskRepository::new(connection).insert(task).unwrap();
        }
        let active = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task_id),
            mode: TimerMode::Stopwatch,
            phase: TimerPhase::Work,
            state: TimerRunState::Running,
            started_at: 1_000,
            last_resumed_at: Some(1_100),
            accumulated_active_ms: 500,
            target_duration_ms: None,
        };
        let completed = CompletedTimerSession {
            id: Uuid::now_v7(),
            task_id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: 1_000,
            ended_at: 5_000,
            active_duration_ms: 3_000,
            created_at: 5_100,
        };

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                    .unwrap(),
                LATEST_SCHEMA_VERSION
            );
            let mut repository = SqliteTimerSessionRepository::new(connection);
            repository.start_active(active.clone(), 1_200).unwrap();
            assert!(repository.insert_completed(completed.clone()).unwrap());
            assert!(!repository.insert_completed(completed.clone()).unwrap());
            let mut conflicting = completed.clone();
            conflicting.active_duration_ms = 2_000;
            assert!(matches!(
                repository.insert_completed(conflicting),
                Err(StorageError::IncompatibleSchema(_))
            ));
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTimerSessionRepository::new(connection);
        assert_eq!(repository.load_active().unwrap(), Some(active.clone()));
        assert_eq!(repository.get_completed(completed.id).unwrap(), completed);
        let paused = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task_id),
            mode: TimerMode::Pomodoro,
            phase: TimerPhase::Work,
            state: TimerRunState::Paused,
            started_at: 10_000,
            last_resumed_at: None,
            accumulated_active_ms: 2_000,
            target_duration_ms: Some(25 * 60 * 1_000),
        };
        assert!(repository.clear_active(active.session_id).unwrap());
        repository.start_active(paused.clone(), 12_000).unwrap();
        assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));
        assert!(!repository.clear_active(Uuid::now_v7()).unwrap());
        assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));
        assert!(repository.clear_active(paused.session_id).unwrap());
        assert_eq!(repository.load_active().unwrap(), None);

        assert!(repository
            .connection()
            .execute(
                "INSERT INTO active_timer_session (
                     singleton, session_id, task_id, mode, phase, state, started_at,
                     last_resumed_at, accumulated_active_ms, updated_at
                 ) VALUES (1, ?1, NULL, 'stopwatch', 'short_break', 'paused', 1, NULL, 0, 1)",
                [Uuid::now_v7().to_string()],
            )
            .is_err());
        let orphan_task_id = Uuid::now_v7();
        let orphan = CompletedTimerSession {
            id: Uuid::now_v7(),
            task_id: orphan_task_id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: 1,
            ended_at: 10,
            active_duration_ms: 5,
            created_at: 11,
        };
        assert!(matches!(
            repository.insert_completed(orphan),
            Err(StorageError::NotFound(id)) if id == orphan_task_id
        ));
        assert!(repository
            .connection()
            .execute(
                "INSERT INTO timer_sessions (
                     id, task_id, mode, finish_kind, started_at, ended_at,
                     active_duration_ms, created_at
                 ) VALUES (?1, ?2, 'stopwatch', 'completed', 1, 10, 5, 9)",
                params![Uuid::now_v7().to_string(), task_id.to_string()],
            )
            .is_err());
    }

    #[test]
    fn finish_active_timer_rejects_start_mismatch_and_accepts_exact_retry() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
        let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
        let task_id = task.id;
        SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(list)
            .unwrap();
        SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(task)
            .unwrap();
        let active = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task_id),
            mode: TimerMode::Stopwatch,
            phase: TimerPhase::Work,
            state: TimerRunState::Paused,
            started_at: 2_000,
            last_resumed_at: None,
            accumulated_active_ms: 500,
            target_duration_ms: None,
        };
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .start_active(active.clone(), 3_000)
            .unwrap();
        let completed = CompletedTimerSession {
            id: active.session_id,
            task_id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: active.started_at,
            ended_at: 4_000,
            active_duration_ms: 500,
            created_at: 4_000,
        };

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        let mut mismatched = completed.clone();
        mismatched.started_at += 1;
        assert!(matches!(
            transaction.finish_active_timer_session(mismatched),
            Err(StorageError::IncompatibleSchema(_))
        ));
        drop(transaction);
        assert_eq!(
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .load_active()
                .unwrap(),
            Some(active.clone())
        );

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        let mut duration_mismatch = completed.clone();
        duration_mismatch.active_duration_ms -= 1;
        assert!(matches!(
            transaction.finish_active_timer_session(duration_mismatch),
            Err(StorageError::CompletedTimerDurationMismatch {
                expected_ms: 500,
                actual_ms: 499,
            })
        ));
        drop(transaction);
        assert_eq!(
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .load_active()
                .unwrap(),
            Some(active.clone())
        );

        let mut repository =
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert!(repository.insert_completed(completed.clone()).unwrap());
        drop(repository);
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        assert!(!transaction
            .finish_active_timer_session(completed.clone())
            .unwrap());
        transaction.commit().unwrap();
        let repository =
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(repository.load_active().unwrap(), None);
        assert_eq!(repository.get_completed(completed.id).unwrap(), completed);
    }

    #[test]
    fn finish_running_timer_requires_restored_duration_and_accepts_pomodoro_target_instant() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
        let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
        SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(list)
            .unwrap();
        SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(task.clone())
            .unwrap();
        let running = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task.id),
            mode: TimerMode::Stopwatch,
            phase: TimerPhase::Work,
            state: TimerRunState::Running,
            started_at: 1_000,
            last_resumed_at: Some(2_000),
            accumulated_active_ms: 500,
            target_duration_ms: None,
        };
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .start_active(running.clone(), 2_000)
            .unwrap();
        let completed = CompletedTimerSession {
            id: running.session_id,
            task_id: task.id,
            mode: TimerMode::Stopwatch,
            finish_kind: TimerFinishKind::Completed,
            started_at: running.started_at,
            ended_at: 3_000,
            active_duration_ms: 1_500,
            created_at: 3_000,
        };
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        let mut mismatch = completed.clone();
        mismatch.active_duration_ms = 1_499;
        assert!(matches!(
            transaction.finish_active_timer_session(mismatch),
            Err(StorageError::CompletedTimerDurationMismatch { .. })
        ));
        drop(transaction);
        assert_eq!(
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .load_active()
                .unwrap(),
            Some(running)
        );
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        assert!(transaction.finish_active_timer_session(completed).unwrap());
        transaction.commit().unwrap();

        let pomodoro = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task.id),
            mode: TimerMode::Pomodoro,
            phase: TimerPhase::Work,
            state: TimerRunState::Running,
            started_at: 10_000,
            last_resumed_at: Some(15_000),
            accumulated_active_ms: 5_000,
            target_duration_ms: Some(25_000),
        };
        let reached_at = taskveil_domain::pomodoro_target_reached_at(&pomodoro).unwrap();
        assert_eq!(reached_at, 35_000);
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .start_active(pomodoro.clone(), 15_000)
            .unwrap();
        let completed = CompletedTimerSession {
            id: pomodoro.session_id,
            task_id: task.id,
            mode: TimerMode::Pomodoro,
            finish_kind: TimerFinishKind::Completed,
            started_at: pomodoro.started_at,
            ended_at: reached_at,
            active_duration_ms: 25_000,
            created_at: 40_000,
        };
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        assert!(transaction.finish_active_timer_session(completed).unwrap());
        transaction.commit().unwrap();
    }

    #[test]
    fn active_timer_start_conflicts_and_update_preserves_session_contract() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
        let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
        SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(list)
            .unwrap();
        SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .insert(task.clone())
            .unwrap();
        let running = ActiveTimerSession {
            session_id: Uuid::now_v7(),
            task_id: Some(task.id),
            mode: TimerMode::Pomodoro,
            phase: TimerPhase::Work,
            state: TimerRunState::Running,
            started_at: 2_000,
            last_resumed_at: Some(2_000),
            accumulated_active_ms: 0,
            target_duration_ms: Some(25_000),
        };
        let mut repository =
            SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        repository.start_active(running.clone(), 2_000).unwrap();

        let mut competing = running.clone();
        competing.session_id = Uuid::now_v7();
        assert!(matches!(
            repository.start_active(competing, 2_001),
            Err(StorageError::ActiveTimerConflict(id)) if id == running.session_id
        ));
        assert_eq!(repository.load_active().unwrap(), Some(running.clone()));

        let mut changed_identity = running.clone();
        changed_identity.session_id = Uuid::now_v7();
        assert!(matches!(
            repository.update_active(changed_identity, 2_002),
            Err(StorageError::InvalidActiveTimerUpdate(
                DomainError::ActiveTimerIdentityChanged
            ))
        ));

        let mut paused = running.clone();
        paused.state = TimerRunState::Paused;
        paused.last_resumed_at = None;
        paused.accumulated_active_ms = 500;
        repository.update_active(paused.clone(), 2_500).unwrap();
        assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));

        let mut injected = paused.clone();
        injected.accumulated_active_ms += 1;
        assert!(matches!(
            repository.update_active(injected, 2_501),
            Err(StorageError::InvalidActiveTimerUpdate(
                DomainError::ActiveTimerProgressChangedOutsidePause
            ))
        ));

        let mut reduced_target = paused;
        reduced_target.target_duration_ms = Some(20_000);
        assert!(matches!(
            repository.update_active(reduced_target, 2_502),
            Err(StorageError::InvalidActiveTimerUpdate(
                DomainError::ActiveTimerTargetRegressed
            ))
        ));
    }

    #[test]
    #[ignore = "task-67 manual performance verification for a 10k encrypted seed"]
    fn task_67_reports_10000_task_storage_timings() {
        let file = NamedTempFile::new().unwrap();
        let seed = seed_performance_database(file.path(), &KEY, PerformanceSeedSchema::Latest);
        assert_eq!(seed.list_ids.len(), 10);
        assert_eq!(seed.task_count, 10_000);

        let mut rows: Vec<(&str, usize, u128, String)> = Vec::new();

        let started = std::time::Instant::now();
        let home_tasks = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteTaskRepository::new(connection);
            repository
                .list_home(seed.today_start_ms, seed.tomorrow_start_ms)
                .unwrap()
        };
        rows.push((
            "get_today_tasks(list_home)",
            home_tasks.len(),
            started.elapsed().as_millis(),
            "cross-list Home query on encrypted DB".to_string(),
        ));

        let started = std::time::Instant::now();
        let list_tasks = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteTaskRepository::new(connection);
            repository.list_active_by_list(seed.list_ids[0]).unwrap()
        };
        rows.push((
            "get_tasks(list 1)",
            list_tasks.len(),
            started.elapsed().as_millis(),
            "single list, 1000 tasks".to_string(),
        ));

        let started = std::time::Instant::now();
        let search_results = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteTaskRepository::new(connection);
            repository.search_tasks("alpha").unwrap()
        };
        rows.push((
            "search_tasks(alpha)",
            search_results.len(),
            started.elapsed().as_millis(),
            "FTS5 prefix query".to_string(),
        ));

        let migration_file = NamedTempFile::new().unwrap();
        let migration_seed =
            seed_performance_database(migration_file.path(), &KEY, PerformanceSeedSchema::V3);
        assert_eq!(migration_seed.task_count, 10_000);
        let started = std::time::Instant::now();
        let migrated_connection = open_encrypted(migration_file.path(), &KEY).unwrap();
        let migration_elapsed_ms = started.elapsed().as_millis();
        assert_eq!(
            read_user_version(&migrated_connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        rows.push((
            "migration(v3_to_latest)",
            migration_seed.task_count,
            migration_elapsed_ms,
            "v4 FTS backfill + v5-v7 migrations".to_string(),
        ));

        let started = std::time::Instant::now();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteListRepository::new(connection);
            repository
                .ensure_default_list("Inbox".to_string(), seed.today_start_ms)
                .unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteListRepository::new(connection);
            assert_eq!(repository.list_all().unwrap().len(), 10);
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteListRepository::new(connection);
            assert!(repository.list_archived().unwrap().is_empty());
        }
        let startup_home_count = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let repository = SqliteTaskRepository::new(connection);
            repository
                .list_home(seed.today_start_ms, seed.tomorrow_start_ms)
                .unwrap()
                .len()
        };
        let startup_elapsed_ms = started.elapsed().as_millis();
        rows.push((
            "startup_approx(init+initial_queries)",
            startup_home_count,
            startup_elapsed_ms,
            "open+default list+lists+archived+Home".to_string(),
        ));

        println!(
            "task-67 Rust performance seed: lists=10 tasks={} due={} closed={}",
            seed.task_count, seed.due_task_count, seed.closed_task_count
        );
        println!("| operation | rows | elapsed_ms | note |");
        println!("|---|---:|---:|---|");
        for (operation, count, elapsed_ms, note) in &rows {
            println!("| {operation} | {count} | {elapsed_ms} | {note} |");
        }

        assert_eq!(list_tasks.len(), 1_000);
        assert!(!home_tasks.is_empty());
        assert!(!search_results.is_empty());
        assert!(
            startup_elapsed_ms < 2_000,
            "startup approximation exceeded F-50: {startup_elapsed_ms} ms"
        );
    }

    #[test]
    fn fts5_search_works_after_reopening_encrypted_database() {
        let file = NamedTempFile::new().unwrap();
        let mut task = sample_task();
        task.content.title = "Encrypted search".to_string();
        task.content.note = "SQLCipher FTS5".to_string();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteTaskRepository::new(connection);
            repository.insert(task.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);

        assert_eq!(repository.search_tasks("sqlcipher").unwrap(), vec![task]);
    }

    #[test]
    fn new_database_is_created_via_baseline_and_migrated_to_latest_schema() {
        let file = NamedTempFile::new().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            archived_at_column(&connection),
            Some(("INTEGER".to_string(), 0))
        );
        assert_eq!(
            is_default_column(&connection),
            Some(("INTEGER".to_string(), 1, "0".to_string()))
        );
        assert_eq!(
            setting_column(&connection, "key"),
            Some(("TEXT".to_string(), 0))
        );
        assert_eq!(
            setting_column(&connection, "value"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            setting_column(&connection, "updated_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "task_id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "remind_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "snoozed_until"),
            Some(("INTEGER".to_string(), 0))
        );
        assert_eq!(
            sync_outbox_column(&connection, "created_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            sync_cursor_column(&connection, "updated_at"),
            Some(("INTEGER".to_string(), 1))
        );
    }

    #[test]
    fn v1_database_migrates_to_latest_and_preserves_existing_data() {
        let file = NamedTempFile::new().unwrap();
        create_baseline_v1_database(file.path(), &KEY, true);

        let mut list = sample_list("a0");
        list.is_default = true;
        let mut task = sample_task();
        task.list_id = list.id;
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_baseline_v1_list(&connection, &list);
        }
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_task_pre_v20(&connection, &task);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        list.sort_order = "00000000000000010000000000000000".to_string();
        task.sort_order = "00000000000000010000000000000000".to_string();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            archived_at_column(&connection),
            Some(("INTEGER".to_string(), 0))
        );
        assert_eq!(
            is_default_column(&connection),
            Some(("INTEGER".to_string(), 1, "0".to_string()))
        );
        assert_eq!(
            setting_column(&connection, "value"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "created_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "blob"),
            Some(("BLOB".to_string(), 0))
        );
        assert_eq!(
            sync_cursor_column(&connection, "name"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .get(list.id)
                .unwrap(),
            list
        );
        assert_eq!(
            SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .get(task.id)
                .unwrap(),
            task
        );
    }

    #[test]
    fn legacy_user_version_zero_v1_database_is_promoted_and_migrated() {
        let file = NamedTempFile::new().unwrap();
        create_baseline_v1_database(file.path(), &KEY, false);

        let mut list = sample_list("legacy");
        list.is_default = true;
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_baseline_v1_list(&connection, &list);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        list.sort_order = "00000000000000010000000000000000".to_string();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            archived_at_column(&connection),
            Some(("INTEGER".to_string(), 0))
        );
        assert_eq!(
            is_default_column(&connection),
            Some(("INTEGER".to_string(), 1, "0".to_string()))
        );
        assert_eq!(
            setting_column(&connection, "updated_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "task_id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            sync_outbox_column(&connection, "revision_hlc"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            SqliteListRepository::new(connection).get(list.id).unwrap(),
            list
        );
    }

    #[test]
    fn sqlite_settings_repository_returns_none_for_missing_key() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteSettingsRepository::new(connection);

        assert_eq!(repository.get_setting("ui_mode").unwrap(), None);
    }

    #[test]
    fn sqlite_settings_repository_roundtrips_setting() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSettingsRepository::new(connection);

        repository
            .set_setting("ui_mode", "simple", 1_799_000_000_000)
            .unwrap();

        assert_eq!(
            repository.get_setting("ui_mode").unwrap(),
            Some("simple".to_string())
        );
    }

    #[test]
    fn sqlite_settings_repository_overwrites_existing_setting() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSettingsRepository::new(connection);

        repository
            .set_setting("ui_mode", "simple", 1_799_000_000_000)
            .unwrap();
        repository
            .set_setting("ui_mode", "advanced", 1_799_000_001_000)
            .unwrap();

        assert_eq!(
            repository.get_setting("ui_mode").unwrap(),
            Some("advanced".to_string())
        );
        let updated_at: i64 = repository
            .connection()
            .query_row(
                "SELECT updated_at FROM settings WHERE key = ?1",
                ["ui_mode"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, 1_799_000_001_000);
    }

    #[test]
    fn sqlite_sync_state_repository_coalesces_record_head_and_old_ack_is_safe() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);
        let record_id = Uuid::now_v7();
        let first_op_id = Uuid::now_v7();
        let second_op_id = Uuid::now_v7();

        repository
            .put_outbox_head(new_live_outbox(
                record_id,
                "tasks",
                first_op_id,
                Some("base-0"),
                "revision-1",
                "mutation-1",
                vec![1, 2, 3],
            ))
            .unwrap();
        let second = repository
            .put_outbox_head(NewSyncOutboxEntry {
                op_id: second_op_id,
                record_id,
                collection: "tasks".to_string(),
                base_revision_hlc: Some("base-0".to_string()),
                revision_hlc: "revision-2".to_string(),
                state: SyncOutboxState::Tombstone {
                    delete_hlc: "delete-2".to_string(),
                },
                created_at: 1_799_000_000_001,
            })
            .unwrap();

        assert_eq!(repository.list_outbox_heads(10).unwrap(), vec![second]);
        assert!(!repository.ack_outbox_op(first_op_id).unwrap());
        assert_eq!(repository.list_outbox_heads(10).unwrap().len(), 1);
        assert!(repository.ack_outbox_op(second_op_id).unwrap());
        assert!(repository.list_outbox_heads(10).unwrap().is_empty());
        assert!(!repository.ack_outbox_op(second_op_id).unwrap());
    }

    #[test]
    fn durable_quarantine_is_idempotent_and_blocks_only_its_record_outbox() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);
        let blocked_id = Uuid::now_v7();
        let unrelated_id = Uuid::now_v7();
        for record_id in [blocked_id, unrelated_id] {
            repository
                .put_outbox_head(new_live_outbox(
                    record_id,
                    "tasks",
                    Uuid::now_v7(),
                    None,
                    "revision-local",
                    "mutation-local",
                    vec![9],
                ))
                .unwrap();
        }
        let quarantined = SyncQuarantineEntry {
            record_id: blocked_id,
            collection: "tasks".to_string(),
            seq: 7,
            revision_hlc: "revision-remote".to_string(),
            state: SyncOutboxState::Live {
                mutation_hlc: "mutation-remote".to_string(),
                blob: vec![1, 2, 3],
            },
            reason: "no_matching_dek".to_string(),
            required_list_id: None,
            first_failed_at: 10,
            last_failed_at: 10,
            attempt_count: 1,
        };
        repository.put_quarantine(quarantined.clone()).unwrap();
        repository
            .put_quarantine(SyncQuarantineEntry {
                state: SyncOutboxState::Tombstone {
                    delete_hlc: "must-not-replace".to_string(),
                },
                reason: "authentication_failed".to_string(),
                last_failed_at: 20,
                ..quarantined.clone()
            })
            .unwrap();

        let rows = repository.list_quarantine(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].first_failed_at, 10);
        assert_eq!(rows[0].last_failed_at, 20);
        assert_eq!(rows[0].reason, "authentication_failed");
        assert_eq!(
            rows[0].state,
            SyncOutboxState::Live {
                mutation_hlc: "mutation-remote".to_string(),
                blob: vec![1, 2, 3]
            }
        );

        repository
            .put_quarantine(SyncQuarantineEntry {
                seq: 6,
                revision_hlc: "older-revision".to_string(),
                last_failed_at: 30,
                ..quarantined.clone()
            })
            .unwrap();
        let rows = repository.list_quarantine(10).unwrap();
        assert_eq!(rows[0].seq, 7);
        assert_eq!(rows[0].revision_hlc, "revision-remote");
        assert_eq!(rows[0].attempt_count, 2);
        assert!(matches!(
            repository.put_quarantine(SyncQuarantineEntry {
                revision_hlc: "different-at-same-seq".to_string(),
                ..quarantined.clone()
            }),
            Err(StorageError::IncompatibleSchema(_))
        ));
        assert!(matches!(
            repository.put_quarantine(SyncQuarantineEntry {
                collection: "lists".to_string(),
                ..quarantined.clone()
            }),
            Err(StorageError::SyncCollectionMismatch { .. })
        ));
        assert!(matches!(
            repository.put_record_state(SyncRecordState {
                record_id: blocked_id,
                collection: "lists".to_string(),
                current_revision_hlc: None,
                state: SyncRecordSemanticState::Tombstone {
                    delete_hlc: "delete".to_string(),
                },
                updated_at: 30,
            }),
            Err(StorageError::SyncCollectionMismatch { .. })
        ));

        repository
            .put_quarantine(SyncQuarantineEntry {
                seq: 8,
                revision_hlc: "newer-revision".to_string(),
                state: SyncOutboxState::Tombstone {
                    delete_hlc: "newer-delete".to_string(),
                },
                reason: "corrupt_envelope".to_string(),
                first_failed_at: 40,
                last_failed_at: 40,
                ..quarantined
            })
            .unwrap();
        let rows = repository.list_quarantine(10).unwrap();
        assert_eq!(rows[0].seq, 8);
        assert_eq!(rows[0].revision_hlc, "newer-revision");
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].first_failed_at, 40);
        assert!(matches!(rows[0].state, SyncOutboxState::Tombstone { .. }));
        let pushable = repository.list_outbox_heads(10).unwrap();
        assert_eq!(pushable.len(), 1);
        assert_eq!(pushable[0].record_id, unrelated_id);
        assert!(repository.has_outbox_head("tasks", blocked_id).unwrap());
        assert!(repository.delete_quarantine(blocked_id).unwrap());
        assert_eq!(repository.list_outbox_heads(10).unwrap().len(), 2);
    }

    #[test]
    fn replayable_quarantine_query_skips_corruption_without_head_of_line_blocking() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);
        for seq in 1..=100 {
            repository
                .put_quarantine(SyncQuarantineEntry {
                    record_id: Uuid::now_v7(),
                    collection: "lists".to_string(),
                    seq,
                    revision_hlc: format!("corrupt-{seq}"),
                    state: SyncOutboxState::Live {
                        mutation_hlc: format!("mutation-{seq}"),
                        blob: vec![1],
                    },
                    reason: "corrupt_envelope".to_string(),
                    required_list_id: None,
                    first_failed_at: 10,
                    last_failed_at: 10,
                    attempt_count: 1,
                })
                .unwrap();
        }
        let waiting_id = Uuid::now_v7();
        repository
            .put_quarantine(SyncQuarantineEntry {
                record_id: waiting_id,
                collection: "lists".to_string(),
                seq: 101,
                revision_hlc: "waiting".to_string(),
                state: SyncOutboxState::Live {
                    mutation_hlc: "waiting-mutation".to_string(),
                    blob: vec![1],
                },
                reason: "missing_dek".to_string(),
                required_list_id: Some(waiting_id),
                first_failed_at: 10,
                last_failed_at: 10,
                attempt_count: 1,
            })
            .unwrap();

        let replayable = repository.list_replayable_quarantine(None, 100).unwrap();
        assert_eq!(replayable.len(), 1);
        assert_eq!(replayable[0].record_id, waiting_id);
        assert_eq!(repository.list_quarantine(100).unwrap().len(), 100);
    }

    #[test]
    fn sqlite_sync_state_repository_preserves_tagged_heads_and_states_after_reopen() {
        let file = NamedTempFile::new().unwrap();
        let record_id = Uuid::now_v7();
        let op_id = Uuid::now_v7();
        let stored = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteSyncStateRepository::new(connection);
            let stored = repository
                .put_outbox_head(NewSyncOutboxEntry {
                    op_id,
                    record_id,
                    collection: "tasks".to_string(),
                    base_revision_hlc: Some("base-reopen".to_string()),
                    revision_hlc: "revision-reopen".to_string(),
                    state: SyncOutboxState::Live {
                        mutation_hlc: "mutation-reopen".to_string(),
                        blob: vec![7, 8, 9],
                    },
                    created_at: 1_799_000_000_000,
                })
                .unwrap();
            repository
                .put_record_state(SyncRecordState {
                    record_id,
                    collection: "tasks".to_string(),
                    current_revision_hlc: Some("revision-reopen".to_string()),
                    state: SyncRecordSemanticState::Tombstone {
                        delete_hlc: "delete-reopen".to_string(),
                    },
                    updated_at: 1_799_000_000_001,
                })
                .unwrap();
            stored
        };

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteSyncStateRepository::new(connection);

        assert_eq!(repository.list_outbox_heads(10).unwrap(), vec![stored]);
        assert_eq!(
            repository.get_record_state("tasks", record_id).unwrap(),
            Some(SyncRecordState {
                record_id,
                collection: "tasks".to_string(),
                current_revision_hlc: Some("revision-reopen".to_string()),
                state: SyncRecordSemanticState::Tombstone {
                    delete_hlc: "delete-reopen".to_string(),
                },
                updated_at: 1_799_000_000_001,
            })
        );
    }

    #[test]
    fn sqlite_sync_state_rejects_unknown_and_changed_collections() {
        let file = NamedTempFile::new().unwrap();
        let record_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);

        assert!(matches!(
            repository.put_outbox_head(new_live_outbox(
                record_id,
                "unknown",
                Uuid::now_v7(),
                None,
                "revision-1",
                "mutation-1",
                vec![1],
            )),
            Err(StorageError::InvalidSyncCollection(collection)) if collection == "unknown"
        ));

        repository
            .put_record_state(live_record_state(
                record_id,
                "tasks",
                None,
                "mutation-1",
                "{}",
                1,
            ))
            .unwrap();
        assert!(matches!(
            repository.put_outbox_head(new_live_outbox(
                record_id,
                "lists",
                Uuid::now_v7(),
                None,
                "revision-2",
                "mutation-2",
                vec![2],
            )),
            Err(StorageError::SyncCollectionMismatch {
                record_id: mismatch_id,
                existing,
                requested,
            }) if mismatch_id == record_id && existing == "tasks" && requested == "lists"
        ));
        assert!(matches!(
            repository.get_record_state("lists", record_id),
            Err(StorageError::SyncCollectionMismatch { .. })
        ));
    }

    #[test]
    fn sync_v2_schema_rejects_malformed_live_and_tombstone_rows() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let record_id = Uuid::now_v7();
        let op_id = Uuid::now_v7();

        let live_without_blob = connection.execute(
            "INSERT INTO sync_outbox (
                 record_id, collection, op_id, revision_hlc,
                 state_kind, semantic_hlc, blob, created_at
             ) VALUES (?1, 'tasks', ?2, 'revision', 'live', 'mutation', NULL, 1)",
            params![record_id.to_string(), op_id.to_string()],
        );
        assert!(live_without_blob.is_err());

        let tombstone_with_plaintext = connection.execute(
            "INSERT INTO sync_record_states (
                 record_id, collection, current_revision_hlc,
                 state_kind, semantic_hlc, plaintext_json, updated_at
             ) VALUES (?1, 'tasks', 'revision', 'tombstone', 'delete', '{}', 1)",
            [record_id.to_string()],
        );
        assert!(tombstone_with_plaintext.is_err());
    }

    #[test]
    fn sqlite_sync_state_repository_roundtrips_pull_cursor() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);

        assert_eq!(repository.get_cursor("default").unwrap(), None);

        repository
            .set_cursor("default", 41, 1_799_000_000_000)
            .unwrap();
        assert_eq!(
            repository.get_cursor("default").unwrap(),
            Some(SyncCursor {
                name: "default".to_string(),
                seq: 41,
                updated_at: 1_799_000_000_000,
            })
        );

        repository
            .set_cursor("default", 42, 1_799_000_001_000)
            .unwrap();
        assert_eq!(
            repository.get_cursor("default").unwrap(),
            Some(SyncCursor {
                name: "default".to_string(),
                seq: 42,
                updated_at: 1_799_000_001_000,
            })
        );
    }

    #[test]
    fn sqlite_reminder_repository_creates_updates_deletes_and_snoozes_reminders() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(task.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderRepository::new(connection);

        let first = repository
            .create_task_reminder(task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap();
        assert_eq!(first.task_id, task.id);
        assert_eq!(
            repository.list_task_reminders(task.id).unwrap(),
            vec![first.clone()]
        );

        let second = repository
            .create_task_reminder(task.id, 1_800_000_600_000, 1_799_000_001_000)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            repository.list_task_reminders(task.id).unwrap(),
            vec![first.clone(), second.clone()]
        );

        let updated = repository
            .update_reminder(first.id, 1_800_001_200_000, 1_799_000_002_000)
            .unwrap();
        assert_eq!(updated.id, first.id);
        assert_eq!(updated.created_at, first.created_at);
        assert_eq!(updated.remind_at, 1_800_001_200_000);

        let snoozed = repository
            .snooze_reminder(second.id, 1_800_004_200_000, 1_799_000_003_000)
            .unwrap();
        assert_eq!(snoozed.snoozed_until, Some(1_800_004_200_000));
        assert_eq!(repository.delete_reminder(updated.id).unwrap(), updated);
        assert_eq!(
            repository.clear_task_reminders(task.id).unwrap(),
            vec![snoozed]
        );
        assert!(repository.list_task_reminders(task.id).unwrap().is_empty());
    }

    #[test]
    fn sqlite_reminder_repository_enforces_time_uniqueness_and_limit() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteTaskRepository::new(connection)
                .insert(task.clone())
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderRepository::new(connection);
        assert!(matches!(
            repository.create_task_reminder(task.id, 100, 100),
            Err(StorageError::ReminderTimeNotFuture)
        ));

        let first = repository.create_task_reminder(task.id, 200, 100).unwrap();
        assert!(matches!(
            repository.create_task_reminder(task.id, 200, 100),
            Err(StorageError::DuplicateReminderTime)
        ));
        for remind_at in [300, 400, 500, 600] {
            repository
                .create_task_reminder(task.id, remind_at, 100)
                .unwrap();
        }
        assert!(matches!(
            repository.create_task_reminder(task.id, 700, 100),
            Err(StorageError::ReminderLimitReached {
                limit: MAX_REMINDERS_PER_TASK
            })
        ));
        assert!(matches!(
            repository.update_reminder(first.id, 300, 100),
            Err(StorageError::DuplicateReminderTime)
        ));
    }

    #[test]
    fn sqlite_reminder_repository_lists_pending_open_tasks_only() {
        let file = NamedTempFile::new().unwrap();
        let mut pending_task = sample_task();
        pending_task.status = TaskStatus::Todo;
        pending_task.sort_order = "a0".to_string();
        let mut closed_task = sample_task();
        closed_task.status = TaskStatus::Todo;
        closed_task.sort_order = "a1".to_string();
        let mut expired_task = sample_task();
        expired_task.status = TaskStatus::Todo;
        expired_task.sort_order = "a2".to_string();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(pending_task.clone()).unwrap();
            task_repository.insert(closed_task.clone()).unwrap();
            task_repository.insert(expired_task.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderRepository::new(connection);
        let pending = repository
            .create_task_reminder(pending_task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap();
        repository
            .create_task_reminder(closed_task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap();
        repository
            .connection()
            .execute(
                "UPDATE tasks SET status = 'done', completed_at = ?2 WHERE id = ?1",
                params![closed_task.id.to_string(), 1_799_000_010_000_i64],
            )
            .unwrap();
        repository
            .create_task_reminder(expired_task.id, 1_799_999_999_999, 1_799_000_000_000)
            .unwrap();

        assert_eq!(
            repository
                .list_pending_reminders(1_799_999_999_999)
                .unwrap(),
            vec![pending]
        );
    }

    #[test]
    fn sqlite_reminder_repository_lists_subtree_and_list_reminders_for_cancellation() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut parent = sample_task();
        parent.list_id = list.id;
        parent.parent_task_id = None;
        parent.sort_order = "a0".to_string();
        let mut child = sample_task();
        child.list_id = list.id;
        child.parent_task_id = Some(parent.id);
        child.sort_order = "a1".to_string();
        let mut other = sample_task();
        other.list_id = list.id;
        other.parent_task_id = None;
        other.sort_order = "a2".to_string();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(parent.clone()).unwrap();
            task_repository.insert(child.clone()).unwrap();
            task_repository.insert(other.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderRepository::new(connection);
        let parent_reminder = repository
            .create_task_reminder(parent.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap();
        let child_reminder = repository
            .create_task_reminder(child.id, 1_800_000_600_000, 1_799_000_000_000)
            .unwrap();
        let other_reminder = repository
            .create_task_reminder(other.id, 1_800_001_200_000, 1_799_000_000_000)
            .unwrap();

        assert_eq!(
            repository.list_task_subtree_reminders(parent.id).unwrap(),
            vec![parent_reminder.clone(), child_reminder.clone()]
        );
        assert_eq!(
            repository.list_list_reminders(list.id).unwrap(),
            vec![parent_reminder, child_reminder, other_reminder]
        );
    }

    #[test]
    fn task_delete_removes_reminders_but_list_delete_rehomes_them() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut default_list = sample_list("a1");
        default_list.is_default = true;
        let mut subtree_task = sample_task();
        subtree_task.list_id = list.id;
        subtree_task.parent_task_id = None;
        subtree_task.sort_order = "a0".to_string();
        let mut list_task = sample_task();
        list_task.list_id = list.id;
        list_task.parent_task_id = None;
        list_task.sort_order = "a1".to_string();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
            list_repository.insert(default_list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(subtree_task.clone()).unwrap();
            task_repository.insert(list_task.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut reminder_repository = SqliteReminderRepository::new(connection);
            reminder_repository
                .create_task_reminder(subtree_task.id, 1_800_000_000_000, 1_799_000_000_000)
                .unwrap();
            reminder_repository
                .create_task_reminder(list_task.id, 1_800_000_600_000, 1_799_000_000_000)
                .unwrap();
        }

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.delete_subtree(subtree_task.id).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let reminder_repository = SqliteReminderRepository::new(connection);
            assert!(reminder_repository
                .list_task_reminders(subtree_task.id)
                .unwrap()
                .is_empty());
            assert_eq!(
                reminder_repository
                    .list_task_reminders(list_task.id)
                    .unwrap()
                    .len(),
                1
            );
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.delete_and_rehome_tasks(list.id).unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let reminder_repository = SqliteReminderRepository::new(connection);
        assert!(reminder_repository
            .list_list_reminders(list.id)
            .unwrap()
            .is_empty());
        assert_eq!(
            reminder_repository
                .list_list_reminders(default_list.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn v4_database_migrates_to_v5_and_adds_settings_table() {
        let file = NamedTempFile::new().unwrap();
        create_v4_database(file.path(), &KEY);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            setting_column(&connection, "key"),
            Some(("TEXT".to_string(), 0))
        );
        assert_eq!(
            setting_column(&connection, "value"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            setting_column(&connection, "updated_at"),
            Some(("INTEGER".to_string(), 1))
        );
    }

    #[test]
    fn v5_database_migrates_to_v6_and_adds_reminders_table() {
        let file = NamedTempFile::new().unwrap();
        create_v5_database(file.path(), &KEY);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            reminder_column(&connection, "id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "task_id"),
            Some(("TEXT".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "remind_at"),
            Some(("INTEGER".to_string(), 1))
        );
        assert_eq!(
            reminder_column(&connection, "snoozed_until"),
            Some(("INTEGER".to_string(), 0))
        );
        assert_eq!(
            reminder_column(&connection, "created_at"),
            Some(("INTEGER".to_string(), 1))
        );
    }

    #[test]
    fn v2_database_promotes_first_active_list_to_default() {
        let file = NamedTempFile::new().unwrap();
        create_v2_database(file.path(), &KEY);

        let archived = List {
            archived_at: Some(1_799_000_001_000),
            ..sample_list("a0")
        };
        let active_second = sample_list("b0");
        let mut active_first = sample_list("a1");
        active_first.created_at = active_second.created_at - 1;
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_v2_list(&connection, &archived);
            insert_v2_list(&connection, &active_second);
            insert_v2_list(&connection, &active_first);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            default_list_ids(&connection),
            vec![active_first.id.to_string()]
        );
    }

    #[test]
    fn v2_database_with_no_active_lists_does_not_promote_default() {
        let file = NamedTempFile::new().unwrap();
        create_v2_database(file.path(), &KEY);

        let archived = List {
            archived_at: Some(1_799_000_001_000),
            ..sample_list("a0")
        };
        {
            let connection = open_raw_encrypted(file.path(), &KEY);
            insert_v2_list(&connection, &archived);
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert!(default_list_ids(&connection).is_empty());
    }

    #[test]
    fn ensure_default_list_creates_default_when_missing_and_keeps_existing_name() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);

        let inbox = repository
            .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
            .unwrap();
        let again = repository
            .ensure_default_list("インボックス".to_string(), 1_799_000_001_000)
            .unwrap();

        assert_eq!(inbox.id, again.id);
        assert_eq!(again.name, "Inbox");
        assert!(again.is_default);
        assert_eq!(repository.list_all().unwrap().len(), 1);
    }

    #[test]
    fn ensure_default_list_observes_ja_name_in_empty_database() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);

        let inbox = repository
            .ensure_default_list("インボックス".to_string(), 1_799_000_000_000)
            .unwrap();

        assert_eq!(inbox.name, "インボックス");
        assert!(inbox.is_default);
    }

    #[test]
    fn unique_index_prevents_multiple_default_lists() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        let first = repository
            .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
            .unwrap();
        let mut second = sample_list("a1");
        second.is_default = true;

        let result = repository.insert(second);

        assert!(matches!(result, Err(StorageError::Sqlite(_))));
        assert_eq!(repository.get_default().unwrap().unwrap().id, first.id);
    }

    #[test]
    fn default_list_cannot_be_archived_or_deleted_but_can_be_renamed() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        let mut list = repository
            .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
            .unwrap();

        list.name = "Renamed inbox".to_string();
        list.updated_at += 1;
        repository.update(list.clone()).unwrap();
        assert_eq!(repository.get(list.id).unwrap().name, "Renamed inbox");
        assert!(repository.get(list.id).unwrap().is_default);

        let mut archived = list.clone();
        archived.archived_at = Some(1_799_000_001_000);
        assert!(matches!(
            repository.update(archived),
            Err(StorageError::DefaultListProtected {
                operation: "archived",
                list_id,
            }) if list_id == list.id
        ));
        assert!(matches!(
            repository.delete_and_rehome_tasks(list.id),
            Err(StorageError::DefaultListProtected {
                operation: "deleted",
                list_id,
            }) if list_id == list.id
        ));
    }

    #[test]
    fn latest_schema_reopen_does_not_reapply_migrations() {
        let file = NamedTempFile::new().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let before_schema_version = schema_version(&connection);
        let before_user_version = read_user_version(&connection).unwrap();
        let before_archived_at_count = count_archived_at_columns(&connection);
        let before_is_default_column = is_default_column(&connection);
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(read_user_version(&connection).unwrap(), before_user_version);
        assert_eq!(schema_version(&connection), before_schema_version);
        assert_eq!(
            count_archived_at_columns(&connection),
            before_archived_at_count
        );
        assert_eq!(is_default_column(&connection), before_is_default_column);
    }

    #[test]
    fn failed_migration_rolls_back_archived_at_and_user_version() {
        let file = NamedTempFile::new().unwrap();
        create_baseline_v1_database(file.path(), &KEY, true);
        let mut connection = open_raw_encrypted(file.path(), &KEY);
        let failing_migrations = &[Migration {
            target_version: 2,
            name: "failing_archived_at",
            apply: failing_archived_at_migration,
        }];

        let result =
            apply_pending_migrations(&mut connection, BASELINE_SCHEMA_VERSION, failing_migrations);

        assert!(matches!(
            result,
            Err(StorageError::MigrationFailed {
                target_version: 2,
                migration: "failing_archived_at",
                ..
            })
        ));
        assert_eq!(
            read_user_version(&connection).unwrap(),
            BASELINE_SCHEMA_VERSION
        );
        assert_eq!(archived_at_column(&connection), None);
    }

    #[test]
    fn unsupported_newer_schema_version_is_rejected() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_raw_encrypted(file.path(), &KEY);
        set_user_version(&connection, LATEST_SCHEMA_VERSION + 1).unwrap();
        drop(connection);

        let result = open_encrypted(file.path(), &KEY);

        assert!(matches!(
            result,
            Err(StorageError::UnsupportedSchemaVersion { found, latest })
                if found == LATEST_SCHEMA_VERSION + 1 && latest == LATEST_SCHEMA_VERSION
        ));
    }

    #[test]
    fn sqlite_task_repository_insert_get_roundtrips_task() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();

        repository.insert(task.clone()).unwrap();

        assert_eq!(repository.get(task.id).unwrap(), task);
    }

    #[test]
    fn equal_task_ranks_use_record_id_as_stable_tie_break() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let list_id = Uuid::from_u128(10);
        let mut later_id = sample_task();
        later_id.id = Uuid::from_u128(12);
        later_id.list_id = list_id;
        later_id.parent_task_id = None;
        later_id.sort_order = "7fffffffffffffffffffffffffffffff".to_string();
        let mut earlier_id = later_id.clone();
        earlier_id.id = Uuid::from_u128(11);
        repository.insert(later_id.clone()).unwrap();
        repository.insert(earlier_id.clone()).unwrap();

        assert_eq!(
            repository
                .list_active_by_list(list_id)
                .unwrap()
                .into_iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![earlier_id.id, later_id.id]
        );
    }

    #[test]
    fn sqlite_list_repository_roundtrips_and_lists_by_sort_order() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        let mut first = sample_list("b0");
        let second = sample_list("a0");

        repository.insert(first.clone()).unwrap();
        repository.insert(second.clone()).unwrap();

        assert_eq!(repository.get(first.id).unwrap(), first);

        first.name = "Renamed".to_string();
        first.color = "#FFAA00".to_string();
        first.icon = "star".to_string();
        first.sort_order = "c0".to_string();
        first.archived_at = Some(1_799_000_001_000);
        first.updated_at += 1_000;
        repository.update(first.clone()).unwrap();

        assert_eq!(repository.get(first.id).unwrap(), first);
        assert_eq!(
            repository
                .list_all()
                .unwrap()
                .into_iter()
                .map(|list| list.id)
                .collect::<Vec<_>>(),
            vec![second.id]
        );
        assert_eq!(
            repository
                .list_archived()
                .unwrap()
                .into_iter()
                .map(|list| list.id)
                .collect::<Vec<_>>(),
            vec![first.id]
        );
    }

    #[test]
    fn archived_lists_use_rank_and_record_id_order() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        let mut later_rank = sample_list("b0000000000000000000000000000000");
        later_rank.id = Uuid::from_u128(12);
        later_rank.archived_at = Some(10);
        let mut later_id = sample_list("a0000000000000000000000000000000");
        later_id.id = Uuid::from_u128(11);
        later_id.archived_at = Some(30);
        let mut earlier_id = later_id.clone();
        earlier_id.id = Uuid::from_u128(10);
        earlier_id.archived_at = Some(20);
        repository.insert(later_rank.clone()).unwrap();
        repository.insert(later_id.clone()).unwrap();
        repository.insert(earlier_id.clone()).unwrap();

        assert_eq!(
            repository
                .list_archived()
                .unwrap()
                .into_iter()
                .map(|list| list.id)
                .collect::<Vec<_>>(),
            vec![earlier_id.id, later_id.id, later_rank.id]
        );
    }

    #[test]
    fn delete_list_rehomes_tasks_and_preserves_task_undo_entries() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Project".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
        let mut default_list =
            new_list("Inbox".to_string(), "a1".to_string(), 1_700_000_000_000).unwrap();
        default_list.is_default = true;
        let task = new_task(
            list.id,
            None,
            "Task".to_string(),
            "a0".to_string(),
            1_700_000_001_000,
        )
        .unwrap();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
            list_repository.insert(default_list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(task.clone()).unwrap();
            let edited =
                update_title(task.clone(), "Edited".to_string(), task.updated_at + 1).unwrap();
            task_repository
                .update_with_undo(
                    task.clone(),
                    edited,
                    TaskUndoOperation::Edit,
                    task.updated_at + 1,
                )
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        assert_eq!(list_repository.count_tasks(list.id).unwrap(), 1);
        assert_eq!(list_repository.delete_and_rehome_tasks(list.id).unwrap(), 1);
        assert!(matches!(
            list_repository.get(list.id),
            Err(StorageError::NotFound(id)) if id == list.id
        ));

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let task_repository = SqliteTaskRepository::new(connection);
        assert_eq!(
            task_repository.get(task.id).unwrap().list_id,
            default_list.id
        );
        assert!(task_repository.latest_unconsumed_undo().unwrap().is_some());
    }

    #[test]
    fn domain_usecases_persist_task_updates_after_reopen() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Inbox".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
        let task = new_task(
            list.id,
            None,
            "Draft title".to_string(),
            "a0".to_string(),
            1_700_000_001_000,
        )
        .unwrap();
        let renamed =
            update_title(task.clone(), "Final title".to_string(), 1_700_000_002_000).unwrap();
        let done =
            transition_task(renamed.clone(), TaskStatus::Done, None, 1_700_000_003_000).unwrap();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(task).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.update(renamed).unwrap();
            task_repository.update(done.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let task_repository = SqliteTaskRepository::new(connection);

        assert_eq!(task_repository.get(done.id).unwrap(), done);
    }

    #[test]
    fn delete_subtree_removes_root_descendants_and_undo_entries() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Inbox".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
        let active = new_task(
            list.id,
            None,
            "Keep".to_string(),
            "a0".to_string(),
            1_700_000_001_000,
        )
        .unwrap();
        let parent = new_task(
            list.id,
            None,
            "Delete parent".to_string(),
            "b0".to_string(),
            1_700_000_001_000,
        )
        .unwrap();
        let child = new_task(
            list.id,
            Some(parent.id),
            "Delete child".to_string(),
            "a0".to_string(),
            1_700_000_001_000,
        )
        .unwrap();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(active.clone()).unwrap();
        task_repository.insert(parent.clone()).unwrap();
        task_repository.insert(child).unwrap();

        let updated = update_title(
            parent.clone(),
            "Before delete".to_string(),
            parent.updated_at + 1,
        )
        .unwrap();
        task_repository
            .update_with_undo(
                parent.clone(),
                updated,
                TaskUndoOperation::Edit,
                parent.updated_at + 1,
            )
            .unwrap();

        assert_eq!(task_repository.count_descendants(parent.id).unwrap(), 1);
        assert_eq!(task_repository.delete_subtree(parent.id).unwrap(), 2);
        assert!(matches!(
            task_repository.get(parent.id),
            Err(StorageError::NotFound(id)) if id == parent.id
        ));
        assert_eq!(
            task_repository.list_active_by_list(list.id).unwrap(),
            vec![active]
        );
        assert!(task_repository.latest_unconsumed_undo().unwrap().is_none());
    }

    #[test]
    fn list_home_filters_due_active_and_closed_tasks_across_active_lists() {
        let file = NamedTempFile::new().unwrap();
        let today_start = 1_800_000_000_000;
        let tomorrow_start = today_start + 86_400_000;
        let overdue = today_start - 86_400_000;
        let tomorrow = tomorrow_start + 1_000;
        let upcoming = tomorrow_start + 86_400_000 + 1_000;

        let inbox = new_list("Inbox".to_string(), "a0".to_string(), today_start).unwrap();
        let work = new_list("Work".to_string(), "a1".to_string(), today_start).unwrap();
        let mut archived = new_list("Archive".to_string(), "a2".to_string(), today_start).unwrap();
        archived.archived_at = Some(today_start + 1);

        let mut due_today = new_task(
            inbox.id,
            None,
            "Due today".to_string(),
            "a0".to_string(),
            today_start,
        )
        .unwrap();
        due_today.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
        due_today.scheduled_at = Some(today_start + 500);
        let no_due_child = new_task(
            inbox.id,
            Some(due_today.id),
            "No due child".to_string(),
            "a0".to_string(),
            today_start,
        )
        .unwrap();
        let no_due_parent = new_task(
            inbox.id,
            None,
            "No due parent".to_string(),
            "a4".to_string(),
            today_start,
        )
        .unwrap();
        let mut due_child = new_task(
            inbox.id,
            Some(no_due_parent.id),
            "Due child".to_string(),
            "a0".to_string(),
            today_start,
        )
        .unwrap();
        due_child.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
        let mut overdue_task = new_task(
            work.id,
            None,
            "Overdue".to_string(),
            "a0".to_string(),
            today_start,
        )
        .unwrap();
        overdue_task.due = Some(TaskDue::date_time(overdue, "UTC").unwrap());
        let mut tomorrow_task = new_task(
            inbox.id,
            None,
            "Tomorrow".to_string(),
            "a1".to_string(),
            today_start,
        )
        .unwrap();
        tomorrow_task.due = Some(TaskDue::date_time(tomorrow, "UTC").unwrap());
        let mut upcoming_task = new_task(
            inbox.id,
            None,
            "Upcoming".to_string(),
            "a2".to_string(),
            today_start,
        )
        .unwrap();
        upcoming_task.due = Some(TaskDue::date_time(upcoming, "UTC").unwrap());
        let no_due = new_task(
            inbox.id,
            None,
            "No due".to_string(),
            "a3".to_string(),
            today_start,
        )
        .unwrap();
        let mut scheduled_today = new_task(
            work.id,
            None,
            "Scheduled today".to_string(),
            "a4".to_string(),
            today_start,
        )
        .unwrap();
        scheduled_today.scheduled_at = Some(today_start + 3_600_000);
        let mut scheduled_tomorrow = new_task(
            work.id,
            None,
            "Scheduled tomorrow".to_string(),
            "a5".to_string(),
            today_start,
        )
        .unwrap();
        scheduled_tomorrow.scheduled_at = Some(tomorrow_start + 3_600_000);
        let mut archived_task = new_task(
            archived.id,
            None,
            "Archived".to_string(),
            "a0".to_string(),
            today_start,
        )
        .unwrap();
        archived_task.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
        let mut closed_today = new_task(
            work.id,
            None,
            "Closed today".to_string(),
            "a1".to_string(),
            today_start,
        )
        .unwrap();
        closed_today =
            transition_task(closed_today, TaskStatus::Done, None, today_start + 1_000).unwrap();
        let mut closed_yesterday = new_task(
            work.id,
            None,
            "Closed yesterday".to_string(),
            "a2".to_string(),
            today_start,
        )
        .unwrap();
        closed_yesterday.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
        closed_yesterday = transition_task(
            closed_yesterday,
            TaskStatus::Done,
            None,
            today_start - 1_000,
        )
        .unwrap();
        let mut wont_do_today = new_task(
            work.id,
            None,
            "Wont do today".to_string(),
            "a3".to_string(),
            today_start,
        )
        .unwrap();
        wont_do_today = transition_task(
            wont_do_today,
            TaskStatus::WontDo,
            Some("not needed".to_string()),
            today_start + 2_000,
        )
        .unwrap();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(inbox.clone()).unwrap();
            list_repository.insert(work.clone()).unwrap();
            list_repository.insert(archived).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        for task in [
            due_today,
            overdue_task,
            tomorrow_task,
            upcoming_task,
            no_due,
            scheduled_today,
            scheduled_tomorrow,
            archived_task,
            closed_today,
            closed_yesterday,
            wont_do_today,
            no_due_child,
            no_due_parent,
            due_child,
        ] {
            task_repository.insert(task).unwrap();
        }

        let home_tasks = task_repository
            .list_home(today_start, tomorrow_start)
            .unwrap();
        let titles = home_tasks
            .iter()
            .map(|entry| entry.task.content.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            titles,
            vec![
                "Overdue",
                "Due today",
                "Due child",
                "Tomorrow",
                "Upcoming",
                "No due child",
                "Closed today",
                "Wont do today",
                "No due parent",
                "Scheduled today"
            ]
        );
        assert_eq!(
            titles.iter().filter(|title| **title == "Due today").count(),
            1,
            "a dual due/scheduled target remains one Home row"
        );
        for title in ["Closed today", "Wont do today"] {
            assert!(
                home_tasks
                    .iter()
                    .find(|entry| entry.task.content.title == title)
                    .unwrap()
                    .is_home_target,
                "today's completed achievement is independent of planning fields"
            );
        }
        assert!(
            home_tasks
                .iter()
                .find(|entry| entry.task.content.title == "Due today")
                .unwrap()
                .is_home_target
        );
        assert!(
            !home_tasks
                .iter()
                .find(|entry| entry.task.content.title == "No due child")
                .unwrap()
                .is_home_target
        );
        assert!(
            !home_tasks
                .iter()
                .find(|entry| entry.task.content.title == "No due parent")
                .unwrap()
                .is_home_target
        );
        assert!(
            home_tasks
                .iter()
                .find(|entry| entry.task.content.title == "Due child")
                .unwrap()
                .is_home_target
        );
        assert_eq!(
            home_tasks
                .iter()
                .find(|entry| entry.task.content.title == "Overdue")
                .unwrap()
                .list_name,
            "Work"
        );
        assert!(!titles.contains(&"No due"));
        assert!(!titles.contains(&"Scheduled tomorrow"));
        assert!(!titles.contains(&"Archived"));
        assert!(!titles.contains(&"Closed yesterday"));
    }

    #[test]
    fn calendar_range_rejects_empty_or_reversed_dimensions() {
        let day = CivilDate::parse("2026-03-08").unwrap();
        let next = CivilDate::parse("2026-03-09").unwrap();
        let start = UtcInstant::from_millis(1_773_000_000_000).unwrap();
        let end = UtcInstant::from_millis(start.as_millis() + 1).unwrap();

        assert_eq!(
            CalendarRange::new(day.clone(), day, start, end),
            Err(CalendarRangeError::InvalidCivilDateRange)
        );
        assert_eq!(
            CalendarRange::new(next.clone(), next, end, start),
            Err(CalendarRangeError::InvalidCivilDateRange)
        );
        assert_eq!(
            CalendarRange::new(
                CivilDate::parse("2026-03-08").unwrap(),
                CivilDate::parse("2026-03-09").unwrap(),
                start,
                start,
            ),
            Err(CalendarRangeError::InvalidInstantRange)
        );

        for hours in [23, 25] {
            let dst_end = UtcInstant::from_millis(start.as_millis() + hours * 3_600_000).unwrap();
            let range = CalendarRange::new(
                CivilDate::parse("2026-03-08").unwrap(),
                CivilDate::parse("2026-03-09").unwrap(),
                start,
                dst_end,
            )
            .unwrap();
            assert_eq!(
                range.end_at().as_millis() - range.start_at().as_millis(),
                hours * 3_600_000
            );
        }
    }

    #[test]
    fn calendar_occurrences_preserve_semantics_boundaries_and_archived_context() {
        let file = NamedTempFile::new().unwrap();
        // America/New_York 2026 spring-forward day is 23 hours. The storage
        // contract consumes caller-provided boundaries and never adds 24h.
        let start_ms = 1_773_035_600_000; // 2026-03-08T05:00:00Z
        let end_ms = start_ms + 23 * 3_600_000;
        let range = CalendarRange::new(
            CivilDate::parse("2026-03-08").unwrap(),
            CivilDate::parse("2026-03-09").unwrap(),
            UtcInstant::from_millis(start_ms).unwrap(),
            UtcInstant::from_millis(end_ms).unwrap(),
        )
        .unwrap();
        let active = new_list("Active".into(), "a0".into(), start_ms).unwrap();
        let mut archived = new_list("Archive".into(), "a1".into(), start_ms).unwrap();
        archived.archived_at = Some(start_ms);
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(active.clone()).unwrap();
            lists.insert(archived.clone()).unwrap();
        }

        let mut dual = new_task(active.id, None, "Dual".into(), "a0".into(), start_ms).unwrap();
        dual.due = Some(TaskDue::date("2026-03-08").unwrap());
        dual.scheduled_at = Some(start_ms);

        let mut datetime =
            new_task(active.id, None, "Datetime".into(), "a1".into(), start_ms).unwrap();
        datetime.due = Some(TaskDue::date_time(end_ms - 1, "America/New_York").unwrap());
        datetime = transition_task(datetime, TaskStatus::InProgress, None, start_ms + 1).unwrap();

        let mut completed =
            new_task(archived.id, None, "Completed".into(), "a2".into(), start_ms).unwrap();
        completed = transition_task(completed, TaskStatus::Done, None, end_ms - 1).unwrap();

        let mut wont_do =
            new_task(active.id, None, "Wont do".into(), "a3".into(), start_ms).unwrap();
        wont_do.due = Some(TaskDue::date_time(start_ms, "UTC").unwrap());
        wont_do = transition_task(
            wont_do,
            TaskStatus::WontDo,
            Some("obsolete".into()),
            start_ms + 2,
        )
        .unwrap();

        let mut excluded_end =
            new_task(active.id, None, "At end".into(), "a4".into(), start_ms).unwrap();
        excluded_end.due = Some(TaskDue::date("2026-03-09").unwrap());
        excluded_end.scheduled_at = Some(end_ms);

        let mut deleted =
            new_task(active.id, None, "Deleted".into(), "a5".into(), start_ms).unwrap();
        deleted.due = Some(TaskDue::date("2026-03-08").unwrap());
        deleted.deleted_at = Some(start_ms + 3);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        for task in [dual, datetime, completed, wont_do, excluded_end, deleted] {
            tasks.insert(task).unwrap();
        }

        let occurrences = tasks.list_calendar_occurrences(&range).unwrap();
        assert_eq!(occurrences.len(), 5);
        assert_eq!(
            occurrences
                .iter()
                .filter(|occurrence| occurrence.task.content.title == "Dual")
                .count(),
            2
        );
        assert!(occurrences.iter().any(|occurrence| matches!(
            &occurrence.kind,
            CalendarOccurrenceKind::DateDue { due_on } if due_on.as_str() == "2026-03-08"
        )));
        assert!(occurrences.iter().any(|occurrence| matches!(
            occurrence.kind,
            CalendarOccurrenceKind::DateTimeDue { due_at, .. } if due_at.as_millis() == end_ms - 1
        )));
        assert!(occurrences.iter().any(|occurrence| matches!(
            occurrence.kind,
            CalendarOccurrenceKind::Scheduled { scheduled_at } if scheduled_at.as_millis() == start_ms
        )));
        assert!(occurrences.iter().any(|occurrence| {
            occurrence.list_archived
                && occurrence.list_name == "Archive"
                && matches!(occurrence.kind, CalendarOccurrenceKind::Completed { .. })
        }));
        assert!(occurrences.iter().any(|occurrence| {
            occurrence.task.status == TaskStatus::WontDo
                && matches!(occurrence.kind, CalendarOccurrenceKind::Completed { .. })
        }));
        assert!(!occurrences.iter().any(|occurrence| {
            occurrence.task.status == TaskStatus::WontDo
                && matches!(occurrence.kind, CalendarOccurrenceKind::DateTimeDue { .. })
        }));
        assert!(!occurrences
            .iter()
            .any(|occurrence| occurrence.task.content.title == "At end"
                || occurrence.task.content.title == "Deleted"));
    }

    #[test]
    fn update_with_undo_records_edit_and_restores_previous_snapshot() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        repository.insert(task.clone()).unwrap();

        let updated =
            update_title(task.clone(), "Undo me".to_string(), task.updated_at + 1).unwrap();
        let undo = repository
            .update_with_undo(
                task.clone(),
                updated.clone(),
                TaskUndoOperation::Edit,
                updated.updated_at,
            )
            .unwrap();

        assert_eq!(repository.latest_unconsumed_undo().unwrap().unwrap(), undo);
        assert_eq!(repository.get(task.id).unwrap(), updated);

        let restored = repository
            .undo_task_operation(undo.id, updated.updated_at + 1)
            .unwrap();

        assert_eq!(restored, task);
        assert_eq!(repository.get(task.id).unwrap(), task);
        assert!(repository.latest_unconsumed_undo().unwrap().is_none());
        assert!(matches!(
            repository.undo_task_operation(undo.id, updated.updated_at + 2),
            Err(StorageError::UndoConsumed(id)) if id == undo.id
        ));
    }

    #[test]
    fn delete_undo_entries_are_not_returned_as_latest_undo() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        repository.insert(task.clone()).unwrap();

        let mut deleted = task.clone();
        deleted.deleted_at = Some(task.updated_at + 1);
        deleted.updated_at = task.updated_at + 1;
        repository
            .update_with_undo(
                task.clone(),
                deleted.clone(),
                TaskUndoOperation::Delete,
                deleted.updated_at,
            )
            .unwrap();

        assert!(repository.latest_unconsumed_undo().unwrap().is_none());
    }

    #[test]
    fn complete_undo_entry_restores_task_state() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        repository.insert(task.clone()).unwrap();

        let done =
            transition_task(task.clone(), TaskStatus::Done, None, task.updated_at + 1).unwrap();
        let complete_undo = repository
            .update_with_undo(
                task.clone(),
                done.clone(),
                TaskUndoOperation::Complete,
                done.updated_at,
            )
            .unwrap();

        assert_eq!(
            repository
                .undo_task_operation(complete_undo.id, done.updated_at + 1)
                .unwrap()
                .status,
            TaskStatus::Todo
        );
    }

    #[test]
    fn undo_rejects_edit_conflict_after_later_update() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        repository.insert(task.clone()).unwrap();

        let edited =
            update_title(task.clone(), "First edit".to_string(), task.updated_at + 1).unwrap();
        let undo = repository
            .update_with_undo(
                task.clone(),
                edited.clone(),
                TaskUndoOperation::Edit,
                edited.updated_at,
            )
            .unwrap();
        let second_edit = update_title(
            edited.clone(),
            "Second edit".to_string(),
            edited.updated_at + 1,
        )
        .unwrap();
        repository.update(second_edit).unwrap();

        assert!(matches!(
            repository.undo_task_operation(undo.id, edited.updated_at + 2),
            Err(StorageError::UndoConflict(id)) if id == task.id
        ));
    }

    #[test]
    fn complete_undo_rejects_physically_deleted_current_task() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        repository.insert(task.clone()).unwrap();

        let done =
            transition_task(task.clone(), TaskStatus::Done, None, task.updated_at + 1).unwrap();
        let undo = repository
            .update_with_undo(
                task.clone(),
                done.clone(),
                TaskUndoOperation::Complete,
                done.updated_at,
            )
            .unwrap();
        repository.delete_subtree(done.id).unwrap();

        assert!(matches!(
            repository.undo_task_operation(undo.id, task.updated_at + 3),
            Err(StorageError::NotFound(id)) if id == undo.id
        ));
    }

    #[test]
    fn update_returns_not_found_for_missing_task_and_list() {
        let file = NamedTempFile::new().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        let task = sample_task();
        assert!(matches!(
            task_repository.update(task.clone()),
            Err(StorageError::NotFound(id)) if id == task.id
        ));

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        let list = sample_list("a0");
        assert!(matches!(
            list_repository.update(list.clone()),
            Err(StorageError::NotFound(id)) if id == list.id
        ));
    }

    #[test]
    fn sqlite_write_tx_commits_domain_and_sync_state_together() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteTaskRepository::new(connection);
            repository.insert(task.clone()).unwrap();
        }

        let edited = update_title(
            task.clone(),
            "Transactional edit".to_string(),
            task.updated_at + 1,
        )
        .unwrap();
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert_eq!(write_tx.get_task(task.id).unwrap(), task);
        write_tx
            .update_with_undo(
                task.clone(),
                edited.clone(),
                TaskUndoOperation::Edit,
                edited.updated_at,
            )
            .unwrap();
        write_tx
            .set_setting("sync_local_hlc", "encoded-hlc", edited.updated_at)
            .unwrap();
        let op_id = Uuid::now_v7();
        write_tx
            .put_outbox_head(new_live_outbox(
                task.id,
                "tasks",
                op_id,
                None,
                "encoded-hlc",
                "encoded-hlc",
                vec![1, 2, 3],
            ))
            .unwrap();
        write_tx
            .put_record_state(live_record_state(
                task.id,
                "tasks",
                Some("encoded-hlc"),
                "encoded-hlc",
                r#"{"title":"Transactional edit"}"#,
                edited.updated_at,
            ))
            .unwrap();
        assert_eq!(write_tx.list_outbox_heads(10).unwrap().len(), 1);
        write_tx.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        assert_eq!(repository.get(task.id).unwrap(), edited);
        drop(repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let settings = SqliteSettingsRepository::new(connection);
        assert_eq!(
            settings.get_setting("sync_local_hlc").unwrap().as_deref(),
            Some("encoded-hlc")
        );
        drop(settings);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert_eq!(sync.list_outbox_heads(10).unwrap().len(), 1);
        assert_eq!(
            sync.get_record_state("tasks", task.id).unwrap(),
            Some(live_record_state(
                task.id,
                "tasks",
                Some("encoded-hlc"),
                "encoded-hlc",
                r#"{"title":"Transactional edit"}"#,
                edited.updated_at,
            ))
        );
    }

    #[test]
    fn sqlite_write_tx_drop_rolls_back_domain_and_sync_state_together() {
        let file = NamedTempFile::new().unwrap();
        let task = sample_task();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteTaskRepository::new(connection);
            repository.insert(task.clone()).unwrap();
        }

        let edited = update_title(
            task.clone(),
            "Rolled back edit".to_string(),
            task.updated_at + 1,
        )
        .unwrap();
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        {
            let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
            write_tx
                .update_with_undo(
                    task.clone(),
                    edited.clone(),
                    TaskUndoOperation::Edit,
                    edited.updated_at,
                )
                .unwrap();
            write_tx
                .set_setting("sync_local_hlc", "rolled-back-hlc", edited.updated_at)
                .unwrap();
            write_tx
                .put_outbox_head(new_live_outbox(
                    task.id,
                    "tasks",
                    Uuid::now_v7(),
                    None,
                    "rolled-back-hlc",
                    "rolled-back-hlc",
                    vec![4, 5, 6],
                ))
                .unwrap();
            write_tx
                .put_record_state(live_record_state(
                    task.id,
                    "tasks",
                    Some("rolled-back-hlc"),
                    "rolled-back-hlc",
                    r#"{"title":"Rolled back edit"}"#,
                    edited.updated_at,
                ))
                .unwrap();
        }
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        assert_eq!(repository.get(task.id).unwrap(), task);
        assert!(repository.latest_unconsumed_undo().unwrap().is_none());
        drop(repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let settings = SqliteSettingsRepository::new(connection);
        assert_eq!(settings.get_setting("sync_local_hlc").unwrap(), None);
        drop(settings);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert!(sync.list_outbox_heads(10).unwrap().is_empty());
        assert_eq!(sync.get_record_state("tasks", task.id).unwrap(), None);
    }

    #[test]
    fn owned_sqlite_write_tx_commits_domain_hlc_record_state_and_outbox() {
        let file = NamedTempFile::new().unwrap();
        let mut list = sample_list("a0");
        list.is_default = true;
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let op_id = Uuid::now_v7();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .set_setting("sync_local_hlc", "owned-hlc", task.updated_at)
            .unwrap();
        transaction.upsert_list_for_sync(list.clone()).unwrap();
        transaction.upsert_task_for_sync(task.clone()).unwrap();
        transaction
            .put_record_state(live_record_state(
                task.id,
                "tasks",
                Some("owned-hlc"),
                "owned-hlc",
                r#"{"title":"Owned commit"}"#,
                task.updated_at,
            ))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                task.id,
                "tasks",
                op_id,
                Some("base-hlc"),
                "owned-hlc",
                "owned-hlc",
                vec![1, 2, 3],
            ))
            .unwrap();
        transaction
            .set_cursor("default", 7, task.updated_at)
            .unwrap();
        assert_eq!(transaction.default_list_id().unwrap(), Some(list.id));
        assert_eq!(transaction.get_list(list.id).unwrap(), Some(list.clone()));
        assert_eq!(transaction.get_task(task.id).unwrap(), Some(task.clone()));
        assert!(transaction.has_outbox_head("tasks", task.id).unwrap());
        assert_eq!(transaction.list_outbox_heads(10).unwrap().len(), 1);
        assert_eq!(transaction.get_cursor("default").unwrap().unwrap().seq, 7);
        let connection = transaction.commit().unwrap();

        assert_eq!(
            get_setting_on(&connection, "sync_local_hlc")
                .unwrap()
                .as_deref(),
            Some("owned-hlc")
        );
        assert_eq!(get_list_on(&connection, list.id).unwrap(), list);
        assert_eq!(get_task_on(&connection, task.id).unwrap(), task);
        assert!(get_record_state_on(&connection, "tasks", task.id)
            .unwrap()
            .is_some());
        assert_eq!(list_outbox_heads_on(&connection, 10).unwrap().len(), 1);
        assert_eq!(
            get_cursor_on(&connection, "default").unwrap().unwrap().seq,
            7
        );
    }

    #[test]
    fn owned_sqlite_write_tx_drop_rolls_back_domain_hlc_record_state_and_outbox() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            transaction
                .set_setting("sync_local_hlc", "rolled-back-owned-hlc", task.updated_at)
                .unwrap();
            transaction.upsert_list_for_sync(list.clone()).unwrap();
            transaction.upsert_task_for_sync(task.clone()).unwrap();
            transaction
                .put_record_state(live_record_state(
                    task.id,
                    "tasks",
                    None,
                    "rolled-back-owned-hlc",
                    r#"{"title":"Owned rollback"}"#,
                    task.updated_at,
                ))
                .unwrap();
            transaction
                .put_outbox_head(new_live_outbox(
                    task.id,
                    "tasks",
                    Uuid::now_v7(),
                    None,
                    "rolled-back-owned-hlc",
                    "rolled-back-owned-hlc",
                    vec![4, 5, 6],
                ))
                .unwrap();
            transaction
                .set_cursor("default", 9, task.updated_at)
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(get_setting_on(&connection, "sync_local_hlc").unwrap(), None);
        assert!(matches!(
            get_list_on(&connection, list.id),
            Err(StorageError::NotFound(id)) if id == list.id
        ));
        assert!(matches!(
            get_task_on(&connection, task.id),
            Err(StorageError::NotFound(id)) if id == task.id
        ));
        assert_eq!(
            get_record_state_on(&connection, "tasks", task.id).unwrap(),
            None
        );
        assert!(list_outbox_heads_on(&connection, 10).unwrap().is_empty());
        assert_eq!(get_cursor_on(&connection, "default").unwrap(), None);
    }

    #[test]
    fn sqlite_write_tx_commits_task_and_list_crud_without_nested_transactions() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut repository = SqliteListRepository::new(connection);
            repository.insert(list.clone()).unwrap();
        }

        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let mut prepared = task.clone();
        prepared.content.note = "Updated before status transition".to_string();
        prepared.updated_at += 1;
        let done = transition_task(
            prepared.clone(),
            TaskStatus::Done,
            None,
            prepared.updated_at + 1,
        )
        .unwrap();
        let mut renamed_list = list.clone();
        renamed_list.name = "Renamed transactionally".to_string();
        renamed_list.updated_at += 1;

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert_eq!(write_tx.get_list(list.id).unwrap(), list);
        write_tx.update_list(renamed_list.clone()).unwrap();
        write_tx.insert_task(task.clone()).unwrap();
        write_tx.update_task(prepared.clone()).unwrap();
        assert_eq!(
            write_tx.list_active_tasks_by_list(list.id).unwrap(),
            vec![prepared.clone()]
        );
        let undo = write_tx
            .update_task_with_undo(
                prepared,
                done.clone(),
                TaskUndoOperation::Complete,
                done.updated_at,
            )
            .unwrap();
        assert_eq!(write_tx.get_task(done.id).unwrap(), done);
        write_tx.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let list_repository = SqliteListRepository::new(connection);
        assert_eq!(list_repository.get(list.id).unwrap(), renamed_list);
        drop(list_repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let task_repository = SqliteTaskRepository::new(connection);
        assert_eq!(task_repository.get(done.id).unwrap(), done);
        assert_eq!(
            task_repository.latest_unconsumed_undo().unwrap(),
            Some(undo)
        );
    }

    #[test]
    fn sqlite_write_tx_snapshots_and_commits_task_subtree_delete() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut parent = sample_task();
        parent.list_id = list.id;
        parent.parent_task_id = None;
        parent.sort_order = "a0".to_string();
        let mut child = sample_task();
        child.list_id = list.id;
        child.parent_task_id = Some(parent.id);
        child.sort_order = "a1".to_string();
        let mut sibling = sample_task();
        sibling.list_id = list.id;
        sibling.parent_task_id = None;
        sibling.sort_order = "a2".to_string();

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut tasks = SqliteTaskRepository::new(connection);
            tasks.insert(parent.clone()).unwrap();
            tasks.insert(child.clone()).unwrap();
            tasks.insert(sibling.clone()).unwrap();
        }

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert_eq!(
            write_tx.list_task_subtree(parent.id).unwrap(),
            vec![parent.clone(), child.clone()]
        );
        assert_eq!(
            write_tx.list_tasks_by_list(list.id).unwrap(),
            vec![parent.clone(), child.clone(), sibling.clone()]
        );
        assert_eq!(write_tx.delete_task_subtree(parent.id).unwrap(), 2);
        assert!(matches!(
            write_tx.get_task(parent.id),
            Err(StorageError::NotFound(id)) if id == parent.id
        ));
        assert!(matches!(
            write_tx.get_task(child.id),
            Err(StorageError::NotFound(id)) if id == child.id
        ));
        assert_eq!(write_tx.get_task(sibling.id).unwrap(), sibling.clone());
        write_tx.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert!(matches!(
            tasks.get(parent.id),
            Err(StorageError::NotFound(id)) if id == parent.id
        ));
        assert!(matches!(
            tasks.get(child.id),
            Err(StorageError::NotFound(id)) if id == child.id
        ));
        assert_eq!(tasks.get(sibling.id).unwrap(), sibling);
    }

    #[test]
    fn sqlite_write_tx_drop_rolls_back_list_and_task_physical_delete() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut default_list = sample_list("a1");
        default_list.is_default = true;
        let mut active = sample_task();
        active.list_id = list.id;
        active.parent_task_id = None;
        active.sort_order = "a0".to_string();
        let mut logically_deleted = sample_task();
        logically_deleted.list_id = list.id;
        logically_deleted.parent_task_id = None;
        logically_deleted.sort_order = "a1".to_string();
        logically_deleted.deleted_at = Some(logically_deleted.updated_at + 1);

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(list.clone()).unwrap();
            lists.insert(default_list).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut tasks = SqliteTaskRepository::new(connection);
            tasks.insert(active.clone()).unwrap();
            tasks.insert(logically_deleted.clone()).unwrap();
        }

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        {
            let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
            assert_eq!(
                write_tx.list_tasks_by_list(list.id).unwrap(),
                vec![active.clone(), logically_deleted.clone()]
            );
            assert_eq!(write_tx.delete_list_and_rehome_tasks(list.id).unwrap(), 2);
            assert!(matches!(
                write_tx.get_list(list.id),
                Err(StorageError::NotFound(id)) if id == list.id
            ));
        }
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let lists = SqliteListRepository::new(connection);
        assert_eq!(lists.get(list.id).unwrap(), list);
        drop(lists);
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(active.id).unwrap(), active);
        assert_eq!(tasks.get(logically_deleted.id).unwrap(), logically_deleted);
    }

    #[test]
    fn sqlite_write_tx_commits_list_delete_and_protects_default_list() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let mut default_list = sample_list("a1");
        default_list.is_default = true;

        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(list.clone()).unwrap();
            lists.insert(default_list.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut tasks = SqliteTaskRepository::new(connection);
            tasks.insert(task.clone()).unwrap();
        }

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert!(matches!(
            write_tx.delete_list_and_rehome_tasks(default_list.id),
            Err(StorageError::DefaultListProtected { list_id, .. }) if list_id == default_list.id
        ));
        assert_eq!(write_tx.delete_list_and_rehome_tasks(list.id).unwrap(), 1);
        write_tx.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let lists = SqliteListRepository::new(connection);
        assert!(matches!(
            lists.get(list.id),
            Err(StorageError::NotFound(id)) if id == list.id
        ));
        assert_eq!(lists.get(default_list.id).unwrap(), default_list);
        drop(lists);
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(task.id).unwrap().list_id, default_list.id);
    }

    #[test]
    fn sqlite_write_tx_drop_rolls_back_undo_restore_and_list_update() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("a0");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let edited = update_title(
            task.clone(),
            "Awaiting undo".to_string(),
            task.updated_at + 1,
        )
        .unwrap();
        let undo = {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut list_repository = SqliteListRepository::new(connection);
            list_repository.insert(list.clone()).unwrap();
            drop(list_repository);

            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut task_repository = SqliteTaskRepository::new(connection);
            task_repository.insert(task.clone()).unwrap();
            task_repository
                .update_with_undo(
                    task.clone(),
                    edited.clone(),
                    TaskUndoOperation::Edit,
                    edited.updated_at,
                )
                .unwrap()
        };
        let mut archived_list = list.clone();
        archived_list.archived_at = Some(edited.updated_at + 1);
        archived_list.updated_at = edited.updated_at + 1;

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        {
            let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
            assert_eq!(
                write_tx
                    .undo_task_operation(undo.id, edited.updated_at + 1)
                    .unwrap(),
                task
            );
            write_tx.update_list(archived_list).unwrap();
            assert_eq!(write_tx.get_task(task.id).unwrap(), task);
        }
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let list_repository = SqliteListRepository::new(connection);
        assert_eq!(list_repository.get(list.id).unwrap(), list);
        drop(list_repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        assert_eq!(task_repository.get(task.id).unwrap(), edited);
        assert_eq!(
            task_repository
                .undo_task_operation(undo.id, edited.updated_at + 2)
                .unwrap(),
            task
        );
    }

    #[test]
    fn encrypted_connections_have_finite_busy_timeout_and_write_tx_locks_immediately() {
        let file = NamedTempFile::new().unwrap();
        let mut first = open_encrypted(file.path(), &KEY).unwrap();
        let second = open_encrypted(file.path(), &KEY).unwrap();
        let busy_timeout_ms: i64 = second
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout_ms, 5_000);
        second.busy_timeout(Duration::ZERO).unwrap();

        let _write_tx = SqliteWriteTx::begin(&mut first).unwrap();
        let result = second.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params!["other_writer", "blocked", 1],
        );
        assert!(matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::DatabaseBusy
        ));
    }

    #[test]
    fn full_resync_progress_and_marks_roll_back_together() {
        let file = NamedTempFile::new().unwrap();
        let generation_id = Uuid::now_v7();
        let record_id = Uuid::now_v7();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 17, 100)
            .unwrap();
        transaction.rollback().unwrap();
        let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(repository.load_full_resync().unwrap(), None);
        drop(repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 17, 100)
            .unwrap();
        transaction.commit().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .mark_full_resync_record(generation_id, "tasks", record_id)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 110)
            .unwrap();
        transaction.rollback().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let progress = load_full_resync_on(&connection).unwrap().unwrap();
        assert_eq!(progress.phase, FullResyncPhase::Base);
        let mark_count: i64 = connection
            .query_row("SELECT count(*) FROM sync_full_resync_marks", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(mark_count, 0);
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 120)
            .unwrap();
        transaction.commit().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .advance_full_resync_delta(generation_id, 18, 130)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 18, 131)
            .unwrap();
        transaction.rollback().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let progress = load_full_resync_on(&connection).unwrap().unwrap();
        assert_eq!(progress.phase, FullResyncPhase::Delta);
        assert_eq!(progress.delta_cursor, 17);
        assert_eq!(progress.closure_high_water, None);
    }

    #[test]
    fn full_resync_sweep_preserves_marks_but_purges_server_seen_absent_outbox() {
        let file = NamedTempFile::new().unwrap();
        let list = sample_list("00000000000000010000000000000000");
        let mut remote_task = sample_task();
        remote_task.id = Uuid::now_v7();
        remote_task.list_id = list.id;
        remote_task.parent_task_id = None;
        let mut pending_task = remote_task.clone();
        pending_task.id = Uuid::now_v7();
        let mut absent_task = remote_task.clone();
        absent_task.id = Uuid::now_v7();

        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        {
            let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
            transaction.insert_list(list.clone()).unwrap();
            for task in [&remote_task, &pending_task, &absent_task] {
                transaction.insert_task(task.clone()).unwrap();
                transaction
                    .put_record_state(live_record_state(
                        task.id,
                        "tasks",
                        Some("r1"),
                        "m1",
                        "{}",
                        1,
                    ))
                    .unwrap();
            }
            transaction
                .put_record_state(live_record_state(
                    list.id,
                    "lists",
                    Some("r1"),
                    "m1",
                    "{}",
                    1,
                ))
                .unwrap();
            transaction
                .put_outbox_head(new_live_outbox(
                    pending_task.id,
                    "tasks",
                    Uuid::now_v7(),
                    Some("r1"),
                    "r2",
                    "m2",
                    vec![1],
                ))
                .unwrap();
            transaction.commit().unwrap();
        }

        let generation_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 5, 10)
            .unwrap();
        transaction
            .mark_full_resync_record(generation_id, "lists", list.id)
            .unwrap();
        transaction
            .mark_full_resync_record(generation_id, "tasks", remote_task.id)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 11)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 5, 12)
            .unwrap();
        transaction.commit().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let rolled_back = transaction
            .sweep_full_resync_batch(generation_id, 1, 19)
            .unwrap();
        assert_eq!(rolled_back.scanned_records, 1);
        transaction.rollback().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let progress = load_full_resync_on(&connection).unwrap().unwrap();
        assert_eq!(progress.sweep_cursor, None);
        let state_count: i64 = connection
            .query_row("SELECT count(*) FROM sync_record_states", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state_count, 4);

        let mut total = FullResyncSweepSummary::default();
        for now in 20..30 {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            let batch = transaction
                .sweep_full_resync_batch(generation_id, 1, now)
                .unwrap();
            transaction.commit().unwrap();
            total.scanned_records += batch.scanned_records;
            total.swept_lists += batch.swept_lists;
            total.swept_tasks += batch.swept_tasks;
            total.swept_record_states += batch.swept_record_states;
            if batch.scanned_records == 0 {
                break;
            }
        }
        assert_eq!(total.scanned_records, 4);
        assert_eq!(total.swept_tasks, 2);
        assert_eq!(total.swept_record_states, 2);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        assert_eq!(
            transaction
                .finalize_full_resync(generation_id, "default", 30)
                .unwrap(),
            5
        );
        transaction.rollback().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(get_cursor_on(&connection, "default").unwrap(), None);
        assert!(load_full_resync_on(&connection).unwrap().is_some());

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        assert_eq!(
            transaction
                .finalize_full_resync(generation_id, "default", 31)
                .unwrap(),
            5
        );
        transaction.commit().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert!(get_task_on(&connection, remote_task.id).is_ok());
        assert!(matches!(
            get_task_on(&connection, pending_task.id),
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            get_task_on(&connection, absent_task.id),
            Err(StorageError::NotFound(_))
        ));
        assert!(!has_outbox_head_on(&connection, "tasks", pending_task.id).unwrap());
        assert_eq!(
            get_cursor_on(&connection, "default").unwrap().unwrap().seq,
            5
        );
        assert_eq!(load_full_resync_on(&connection).unwrap(), None);
    }

    #[test]
    fn full_resync_sweeps_absent_series_then_template_across_stable_batches() {
        let file = NamedTempFile::new().unwrap();
        let template = sample_template();
        let schedule = sample_schedule(template.id);
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.upsert_template(template.clone()).unwrap();
        transaction.upsert_series(schedule.clone()).unwrap();
        for (id, collection) in [(template.id, "templates"), (schedule.id, "task_series")] {
            transaction
                .put_record_state(live_record_state(
                    id,
                    collection,
                    Some("server-r1"),
                    "m1",
                    "{}",
                    1,
                ))
                .unwrap();
        }
        transaction.commit().unwrap();

        let generation_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 0, 10)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 11)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 0, 12)
            .unwrap();
        transaction.commit().unwrap();

        let mut total = FullResyncSweepSummary::default();
        for now in 20..24 {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            let batch = transaction
                .sweep_full_resync_batch(generation_id, 1, now)
                .unwrap();
            transaction.commit().unwrap();
            total.scanned_records += batch.scanned_records;
            total.swept_templates += batch.swept_templates;
            total.swept_task_series += batch.swept_task_series;
            total.swept_record_states += batch.swept_record_states;
            if batch.scanned_records == 0 {
                break;
            }
        }

        assert_eq!(total.scanned_records, 2);
        assert_eq!(total.swept_task_series, 1);
        assert_eq!(total.swept_templates, 1);
        assert_eq!(total.swept_record_states, 2);
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert!(matches!(
            get_series_on(&connection, schedule.id),
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            get_template_on(&connection, template.id),
            Err(StorageError::NotFound(_))
        ));
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        assert_eq!(
            transaction
                .finalize_full_resync(generation_id, "default", 30)
                .unwrap(),
            0
        );
        transaction.commit().unwrap();
    }

    #[test]
    fn full_resync_preserves_never_synced_series_independently_of_template() {
        let file = NamedTempFile::new().unwrap();
        let template = sample_template();
        let schedule = sample_schedule(template.id);
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.upsert_template(template.clone()).unwrap();
        transaction.upsert_series(schedule.clone()).unwrap();
        for (id, collection) in [(template.id, "templates"), (schedule.id, "task_series")] {
            transaction
                .put_record_state(live_record_state(id, collection, None, "m1", "{}", 1))
                .unwrap();
            transaction
                .put_outbox_head(new_live_outbox(
                    id,
                    collection,
                    Uuid::now_v7(),
                    None,
                    "r1",
                    "m1",
                    vec![1],
                ))
                .unwrap();
        }
        transaction.commit().unwrap();

        let generation_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 0, 10)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 11)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 0, 12)
            .unwrap();
        transaction.commit().unwrap();

        loop {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            let batch = transaction
                .sweep_full_resync_batch(generation_id, 1, 20)
                .unwrap();
            transaction.commit().unwrap();
            if batch.scanned_records == 0 {
                break;
            }
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(get_series_on(&connection, schedule.id).unwrap(), schedule);
        assert_eq!(get_template_on(&connection, template.id).unwrap(), template);
        assert!(has_outbox_head_on(&connection, "task_series", schedule.id).unwrap());
        assert!(has_outbox_head_on(&connection, "templates", template.id).unwrap());
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .finalize_full_resync(generation_id, "default", 30)
            .unwrap();
        transaction.commit().unwrap();
    }

    #[test]
    fn full_resync_preserves_valid_never_synced_list_and_task_in_dependency_order() {
        let file = NamedTempFile::new().unwrap();
        let tenant_id = Uuid::now_v7();
        let list = sample_list("00000000000000010000000000000000");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let mut local_crypto =
            SqliteLocalCryptoRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        local_crypto
            .bind_tenant_root(
                LocalProfileBinding {
                    tenant_id,
                    user_id: Uuid::now_v7(),
                    device_id: Uuid::now_v7(),
                    bound_at: 1,
                    updated_at: 1,
                },
                &local_tenant_root_bundle(tenant_id, 1),
            )
            .unwrap();
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.insert_list(list.clone()).unwrap();
        transaction.insert_task(task.clone()).unwrap();
        for (id, collection) in [(list.id, "lists"), (task.id, "tasks")] {
            transaction
                .put_record_state(live_record_state(id, collection, None, "m1", "{}", 1))
                .unwrap();
            transaction
                .put_outbox_head(new_live_outbox(
                    id,
                    collection,
                    Uuid::now_v7(),
                    None,
                    "r1",
                    "m1",
                    vec![1],
                ))
                .unwrap();
        }
        transaction.commit().unwrap();

        let generation_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 0, 10)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 11)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 0, 12)
            .unwrap();
        transaction.commit().unwrap();

        loop {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            let batch = transaction
                .sweep_full_resync_batch(generation_id, 10, 20)
                .unwrap();
            transaction.commit().unwrap();
            if batch.scanned_records == 0 {
                break;
            }
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert!(get_list_on(&connection, list.id).is_ok());
        assert!(get_task_on(&connection, task.id).is_ok());
        let outbox = list_outbox_heads_on(&connection, 10).unwrap();
        assert_eq!(outbox.len(), 2);
        assert_eq!(outbox[0].collection, "lists");
        assert_eq!(outbox[1].collection, "tasks");
    }

    #[test]
    fn full_resync_preserves_never_synced_task_under_remote_current_list() {
        let file = NamedTempFile::new().unwrap();
        let tenant_id = Uuid::now_v7();
        let list = sample_list("00000000000000010000000000000000");
        let mut task = sample_task();
        task.list_id = list.id;
        task.parent_task_id = None;
        let mut crypto =
            SqliteLocalCryptoRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        crypto
            .bind_tenant_root(
                LocalProfileBinding {
                    tenant_id,
                    user_id: Uuid::now_v7(),
                    device_id: Uuid::now_v7(),
                    bound_at: 1,
                    updated_at: 1,
                },
                &local_tenant_root_bundle(tenant_id, 1),
            )
            .unwrap();
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.insert_list(list.clone()).unwrap();
        transaction.insert_task(task.clone()).unwrap();
        transaction
            .put_record_state(live_record_state(
                list.id,
                "lists",
                Some("r1"),
                "m1",
                "{}",
                1,
            ))
            .unwrap();
        transaction
            .put_record_state(live_record_state(task.id, "tasks", None, "m1", "{}", 1))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                task.id,
                "tasks",
                Uuid::now_v7(),
                None,
                "r1",
                "m1",
                vec![1],
            ))
            .unwrap();
        transaction.commit().unwrap();

        let generation_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .start_full_resync(generation_id, 1, 1, 10)
            .unwrap();
        transaction
            .mark_full_resync_record(generation_id, "lists", list.id)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 11)
            .unwrap();
        transaction
            .advance_full_resync_delta(generation_id, 1, 12)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 1, 13)
            .unwrap();
        transaction.commit().unwrap();

        loop {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            let batch = transaction
                .sweep_full_resync_batch(generation_id, 10, 20)
                .unwrap();
            transaction.commit().unwrap();
            if batch.scanned_records == 0 {
                break;
            }
        }
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert!(get_task_on(&connection, task.id).is_ok());
        assert!(has_outbox_head_on(&connection, "tasks", task.id).unwrap());
    }

    #[test]
    fn v16_empty_database_migrates_to_typed_due_and_rejects_mixed_shape() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_raw_encrypted(file.path(), &KEY);
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY NOT NULL, list_id TEXT NOT NULL,
                    parent_task_id TEXT, title TEXT NOT NULL, note TEXT NOT NULL,
                    status TEXT NOT NULL, priority INTEGER NOT NULL, due_at INTEGER,
                    scheduled_at INTEGER, estimated_minutes INTEGER, sort_order TEXT NOT NULL,
                    completed_at INTEGER, closed_reason TEXT, deleted_at INTEGER,
                    assignee TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 16;",
            )
            .unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert!(table_columns_raw(&connection, "tasks")
            .unwrap()
            .iter()
            .any(|column| column == "due_kind"));
        assert!(connection
            .execute(
                "INSERT INTO tasks (
                    id, list_id, title, note, status, priority,
                    due_kind, due_on, sort_order, created_at, updated_at
                 ) VALUES ('task', 'list', 'title', '', 'todo', 0,
                           'datetime', '2026-07-12', 'a0', 1, 1)",
                [],
            )
            .is_err());
    }

    #[test]
    fn v17_to_v22_preserves_domain_and_resets_pre_protocol_v8_transport_state() {
        let file = NamedTempFile::new().unwrap();
        let list = new_list("Preserved".into(), "a0".into(), 100).unwrap();
        let task = new_task(list.id, None, "Preserved task".into(), "a0".into(), 100).unwrap();
        let quarantine_id = Uuid::now_v7();
        let generation_id = Uuid::now_v7();
        create_baseline_v1_database(file.path(), &KEY, true);
        {
            let mut connection = open_raw_encrypted(file.path(), &KEY);
            let transaction = connection.transaction().unwrap();
            for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 17) {
                (migration.apply)(&transaction).unwrap();
            }
            set_user_version(&transaction, 17).unwrap();
            transaction.commit().unwrap();
            insert_list_on(&connection, &list).unwrap();
            insert_task_pre_v20(&connection, &task);
            connection
                .execute_batch(&format!(
                    "INSERT INTO sync_outbox (
                         record_id, collection, op_id, revision_hlc, state_kind,
                         semantic_hlc, blob, created_at
                     ) VALUES ('{task_id}', 'tasks', '{op_id}', 'r1', 'live', 'm1', x'01', 1);
                     INSERT INTO sync_record_states (
                         record_id, collection, current_revision_hlc, state_kind,
                         semantic_hlc, plaintext_json, updated_at
                     ) VALUES ('{task_id}', 'tasks', 'r0', 'tombstone', 'd1', NULL, 1);
                     INSERT INTO sync_cursors(name, seq, updated_at) VALUES ('main', 42, 1);
                     INSERT INTO sync_record_origins(record_id, collection, origin_kind, updated_at)
                     VALUES ('{task_id}', 'tasks', 'server_seen', 1);
                     INSERT INTO sync_quarantine (
                         record_id, collection, seq, revision_hlc, state_kind,
                         semantic_hlc, blob, reason, required_list_id,
                         first_failed_at, last_failed_at, attempt_count
                     ) VALUES ('{quarantine_id}', 'tasks', 7, 'r7', 'live', 'm7', x'02',
                               'corrupt_envelope', NULL, 2, 3, 1);
                     INSERT INTO sync_full_resync_state (
                         singleton, generation_id, phase, base_seq,
                         base_cursor_collection, base_cursor_record_id, delta_cursor,
                         closure_high_water, sweep_cursor_collection, sweep_cursor_record_id,
                         started_at, updated_at, continuity_generation
                     ) VALUES (1, '{generation_id}', 'sweep', 42,
                               'tasks', '{task_id}', 42, 42, 'tasks', '{task_id}', 1, 2, 3);
                     INSERT INTO sync_full_resync_marks(generation_id, collection, record_id)
                     VALUES ('{generation_id}', 'tasks', '{task_id}');
                     PRAGMA user_version = 17;",
                    task_id = task.id,
                    op_id = Uuid::now_v7(),
                    quarantine_id = quarantine_id,
                    generation_id = generation_id,
                ))
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(get_cursor_on(&connection, "main").unwrap(), None);
        for table in [
            "sync_outbox",
            "sync_record_states",
            "sync_quarantine",
            "sync_full_resync_state",
            "sync_full_resync_marks",
            "sync_record_origins",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0,
                "{table} must be reset for protocol v8"
            );
        }
        assert_eq!(
            SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
                .get(task.id)
                .unwrap(),
            task
        );
        for table in [
            "timer_sessions",
            "active_timer_session",
            "local_tenant_root_key_cache",
        ] {
            assert!(!table_columns_raw(&connection, table).unwrap().is_empty());
        }
        drop(connection);
        assert_eq!(
            read_user_version(&open_encrypted(file.path(), &KEY).unwrap()).unwrap(),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn v18_to_v19_adds_durable_list_aliases_and_reopens() {
        let file = NamedTempFile::new().unwrap();
        let mut old_default = sample_list("a0");
        old_default.is_default = true;
        let canonical = sample_list("a1");
        create_baseline_v1_database(file.path(), &KEY, true);
        {
            let mut connection = open_raw_encrypted(file.path(), &KEY);
            let transaction = connection.transaction().unwrap();
            for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 18) {
                (migration.apply)(&transaction).unwrap();
            }
            set_user_version(&transaction, 18).unwrap();
            transaction.commit().unwrap();
            insert_list_on(&connection, &old_default).unwrap();
            insert_list_on(&connection, &canonical).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            table_columns_raw(&connection, "list_aliases").unwrap(),
            vec!["alias_list_id", "canonical_list_id", "updated_at"]
        );
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .materialize_canonical_list(canonical.id)
            .unwrap();
        transaction
            .replace_list_aliases(canonical.id, &[old_default.id], 123)
            .unwrap();
        transaction.commit().unwrap();

        let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(
            repository.resolve_list_alias(old_default.id).unwrap(),
            canonical.id
        );
        assert_eq!(
            repository.list_list_aliases().unwrap(),
            vec![ListAlias {
                alias_list_id: old_default.id,
                canonical_list_id: canonical.id,
                updated_at: 123,
            }]
        );
        assert_eq!(
            read_user_version(&open_encrypted(file.path(), &KEY).unwrap()).unwrap(),
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn v19_to_v22_preserves_tasks_and_resets_legacy_recurrence_sync_metadata() {
        let file = NamedTempFile::new().unwrap();
        create_baseline_v1_database(file.path(), &KEY, true);
        let list = new_list("Preserved".into(), "a0".into(), 100).unwrap();
        let task = new_task(list.id, None, "Preserved task".into(), "a0".into(), 100).unwrap();
        let tombstone_id = Uuid::now_v7();
        {
            let mut connection = open_raw_encrypted(file.path(), &KEY);
            let transaction = connection.transaction().unwrap();
            for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 19) {
                (migration.apply)(&transaction).unwrap();
            }
            set_user_version(&transaction, 19).unwrap();
            transaction.commit().unwrap();
            insert_list_on(&connection, &list).unwrap();
            insert_task_pre_v20(&connection, &task);
            put_record_state_on(
                &connection,
                live_record_state(task.id, "tasks", Some("r0"), "m1", "{}", 1),
            )
            .unwrap();
            put_record_state_on(
                &connection,
                SyncRecordState {
                    record_id: tombstone_id,
                    collection: "tasks".to_string(),
                    current_revision_hlc: Some("r1".to_string()),
                    state: SyncRecordSemanticState::Tombstone {
                        delete_hlc: "d1".to_string(),
                    },
                    updated_at: 2,
                },
            )
            .unwrap();
            set_cursor_on(&connection, "main", 42, 3).unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            read_user_version(&connection).unwrap(),
            LATEST_SCHEMA_VERSION
        );
        let migrated = get_task_on(&connection, task.id).unwrap();
        assert_eq!(migrated.series_occurrence, None);
        assert_eq!(migrated.content.title, task.content.title);
        assert_eq!(get_cursor_on(&connection, "main").unwrap(), None);
        assert_eq!(
            get_record_state_on(&connection, "tasks", tombstone_id).unwrap(),
            None
        );
        for table in ["templates", "task_series"] {
            assert!(!table_columns_raw(&connection, table).unwrap().is_empty());
        }
        for collection in ["templates", "task_series"] {
            assert!(connection
                .execute(
                    "INSERT INTO sync_cursors(name, seq, updated_at) VALUES (?1, 0, 1)",
                    [format!("cursor-{collection}")],
                )
                .is_ok());
            assert!(connection
                .execute(
                    "INSERT INTO sync_record_states (
                         record_id, collection, current_revision_hlc, state_kind,
                         semantic_hlc, plaintext_json, updated_at
                     ) VALUES (?1, ?2, NULL, 'tombstone', 'd1', NULL, 1)",
                    params![Uuid::now_v7().to_string(), collection],
                )
                .is_ok());
        }
    }

    #[test]
    fn canonical_materialization_hides_alias_and_resolves_product_reads() {
        let file = NamedTempFile::new().unwrap();
        let mut old_default = sample_list("a0");
        old_default.is_default = true;
        let canonical = sample_list("a1");
        let alias_task = new_task(
            old_default.id,
            None,
            "Alias task".to_string(),
            "a0".to_string(),
            100,
        )
        .unwrap();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(old_default.clone()).unwrap();
            lists.insert(canonical.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteTaskRepository::new(connection)
                .insert(alias_task.clone())
                .unwrap();
        }

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .materialize_canonical_list(canonical.id)
            .unwrap();
        transaction
            .replace_list_aliases(canonical.id, &[old_default.id], 200)
            .unwrap();
        let mut moved = transaction
            .list_all_tasks_by_list_for_sync(old_default.id)
            .unwrap()
            .pop()
            .unwrap();
        moved.list_id = canonical.id;
        transaction.upsert_task_for_sync(moved).unwrap();
        transaction.commit().unwrap();

        let repository_task = new_task(
            old_default.id,
            None,
            "Repository alias task".to_string(),
            "a1".to_string(),
            201,
        )
        .unwrap();
        let mut tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        tasks.insert(repository_task.clone()).unwrap();
        assert_eq!(tasks.get(repository_task.id).unwrap().list_id, canonical.id);

        let tx_task = new_task(
            old_default.id,
            None,
            "Transaction alias task".to_string(),
            "a2".to_string(),
            202,
        )
        .unwrap();
        let mut connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.insert_task(tx_task.clone()).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            get_task_on(&connection, tx_task.id).unwrap().list_id,
            canonical.id
        );

        let lists = SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        let product_lists = lists.list_all().unwrap();
        assert_eq!(product_lists.len(), 1);
        assert_eq!(product_lists[0].id, canonical.id);
        assert!(product_lists[0].is_default);
        assert_eq!(lists.get(old_default.id).unwrap().id, canonical.id);
        assert_eq!(
            lists.get_raw_for_sync(old_default.id).unwrap().id,
            old_default.id
        );
        assert_eq!(lists.list_all_for_sync().unwrap().len(), 2);
        let tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        let resolved = tasks.list_active_by_list(old_default.id).unwrap();
        assert_eq!(resolved.len(), 3);
        assert!(resolved.iter().all(|task| task.list_id == canonical.id));
    }

    #[test]
    fn canonical_cutover_drop_rolls_back_domain_alias_task_and_transport_writes() {
        let file = NamedTempFile::new().unwrap();
        let mut old_default = sample_list("a0");
        old_default.is_default = true;
        let canonical = sample_list("a1");
        let alias_task = new_task(
            old_default.id,
            None,
            "Rollback task".to_string(),
            "a0".to_string(),
            100,
        )
        .unwrap();
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut lists = SqliteListRepository::new(connection);
            lists.insert(old_default.clone()).unwrap();
            lists.insert(canonical.clone()).unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            SqliteTaskRepository::new(connection)
                .insert(alias_task.clone())
                .unwrap();
        }
        {
            let connection = open_encrypted(file.path(), &KEY).unwrap();
            let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
            transaction
                .materialize_canonical_list(canonical.id)
                .unwrap();
            transaction
                .replace_list_aliases(canonical.id, &[old_default.id], 300)
                .unwrap();
            let mut moved = alias_task.clone();
            moved.list_id = canonical.id;
            transaction.upsert_task_for_sync(moved).unwrap();
            transaction
                .put_record_state(live_record_state(
                    alias_task.id,
                    "tasks",
                    Some("r1"),
                    "m1",
                    "{}",
                    300,
                ))
                .unwrap();
            // Drop without commit: OwnedSqliteWriteTx must roll back every table.
        }

        let lists = SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(lists.get_default().unwrap().unwrap().id, old_default.id);
        assert_eq!(lists.list_all().unwrap().len(), 2);
        let sync = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert!(sync.list_list_aliases().unwrap().is_empty());
        assert!(sync
            .get_record_state("tasks", alias_task.id)
            .unwrap()
            .is_none());
        let tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(tasks.get(alias_task.id).unwrap().list_id, old_default.id);
    }

    #[test]
    fn raw_list_states_and_all_live_quarantine_reasons_are_visible_to_election() {
        let file = NamedTempFile::new().unwrap();
        let list_id = Uuid::now_v7();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .put_record_state(live_record_state(
                list_id,
                "lists",
                Some("r1"),
                "m1",
                "{\"authenticated\":true}",
                400,
            ))
            .unwrap();
        transaction
            .put_quarantine(SyncQuarantineEntry {
                record_id: list_id,
                collection: "lists".to_string(),
                seq: 1,
                revision_hlc: "r2".to_string(),
                state: SyncOutboxState::Live {
                    mutation_hlc: "m2".to_string(),
                    blob: vec![1],
                },
                reason: "corrupt_envelope".to_string(),
                required_list_id: None,
                first_failed_at: 401,
                last_failed_at: 401,
                attempt_count: 1,
            })
            .unwrap();
        assert_eq!(transaction.list_record_states("lists").unwrap().len(), 1);
        assert!(transaction.has_live_quarantine("lists").unwrap());
        assert!(!transaction.has_live_quarantine("tasks").unwrap());
        transaction.commit().unwrap();

        let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
        assert_eq!(repository.list_record_states("lists").unwrap().len(), 1);
        assert!(repository.has_live_quarantine("lists").unwrap());
    }

    #[test]
    fn v16_profile_with_ambiguous_due_data_requires_recreation() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_raw_encrypted(file.path(), &KEY);
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY NOT NULL, list_id TEXT NOT NULL,
                    parent_task_id TEXT, title TEXT NOT NULL, note TEXT NOT NULL,
                    status TEXT NOT NULL, priority INTEGER NOT NULL, due_at INTEGER,
                    scheduled_at INTEGER, estimated_minutes INTEGER, sort_order TEXT NOT NULL,
                    completed_at INTEGER, closed_reason TEXT, deleted_at INTEGER,
                    assignee TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 INSERT INTO tasks (
                    id, list_id, title, note, status, priority, due_at,
                    sort_order, created_at, updated_at
                 ) VALUES ('task', 'list', 'title', '', 'todo', 0, 0, 'a0', 1, 1);
                 PRAGMA user_version = 16;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::MigrationFailed {
                target_version: 17,
                migration: "replace_task_due_semantics",
                ..
            })
        ));
    }
}
