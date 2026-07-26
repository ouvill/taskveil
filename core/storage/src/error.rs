use crate::*;

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
    #[error("unsupported database migration version: found {found}, latest supported {latest}")]
    UnsupportedMigrationVersion { found: i32, latest: i32 },
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
    #[error("local profile runtime epoch changed: expected {expected}, found {actual}")]
    ProfileRuntimeEpochChanged { expected: i64, actual: i64 },
    #[error("local profile runtime epoch or fencing token overflowed")]
    ProfileCoordinationOverflow,
    #[error("local profile coordination clock moved backwards")]
    ProfileCoordinationClockRollback,
    #[error("another sync run owns the local profile lease")]
    SyncLeaseBusy,
    #[error("the local profile sync lease was lost")]
    SyncLeaseLost,
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

impl StorageError {
    pub fn is_database_busy(&self) -> bool {
        let error = match self {
            Self::Sqlite(error) => error,
            Self::MigrationFailed { source, .. } => source,
            _ => return false,
        };
        matches!(
            error,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
                    ..
                },
                _
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_busy_and_locked_errors_are_classified_for_client_mapping() {
        for code in [rusqlite::ffi::SQLITE_BUSY, rusqlite::ffi::SQLITE_LOCKED] {
            let error = StorageError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ));
            assert!(error.is_database_busy());
        }
        assert!(!StorageError::NotFound(Uuid::now_v7()).is_database_busy());
    }
}
