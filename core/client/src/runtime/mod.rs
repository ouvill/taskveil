mod account;
mod application;
mod recurrence;
mod sync;

pub use account::{AccountRegistrationPending, AccountRegistrationPhase, AccountRegistrationState};
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
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use taskveil_crypto::{derive_local_db_key, PlatformLocalKeyCapsuleStore};
use taskveil_storage::{
    open_encrypted, InternalMetadataRepository, ListRepository, LocalCryptoRepository,
    SqliteAppSettingsRepository, SqliteInternalMetadataRepository, SqliteListRepository,
    SqliteLocalCryptoRepository, SqliteProfileCoordinationRepository, SqliteReminderRepository,
    SqliteTaskRepository, SqliteTemplateSeriesRepository, SqliteTimerSessionRepository,
};
use taskveil_sync::SyncRunSummary;
use zeroize::Zeroizing;

use crate::profile_coordination::{ProfileCoordinator, ProfileExclusiveGuard, ProfileSharedGuard};
use crate::{
    device_key_rotation::resolve_active_capsule, AccountSessionState, ClientError,
    LocalCryptoContext, LocalCryptoUnavailable, LocalMutationContext,
};

pub(super) const SYNC_SERVER_URL_METADATA_KEY: &str = "sync_server_url";
pub(super) const DEFAULT_SYNC_SERVER_URL: &str = "http://localhost:3000";
pub(super) const ACCOUNT_EMAIL_METADATA_KEY: &str = "account_email";
pub(super) const ACCOUNT_USER_ID_METADATA_KEY: &str = "account_user_id";
pub(super) const ACCOUNT_TENANT_ID_METADATA_KEY: &str = "account_tenant_id";
pub(super) const ACCOUNT_DEVICE_ID_METADATA_KEY: &str = "account_device_id";
pub(super) const ACCOUNT_SESSION_EXPIRES_AT_METADATA_KEY: &str = "account_session_expires_at";
pub(super) const ACCOUNT_ROOT_PUBLIC_METADATA_KEY: &str = "account_root_public";
pub(super) const ACCOUNT_MK_GENERATION_METADATA_KEY: &str = "account_mk_generation";
const TIMER_RUNTIME_METADATA_KEY: &str = "timer_runtime_v1";
const MAX_FRONTEND_SETTING_BYTES: usize = 16 * 1024;
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
    profile_coordinator: Arc<ProfileCoordinator>,
    db_key: Mutex<Zeroizing<[u8; 32]>>,
    account: Mutex<AccountRuntimeState>,
    sync: Mutex<SyncRuntimeState>,
    runtime_epoch: AtomicI64,
    capsule_generation: AtomicU64,
}

pub(super) struct AccountRuntimeState {
    pub(super) session: Option<AccountSessionState>,
    pub(super) session_restored: bool,
    pub(super) loaded_credential_generation: Option<String>,
    pub(super) crypto: CryptoRuntimeState,
}

pub(super) enum CryptoRuntimeState {
    Unloaded,
    Anonymous,
    Ready(Box<LocalCryptoContext>),
    Unavailable(LocalCryptoUnavailable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AccountReadiness {
    LoggedOut,
    Ready,
    CredentialUnavailable,
    AccountBoundUnavailable,
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
    #[cfg(test)]
    fn pinned_test_coordinator(db_dir: &Path, db_path: &Path) -> Arc<ProfileCoordinator> {
        let coordinator = ProfileCoordinator::for_profile(db_dir).unwrap();
        coordinator.pin_database(db_path).unwrap();
        coordinator
    }

    pub fn open(config: LocalProfileConfig) -> Result<Self, ClientError> {
        let profile_coordinator = ProfileCoordinator::for_profile(&config.db_dir)?;
        let _profile_lock = profile_coordinator.try_exclusive()?;
        let db_dir = profile_coordinator.canonical_root().to_path_buf();
        let db_path = db_dir.join("taskveil.db");
        let mut capsule_store = PlatformLocalKeyCapsuleStore::new(&db_dir);
        let capsule = resolve_active_capsule(&mut capsule_store, &db_path)?;
        let db_key = Zeroizing::new(derive_local_db_key(capsule.device_key()));
        let connection = open_encrypted(&db_path, &db_key)?;
        SqliteListRepository::new(connection)
            .ensure_default_list(config.default_inbox_name, now_ms()?)?;
        profile_coordinator.pin_database(&db_path)?;
        let mut coordination =
            SqliteProfileCoordinationRepository::new(open_encrypted(&db_path, &db_key)?);
        let runtime = coordination.publish_capsule_generation(
            i64::try_from(capsule.generation()).map_err(|_| ClientError::LocalKeyState)?,
            now_ms()?,
        )?;
        Ok(Self {
            db_dir,
            db_path,
            profile_coordinator,
            db_key: Mutex::new(db_key),
            account: Mutex::new(AccountRuntimeState {
                session: None,
                session_restored: false,
                loaded_credential_generation: None,
                crypto: CryptoRuntimeState::Unloaded,
            }),
            sync: Mutex::new(SyncRuntimeState::default()),
            runtime_epoch: AtomicI64::new(runtime.runtime_epoch),
            capsule_generation: AtomicU64::new(capsule.generation()),
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
        let _operation = self.begin_operation()?;
        let _session_lock = account::acquire_session_token_set_lock(&self.db_dir)?;
        self.sync_server_url_unlocked()
    }

    pub(super) fn sync_server_url_unlocked(&self) -> Result<String, ClientError> {
        let stored = self.non_empty_internal_metadata(SYNC_SERVER_URL_METADATA_KEY)?;
        let requested = stored.as_deref().unwrap_or(DEFAULT_SYNC_SERVER_URL);
        let canonical = taskveil_sync::canonical_server_origin(requested)
            .map_err(|_| ClientError::AccountRequest)?;
        if let Some(issuer) = account::stored_session_credential_issuer(&self.db_dir)? {
            let bound = taskveil_sync::canonical_server_origin(&issuer)
                .map_err(|_| ClientError::AccountRequest)?;
            if stored.is_some() && canonical != bound {
                return Err(ClientError::AccountRequest);
            }
            return Ok(bound);
        }
        Ok(canonical)
    }

    pub fn set_sync_server_url(&self, server_url: String) -> Result<(), ClientError> {
        let _operation = self.begin_exclusive_operation()?;
        let _session_lock = account::acquire_session_token_set_lock(&self.db_dir)?;
        let server_url = taskveil_sync::canonical_server_origin(&server_url)
            .map_err(|_| ClientError::AccountRequest)?;
        if account::stored_session_credential_issuer(&self.db_dir)?
            .is_some_and(|issuer| issuer != server_url)
        {
            return Err(ClientError::AccountRequest);
        }
        self.set_internal_metadata_value(SYNC_SERVER_URL_METADATA_KEY, &server_url)
    }

    #[allow(dead_code)] // Consumed by the CRUD migration phase of task-92.
    pub(crate) fn local_mutation_state(&self) -> Result<LocalMutationState, ClientError> {
        self.ensure_local_crypto_runtime_restored()?;
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

    pub(super) fn operation_guard(&self) -> Result<OperationGuard, ClientError> {
        self.begin_operation()
    }

    pub(super) fn begin_operation(&self) -> Result<OperationGuard, ClientError> {
        let coordinator = Arc::clone(&self.profile_coordinator);
        let shared = coordinator.try_shared()?;
        coordinator.validate_pinned_paths(&self.db_path)?;
        if self.has_pending_capsule_locked()? {
            drop(shared);
            {
                let _exclusive = coordinator.try_exclusive()?;
                coordinator.validate_pinned_paths(&self.db_path)?;
                self.recover_pending_capsule_locked()?;
            }
            let profile = ProfileOperationGuard::Shared(coordinator.try_shared()?);
            return self.begin_operation_with_profile(profile);
        }
        let profile = ProfileOperationGuard::Shared(shared);
        self.begin_operation_with_profile(profile)
    }

    pub(super) fn begin_exclusive_operation(&self) -> Result<OperationGuard, ClientError> {
        let exclusive = self.profile_coordinator.try_exclusive()?;
        self.profile_coordinator
            .validate_pinned_paths(&self.db_path)?;
        self.recover_pending_capsule_locked()?;
        let profile = ProfileOperationGuard::Exclusive(exclusive);
        self.begin_operation_with_profile(profile)
    }

    /// Captures the durable inputs needed to start a network workflow without
    /// retaining the profile guard across an `.await`.
    ///
    /// The profile guard remains the authority for capsule/runtime refresh,
    /// while the returned epoch is fenced by the sync lease at every remote
    /// and local commit boundary.
    pub(super) fn prepare_network_operation<T>(
        &self,
        prepare: impl FnOnce(&Self) -> Result<T, ClientError>,
    ) -> Result<(NetworkOperationContext, T), ClientError> {
        let _operation = self.begin_operation()?;
        let prepared = prepare(self)?;
        Ok((
            NetworkOperationContext {
                db_key: self.db_key(),
                runtime_epoch: self.loaded_runtime_epoch(),
            },
            prepared,
        ))
    }

    fn has_pending_capsule_locked(&self) -> Result<bool, ClientError> {
        use taskveil_crypto::{LocalKeyCapsuleSlot, LocalKeyCapsuleStore};

        PlatformLocalKeyCapsuleStore::new(&self.db_dir)
            .load(LocalKeyCapsuleSlot::Pending)
            .map(|capsule| capsule.is_some())
            .map_err(ClientError::KeyStore)
    }

    fn recover_pending_capsule_locked(&self) -> Result<(), ClientError> {
        if !self.has_pending_capsule_locked()? {
            return Ok(());
        }
        let mut capsule_store = PlatformLocalKeyCapsuleStore::new(&self.db_dir);
        let capsule = resolve_active_capsule(&mut capsule_store, &self.db_path)?;
        self.replace_db_key(Zeroizing::new(derive_local_db_key(capsule.device_key())))?;
        let runtime = SqliteProfileCoordinationRepository::new(open_encrypted(
            &self.db_path,
            &self.db_key(),
        )?)
        .publish_capsule_generation(
            i64::try_from(capsule.generation()).map_err(|_| ClientError::LocalKeyState)?,
            now_ms()?,
        )?;
        {
            let mut account = self.account_state()?;
            account.session = None;
            account.session_restored = false;
            account.loaded_credential_generation = None;
            account.crypto = CryptoRuntimeState::Unloaded;
        }
        self.publish_runtime_generation(runtime.runtime_epoch, capsule.generation());
        self.ensure_local_crypto_runtime_restored()
    }

    fn begin_operation_with_profile(
        &self,
        profile: ProfileOperationGuard,
    ) -> Result<OperationGuard, ClientError> {
        self.profile_coordinator
            .validate_pinned_paths(&self.db_path)?;
        self.refresh_profile_runtime_locked()?;
        Ok(OperationGuard { _profile: profile })
    }

    fn refresh_profile_runtime_locked(&self) -> Result<(), ClientError> {
        use taskveil_crypto::{LocalKeyCapsuleSlot, LocalKeyCapsuleStore};

        let capsule_store = PlatformLocalKeyCapsuleStore::new(&self.db_dir);
        let capsule = capsule_store
            .load(LocalKeyCapsuleSlot::Active)
            .map_err(ClientError::KeyStore)?;
        #[cfg(test)]
        if capsule.is_none() {
            let runtime = SqliteProfileCoordinationRepository::new(open_encrypted(
                &self.db_path,
                &self.db_key(),
            )?)
            .load_runtime()?;
            if runtime.runtime_epoch != self.runtime_epoch.load(Ordering::Acquire) {
                let mut account = self.account_state()?;
                account.session = None;
                account.session_restored = false;
                account.loaded_credential_generation = None;
                account.crypto = CryptoRuntimeState::Unloaded;
                self.runtime_epoch
                    .store(runtime.runtime_epoch, Ordering::Release);
                return Err(ClientError::LocalKeyState);
            }
            return Ok(());
        }
        let capsule = capsule.ok_or(ClientError::LocalKeyState)?;
        if capsule.generation() != self.capsule_generation.load(Ordering::Acquire) {
            self.replace_db_key(Zeroizing::new(derive_local_db_key(capsule.device_key())))?;
        }
        let runtime = SqliteProfileCoordinationRepository::new(open_encrypted(
            &self.db_path,
            &self.db_key(),
        )?)
        .load_runtime()?;
        if runtime.runtime_epoch != self.runtime_epoch.load(Ordering::Acquire) {
            {
                let mut account = self.account_state()?;
                account.session = None;
                account.session_restored = false;
                account.loaded_credential_generation = None;
                account.crypto = CryptoRuntimeState::Unloaded;
            }
            self.runtime_epoch
                .store(runtime.runtime_epoch, Ordering::Release);
            self.capsule_generation
                .store(capsule.generation(), Ordering::Release);
            self.ensure_local_crypto_runtime_restored()?;
        } else if self.capsule_generation.load(Ordering::Acquire) != capsule.generation() {
            self.capsule_generation
                .store(capsule.generation(), Ordering::Release);
        }
        Ok(())
    }

    pub(super) fn loaded_runtime_epoch(&self) -> i64 {
        self.runtime_epoch.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn publish_runtime_epoch(&self, runtime_epoch: i64) {
        self.runtime_epoch.store(runtime_epoch, Ordering::Release);
    }

    pub(super) fn publish_runtime_epoch_if_current(
        &self,
        expected: i64,
        runtime_epoch: i64,
    ) -> Result<(), ClientError> {
        self.runtime_epoch
            .compare_exchange(expected, runtime_epoch, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| ClientError::LeaseLost)
    }

    pub(super) fn retry_runtime_epoch_once<T>(
        &self,
        mut operation: impl FnMut() -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        match operation() {
            Err(ClientError::Storage(
                taskveil_storage::StorageError::ProfileRuntimeEpochChanged { .. },
            )) => {
                // Epoch assertions occur immediately after BEGIN IMMEDIATE and
                // before the command writes anything, so replaying the full
                // command once is exactly-once. A second mismatch is surfaced
                // fail-closed instead of looping.
                self.refresh_profile_runtime_locked()?;
                operation()
            }
            result => result,
        }
    }

    pub(super) fn publish_runtime_generation(&self, runtime_epoch: i64, capsule_generation: u64) {
        self.runtime_epoch.store(runtime_epoch, Ordering::Release);
        self.capsule_generation
            .store(capsule_generation, Ordering::Release);
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

    pub(super) fn with_app_settings_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteAppSettingsRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteAppSettingsRepository::new(connection))
    }

    pub(super) fn with_internal_metadata_repository<T>(
        &self,
        f: impl FnOnce(&mut SqliteInternalMetadataRepository) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        f(&mut SqliteInternalMetadataRepository::new(connection))
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

    pub(super) fn internal_metadata(&self, key: &str) -> Result<Option<String>, ClientError> {
        self.with_internal_metadata_repository(|repository| {
            Ok(repository.get_internal_metadata(key)?)
        })
    }

    pub(super) fn set_internal_metadata_value(
        &self,
        key: &str,
        value: &str,
    ) -> Result<(), ClientError> {
        let updated_at = now_ms()?;
        self.with_internal_metadata_repository(|repository| {
            repository.set_internal_metadata(key, value, updated_at)?;
            Ok(())
        })
    }

    pub(super) fn non_empty_internal_metadata(
        &self,
        key: &str,
    ) -> Result<Option<String>, ClientError> {
        Ok(self
            .internal_metadata(key)?
            .filter(|value| !value.trim().is_empty()))
    }

    pub(super) fn has_profile_binding(&self) -> Result<bool, ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        Ok(SqliteLocalCryptoRepository::new(connection)
            .load_binding()?
            .is_some())
    }
}

pub(super) struct OperationGuard {
    _profile: ProfileOperationGuard,
}

pub(super) struct NetworkOperationContext {
    pub(super) db_key: Zeroizing<[u8; 32]>,
    pub(super) runtime_epoch: i64,
}

#[allow(dead_code)] // Variant payloads own the RAII locks until operation drop.
enum ProfileOperationGuard {
    Shared(ProfileSharedGuard),
    Exclusive(ProfileExclusiveGuard),
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
        assert_send(client.account_registration_begin("user@example.com".into()));
        assert_send(client.account_registration_resend());
        assert_send(client.account_registration_verify_otp("12345678".into()));
        assert_send(client.account_registration_complete("password".into(), None));
        assert_send(client.account_login("user@example.com".into(), "password".into(), None, None));
        assert_send(client.account_logout());
        assert_send(client.sync_now());
        assert_send(client.realtime_ticket());
    }
}
