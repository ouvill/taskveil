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

mod database;
mod error;
mod full_resync;
mod list_repository;
mod local_crypto_repository;
mod migrations;
mod models;
mod reminder_repository;
mod row;
mod settings_repository;
mod sync_state_repository;
mod task_repository;
mod template_series_repository;
mod timer_repository;
mod traits;
mod transaction;

use database::*;
use full_resync::*;
use list_repository::*;
use row::*;
use settings_repository::*;
use sync_state_repository::*;
use task_repository::*;
use template_series_repository::*;
use timer_repository::*;

pub use database::{open_encrypted, rekey_encrypted_database};
pub use error::StorageError;
pub use list_repository::SqliteListRepository;
pub use local_crypto_repository::SqliteLocalCryptoRepository;
pub use models::{
    CalendarOccurrence, CalendarOccurrenceKind, CalendarRange, CalendarRangeError, FullResyncPhase,
    FullResyncProgress, FullResyncStableCursor, FullResyncSweepSummary, HomeTask, ListAlias,
    LocalProfileBinding, LocalTenantRootKeyBundle, NewSyncOutboxEntry, Reminder, SyncCursor,
    SyncOutboxEntry, SyncOutboxState, SyncQuarantineEntry, SyncRecordSemanticState,
    SyncRecordState, TaskUndoEntry, TaskUndoOperation, MAX_REMINDERS_PER_TASK,
};
pub use reminder_repository::SqliteReminderRepository;
pub use settings_repository::SqliteSettingsRepository;
pub use sync_state_repository::SqliteSyncStateRepository;
pub use task_repository::SqliteTaskRepository;
pub use template_series_repository::SqliteTemplateSeriesRepository;
pub use timer_repository::SqliteTimerSessionRepository;
pub use traits::{
    ListRepository, LocalCryptoRepository, ReminderRepository, SettingsRepository,
    SyncStateRepository, TaskRepository, TemplateSeriesRepository, TimerSessionRepository,
};
pub use transaction::{OwnedSqliteWriteTx, SqliteWriteTx};

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

#[cfg(test)]
mod tests;
