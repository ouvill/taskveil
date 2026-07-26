//! Frontend-independent Taskveil client application services.
//!
//! This crate is the shared entry point for Flutter, CLI, and MCP. It owns
//! transaction boundaries that span domain rows and local sync bookkeeping.
//!
//! Frontends must enter through [`TaskveilClient`]. Low-level storage and sync
//! orchestration types are deliberately not part of the normal public API:
//!
//! ```no_run
//! use taskveil_client::{LocalProfileConfig, TaskveilClient};
//!
//! let client = TaskveilClient::open(LocalProfileConfig::new("/tmp/taskveil", "Inbox"))?;
//! # Ok::<(), taskveil_client::ClientError>(())
//! ```
//!
//! The superseded pre-release names are intentionally unavailable:
//!
//! ```compile_fail
//! use taskveil_client::{ClientProfile, ProfileConfig};
//! ```
//!
//! ```compile_fail
//! use taskveil_client::SqliteMutationService;
//! ```
//!
//! ```compile_fail
//! use taskveil_client::SqliteSyncStore;
//! ```

mod crud_service;
mod device_key_rotation;
mod local_crypto;
mod model;
mod mutation_service;
mod profile_coordination;
mod runtime;
mod sqlite_sync_store;

pub(crate) use crud_service::{CreateTaskInput, ReorderTaskInput, SetTaskStatusInput};
#[cfg(test)]
pub(crate) use local_crypto::persist_local_crypto_context;
pub(crate) use local_crypto::{
    load_local_crypto_context, LocalCryptoAvailability, LocalCryptoContext, LocalCryptoIdentity,
    LocalCryptoUnavailable,
};
pub use model::{
    AccountAuthResult, AccountSessionState, BillingState, FrontendSettingKey,
    OrganizationSafetyState, RealtimeTicket, SyncStatus,
};
pub use mutation_service::ClientError;
pub(crate) use mutation_service::{LocalMutationContext, SqliteMutationService, UpdateTaskInput};
pub use runtime::{
    AccountRegistrationPending, AccountRegistrationPhase, AccountRegistrationState,
    CalendarOccurrenceKind, CalendarOccurrenceView, CalendarRange, CreateTaskCommand,
    CreateTaskSeriesFromTaskCommand, CreateTaskSeriesFromTemplateCommand, CreateTemplateCommand,
    HomeTaskView, LocalProfileConfig, ReminderView, ReorderTaskCommand,
    ReplaceTaskBlueprintCommand, SaveTemplateCommand, SetTaskStatusCommand, SettlementSummary,
    TaskUndoKind, TaskUndoView, TaskveilClient, UpdateTaskCommand, UpdateTaskSeriesCommand,
    UpdateTemplateCommand,
};
pub(crate) use sqlite_sync_store::SqliteSyncStore;
pub use taskveil_domain::TASK_BLUEPRINT_SCHEMA_REVISION;
pub use taskveil_domain::{
    pomodoro_target_reached_at, ActiveTimerSession, CivilDate, CompletedTimerSession,
    DueValueError, IanaTimeZone, List, SeriesCursor, Streak, Task, TaskBlueprint,
    TaskBlueprintNode, TaskContent, TaskDue, TaskSeries, TaskStatus, TaskTemplate, TimerFinishKind,
    TimerMode, TimerPhase, TimerRunState, UtcInstant, Uuid,
};

pub use chrono;

/// Unstable low-level primitives for cross-crate integration tests.
///
/// Product frontends must not enable this feature. These exports may change as
/// the internal client implementation evolves; [`TaskveilClient`] is the only
/// supported application entry point.
#[cfg(feature = "test-support")]
pub mod test_support {
    pub use crate::crud_service::{CreateTaskInput, ReorderTaskInput, SetTaskStatusInput};
    pub use crate::local_crypto::{
        load_local_crypto_context, persist_account_crypto_context, persist_local_crypto_context,
        LocalCryptoAvailability, LocalCryptoContext, LocalCryptoIdentity, LocalCryptoUnavailable,
    };
    pub use crate::mutation_service::{
        LocalMutationContext, SqliteMutationService, UpdateTaskInput,
    };
    pub use crate::sqlite_sync_store::{SqliteSyncStore, SqliteSyncWriteTx};
}
