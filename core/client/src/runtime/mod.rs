mod account;
mod application;
mod recurrence;
mod sync;

pub use application::{
    CalendarOccurrenceKind, CalendarOccurrenceView, CalendarRange, CreateTaskCommand, HomeTaskView,
    ReminderView, ReorderTaskCommand, SetTaskStatusCommand, TaskUndoKind, TaskUndoView,
    UpdateTaskCommand,
};
pub use recurrence::{
    CreateTaskSeriesFromTaskCommand, CreateTaskSeriesFromTemplateCommand, CreateTemplateCommand,
    ReplaceTaskBlueprintCommand, SaveTemplateCommand, SettlementSummary, UpdateTaskSeriesCommand,
    UpdateTemplateCommand,
};

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use taskveil_crypto::{derive_local_db_key, PlatformLocalKeyCapsuleStore};
use taskveil_storage::{
    open_encrypted, ListRepository, LocalCryptoRepository, SettingsRepository,
    SqliteListRepository, SqliteLocalCryptoRepository, SqliteReminderRepository,
    SqliteSettingsRepository, SqliteTaskRepository, SqliteTemplateSeriesRepository,
    SqliteTimerSessionRepository,
};
use taskveil_sync::SyncRunSummary;
use zeroize::Zeroizing;

use crate::{
    device_key_rotation::resolve_active_capsule, AccountSessionState, ClientError,
    LocalCryptoContext, LocalCryptoUnavailable, LocalMutationContext,
};

pub(super) const SYNC_SERVER_URL_SETTING_KEY: &str = "sync_server_url";
pub(super) const DEFAULT_SYNC_SERVER_URL: &str = "http://localhost:3000";
pub(super) const ACCOUNT_EMAIL_SETTING_KEY: &str = "account_email";
pub(super) const ACCOUNT_USER_ID_SETTING_KEY: &str = "account_user_id";
pub(super) const ACCOUNT_TENANT_ID_SETTING_KEY: &str = "account_tenant_id";
pub(super) const ACCOUNT_DEVICE_ID_SETTING_KEY: &str = "account_device_id";
pub(super) const ACCOUNT_SESSION_EXPIRES_AT_SETTING_KEY: &str = "account_session_expires_at";
pub(super) const ACCOUNT_ROOT_PUBLIC_SETTING_KEY: &str = "account_root_public";
pub(super) const ACCOUNT_MK_GENERATION_SETTING_KEY: &str = "account_mk_generation";
pub(super) const INITIAL_BACKFILL_CURSOR_NAME: &str = "initial_backfill";

#[derive(Debug, Clone)]
/// Configuration used to open one local encrypted profile.
///
/// This selects local persistence and bootstrap values only. Account identity
/// is stored separately as a durable `LocalProfileBinding`; credentials and
/// runtime session state are not configuration.
pub struct LocalProfileConfig {
    pub db_dir: PathBuf,
    pub default_inbox_name: String,
}

impl LocalProfileConfig {
    pub fn new(db_dir: impl Into<PathBuf>, default_inbox_name: impl Into<String>) -> Self {
        Self {
            db_dir: db_dir.into(),
            default_inbox_name: default_inbox_name.into(),
        }
    }
}

/// Frontend-neutral application facade for one local Taskveil profile.
///
/// Flutter, CLI, and MCP use this type for application operations. The type
/// owns runtime state and coordinates storage, crypto, account, and sync; it is
/// not a user-facing account profile model.
pub struct TaskveilClient {
    pub(crate) db_dir: PathBuf,
    pub(crate) db_path: PathBuf,
    db_key: Mutex<Zeroizing<[u8; 32]>>,
    account: Mutex<AccountRuntimeState>,
    sync: Mutex<SyncRuntimeState>,
    operation_busy: AtomicBool,
}

pub(super) struct AccountRuntimeState {
    pub(super) session: Option<AccountSessionState>,
    pub(super) session_restored: bool,
    pub(super) crypto: CryptoRuntimeState,
}

pub(super) enum CryptoRuntimeState {
    Unloaded,
    Anonymous,
    Ready(Box<LocalCryptoContext>),
    Unavailable(LocalCryptoUnavailable),
}

#[derive(Default)]
pub(super) struct SyncRuntimeState {
    pub(super) running: bool,
    pub(super) last_success_at: Option<i64>,
    pub(super) last_failure_at: Option<i64>,
    pub(super) last_error: Option<String>,
    pub(super) last_summary: SyncRunSummary,
}

#[allow(dead_code)] // Consumed by the CRUD migration phase of task-92.
pub(crate) enum LocalMutationState {
    Anonymous,
    Ready(LocalMutationContext),
    AccountBoundUnavailable,
}

impl TaskveilClient {
    pub fn open(config: LocalProfileConfig) -> Result<Self, ClientError> {
        std::fs::create_dir_all(&config.db_dir).map_err(ClientError::Io)?;
        let db_path = config.db_dir.join("taskveil.db");
        let mut capsule_store = PlatformLocalKeyCapsuleStore::new(&config.db_dir);
        let capsule = resolve_active_capsule(&mut capsule_store, &db_path)?;
        let db_key = Zeroizing::new(derive_local_db_key(capsule.device_key()));
        let connection = open_encrypted(&db_path, &db_key)?;
        SqliteListRepository::new(connection)
            .ensure_default_list(config.default_inbox_name, now_ms()?)?;

        Ok(Self {
            db_dir: config.db_dir,
            db_path,
            db_key: Mutex::new(db_key),
            account: Mutex::new(AccountRuntimeState {
                session: None,
                session_restored: false,
                crypto: CryptoRuntimeState::Unloaded,
            }),
            sync: Mutex::new(SyncRuntimeState::default()),
            operation_busy: AtomicBool::new(false),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Seeds the encrypted local profile for cross-crate performance tests.
    ///
    /// This method is unavailable in product builds.
    #[cfg(feature = "test-support")]
    pub fn seed_home_calendar_performance_fixture(&self) -> Result<usize, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
        taskveil_storage::test_support::seed_home_calendar_performance_fixture(&mut connection)
            .map_err(ClientError::from)
    }

    pub fn sync_server_url(&self) -> Result<String, ClientError> {
        let stored = self.setting(SYNC_SERVER_URL_SETTING_KEY)?;
        Ok(stored
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SYNC_SERVER_URL.to_string()))
    }

    pub fn set_sync_server_url(&self, server_url: String) -> Result<(), ClientError> {
        let _operation = self.begin_operation()?;
        let _session_lock = account::acquire_session_token_set_lock(&self.db_dir)?;
        let server_url = taskveil_sync::canonical_server_origin(&server_url)
            .map_err(|_| ClientError::AccountRequest)?;
        if account::stored_session_credential_issuer(&self.db_dir)?
            .is_some_and(|issuer| issuer != server_url)
        {
            return Err(ClientError::AccountRequest);
        }
        self.set_setting_value(SYNC_SERVER_URL_SETTING_KEY, &server_url)
    }

    #[allow(dead_code)] // Consumed by the CRUD migration phase of task-92.
    pub(crate) fn local_mutation_state(&self) -> Result<LocalMutationState, ClientError> {
        self.ensure_account_runtime_restored()?;
        let account = self.account_state()?;
        match &account.crypto {
            CryptoRuntimeState::Ready(crypto) => {
                Ok(LocalMutationState::Ready(crypto.mutation_context()))
            }
            CryptoRuntimeState::Anonymous => Ok(LocalMutationState::Anonymous),
            CryptoRuntimeState::Unavailable(reason) => {
                let _reason = reason;
                Ok(LocalMutationState::AccountBoundUnavailable)
            }
            CryptoRuntimeState::Unloaded => Ok(LocalMutationState::AccountBoundUnavailable),
        }
    }

    #[allow(dead_code)] // Consumed by the CRUD migration phase of task-92.
    pub(crate) fn preflight_sync_mutation(&self) -> Result<(), ClientError> {
        match self.local_mutation_state()? {
            LocalMutationState::Anonymous | LocalMutationState::Ready(_) => Ok(()),
            LocalMutationState::AccountBoundUnavailable => {
                Err(ClientError::AccountBoundUnavailable)
            }
        }
    }

    pub(super) fn db_key(&self) -> Zeroizing<[u8; 32]> {
        self.db_key
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn replace_db_key(&self, db_key: Zeroizing<[u8; 32]>) -> Result<(), ClientError> {
        *self.db_key.lock().map_err(|_| ClientError::RuntimeState)? = db_key;
        Ok(())
    }

    pub(super) fn account_state(&self) -> Result<MutexGuard<'_, AccountRuntimeState>, ClientError> {
        self.account.lock().map_err(|_| ClientError::RuntimeState)
    }

    pub(super) fn sync_state(&self) -> Result<MutexGuard<'_, SyncRuntimeState>, ClientError> {
        self.sync.lock().map_err(|_| ClientError::RuntimeState)
    }

    pub(super) fn operation_guard(&self) -> Result<OperationGuard<'_>, ClientError> {
        self.begin_operation()
    }

    pub(super) fn begin_operation(&self) -> Result<OperationGuard<'_>, ClientError> {
        self.operation_busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| ClientError::Busy)?;
        Ok(OperationGuard {
            running: &self.operation_busy,
        })
    }

    pub(super) fn with_task_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteTaskRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteTaskRepository::new(connection))
    }

    pub(super) fn with_list_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteListRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteListRepository::new(connection))
    }

    pub(super) fn with_settings_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteSettingsRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteSettingsRepository::new(connection))
    }

    #[allow(dead_code)] // Consumed by the reminder migration phase of task-92.
    pub(super) fn with_reminder_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteReminderRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteReminderRepository::new(connection))
    }

    pub(super) fn with_timer_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteTimerSessionRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteTimerSessionRepository::new(connection))
    }

    pub(super) fn with_recurrence_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteTemplateSeriesRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteTemplateSeriesRepository::new(connection))
    }

    pub(super) fn setting(&self, key: &str) -> Result<Option<String>, ClientError> {
        self.with_settings_repository(|repository| Ok(repository.get_setting(key)?))
    }

    pub(super) fn set_setting_value(&self, key: &str, value: &str) -> Result<(), ClientError> {
        let updated_at = now_ms()?;
        self.with_settings_repository(|repository| {
            repository.set_setting(key, value, updated_at)?;
            Ok(())
        })
    }

    pub(super) fn non_empty_setting(&self, key: &str) -> Result<Option<String>, ClientError> {
        Ok(self.setting(key)?.filter(|value| !value.trim().is_empty()))
    }

    pub(super) fn has_profile_binding(&self) -> Result<bool, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        Ok(SqliteLocalCryptoRepository::new(connection)
            .load_binding()?
            .is_some())
    }
}

pub(super) struct OperationGuard<'a> {
    running: &'a AtomicBool,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

pub(super) fn now_ms() -> Result<i64, ClientError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ClientError::RuntimeState)?;
    i64::try_from(duration.as_millis()).map_err(|_| ClientError::RuntimeState)
}

#[cfg(test)]
mod async_contract_tests {
    use super::TaskveilClient;

    fn assert_send<T: Send>(_: T) {}

    #[allow(dead_code)]
    fn network_api_futures_are_send(client: &TaskveilClient) {
        assert_send(client.account_register(
            "user@example.com".into(),
            "password".into(),
            None,
            None,
        ));
        assert_send(client.account_login("user@example.com".into(), "password".into(), None, None));
        assert_send(client.account_logout());
        assert_send(client.sync_now());
        assert_send(client.realtime_ticket());
    }
}
