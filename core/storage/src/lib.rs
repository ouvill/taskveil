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
mod profile_coordination;
mod reminder_notification_repository;
mod reminder_repository;
mod row;
mod settings_repository;
mod sync_state_repository;
mod task_repository;
mod template_series_repository;
#[cfg(feature = "test-support")]
pub mod test_support;
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
    LocalProfileBinding, LocalTenantRootKeyBundle, NewSyncOutboxEntry, Reminder,
    ReminderNotificationAction, ReminderNotificationCommand, SyncCursor, SyncOutboxEntry,
    SyncOutboxState, SyncQuarantineEntry, SyncRecordSemanticState, SyncRecordState, TaskUndoEntry,
    TaskUndoOperation, MAX_REMINDERS_PER_TASK,
};
pub use profile_coordination::{
    ProfileRuntimeState, SqliteProfileCoordinationRepository, SyncLease,
};
pub use reminder_notification_repository::SqliteReminderNotificationRepository;
pub use reminder_repository::SqliteReminderRepository;
pub use settings_repository::{
    AppSettingKey, SqliteAppSettingsRepository, SqliteInternalMetadataRepository,
};
pub use sync_state_repository::SqliteSyncStateRepository;
pub use task_repository::SqliteTaskRepository;
pub use template_series_repository::SqliteTemplateSeriesRepository;
pub use timer_repository::SqliteTimerSessionRepository;
pub use traits::{
    AppSettingsRepository, InternalMetadataRepository, ListRepository, LocalCryptoRepository,
    ReminderNotificationRepository, ReminderRepository, SyncStateRepository, TaskRepository,
    TemplateSeriesRepository, TimerSessionRepository,
};
pub use transaction::{OwnedSqliteWriteTx, SqliteWriteTx};

#[cfg(test)]
use migrations::ensure_schema_at_version;
use migrations::{ensure_schema, MIGRATIONS};

pub const LATEST_MIGRATION_VERSION: i32 = 5;
const LOCAL_DB_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests;
