use std::{future::Future, pin::Pin};

use taskveil_storage::{TaskRepository, TemplateSeriesRepository, TimerSessionRepository};
use taskveil_sync::{
    account::{AccountClient, AccountClientError},
    ActiveSyncContext, LocalSyncAtomicStore, LocalSyncKeys, LocalSyncStore,
    LocalSyncWriteTransaction, SyncKeyRefresher, SyncRunSummary,
};

use super::{
    now_ms, AccountReadiness, CryptoRuntimeState, SyncRuntimeState, TaskveilClient,
    INITIAL_BACKFILL_CURSOR_NAME,
};
use crate::{ClientError, RealtimeTicket, SqliteSyncStore, SyncStatus};

const SYNC_LEASE_TTL_MS: i64 = 5 * 60 * 1_000;

enum SyncReadiness {
    LoggedOut,
    Ready(Box<ActiveSyncContext>),
    CredentialUnavailable,
    AccountBoundUnavailable,
}

impl TaskveilClient {
    pub fn sync_status(&self) -> Result<SyncStatus, ClientError> {
        let _operation = self.begin_operation()?;
        let logged_in = match self.sync_readiness()? {
            SyncReadiness::LoggedOut => false,
            SyncReadiness::Ready(_) => true,
            SyncReadiness::CredentialUnavailable => return Err(ClientError::CredentialUnavailable),
            SyncReadiness::AccountBoundUnavailable => {
                return Err(ClientError::AccountBoundUnavailable)
            }
        };
        let state = self.sync_state()?;
        Ok(sync_status(logged_in, &state))
    }

    pub async fn sync_now(&self) -> Result<SyncStatus, ClientError> {
        let operation = self.begin_operation()?;
        let context = match self.sync_readiness()? {
            SyncReadiness::LoggedOut => {
                let state = self.sync_state()?;
                return Ok(sync_status(false, &state));
            }
            SyncReadiness::Ready(context) => *context,
            SyncReadiness::CredentialUnavailable => return Err(ClientError::CredentialUnavailable),
            SyncReadiness::AccountBoundUnavailable => {
                return Err(ClientError::AccountBoundUnavailable)
            }
        };
        {
            let mut state = self.sync_state()?;
            if state.running {
                return Ok(sync_status(true, &state));
            }
            state.running = true;
            state.last_error = None;
        }
        let running = SyncRunningGuard { client: self };
        let result = self.run_sync_now(context).await;
        let timestamp = now_ms()?;
        let mut state = self.sync_state()?;
        let (status, surfaced_error) = finish_sync_run(true, &mut state, result, timestamp);
        drop(state);
        drop(running);
        drop(operation);
        match surfaced_error {
            Some(error) => Err(error),
            None => Ok(status),
        }
    }

    /// Fetches a short-lived foreground realtime ticket without exposing the
    /// session token or tenant/device identifiers to the frontend.
    pub async fn realtime_ticket(&self) -> Result<RealtimeTicket, ClientError> {
        let _operation = self.begin_operation()?;
        let context = match self.sync_readiness()? {
            SyncReadiness::Ready(context) => context,
            SyncReadiness::LoggedOut | SyncReadiness::CredentialUnavailable => {
                return Err(ClientError::CredentialUnavailable)
            }
            SyncReadiness::AccountBoundUnavailable => {
                return Err(ClientError::AccountBoundUnavailable)
            }
        };
        let token = self.access_token(false).await?;
        let client =
            AccountClient::new(&context.server_url).map_err(|_| ClientError::AccountRequest)?;
        let response = match client.realtime_ticket(context.tenant_id, &token).await {
            Err(AccountClientError::Server(401)) => {
                let token = self.access_token(true).await?;
                client.realtime_ticket(context.tenant_id, &token).await
            }
            result => result,
        }
        .map_err(|error| match error {
            AccountClientError::EntitlementRequired => ClientError::EntitlementRequired,
            _ => ClientError::AccountRequest,
        })?;
        Ok(RealtimeTicket {
            websocket_url: response.websocket_url,
            ticket: response.ticket,
            expires_at: response.expires_at,
        })
    }

    async fn run_sync_now(
        &self,
        mut context: ActiveSyncContext,
    ) -> Result<SyncRunSummary, ClientError> {
        let mut store = SqliteSyncStore::new_secret(self.db_path.clone(), self.db_key());
        let lease_owner = taskveil_domain::Uuid::now_v7().to_string();
        store
            .acquire_sync_lease(&lease_owner, self.loaded_runtime_epoch(), SYNC_LEASE_TTL_MS)
            .map_err(|error| map_sync_run_error(map_sync_lease_error(error)))?;
        let result = async {
            let token = self.access_token_for_sync(false, &mut store).await?;
            context.server_url = token.issuer.clone();
            context.session_token = taskveil_sync::SecretString::new(token.to_string());
            match self.run_sync_attempt(&context, &mut store).await {
                Err(error) if error == "unauthorized" => {
                    let token = self.access_token_for_sync(true, &mut store).await?;
                    context.server_url = token.issuer.clone();
                    context.session_token = taskveil_sync::SecretString::new(token.to_string());
                    self.run_sync_attempt(&context, &mut store)
                        .await
                        .map_err(map_sync_run_error)
                }
                result => result.map_err(map_sync_run_error),
            }
        }
        .await;
        let release = store
            .release_sync_lease()
            .map_err(|error| map_sync_run_error(map_sync_lease_error(error)));
        match (result, release) {
            (Ok(summary), Ok(())) => Ok(summary),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn run_sync_attempt(
        &self,
        context: &ActiveSyncContext,
        store: &mut SqliteSyncStore,
    ) -> Result<SyncRunSummary, String> {
        let mut clock = || now_ms().map_err(|error| error.to_string());
        let mut key_refresher = ProductionKeyRefresher {
            client: self,
            lease: store.active_lease().map_err(map_sync_lease_error)?,
        };
        let mut pre_push = |store: &mut SqliteSyncStore| {
            self.run_initial_backfill_if_needed(context, store)
                .map_err(|error| map_sync_lease_error_from_client(&error))
        };
        async {
            let mut summary = taskveil_sync::run_sync_now_with_key_refresh_and_pre_push(
                context.clone(),
                &mut *store,
                &mut clock,
                &mut key_refresher,
                &mut pre_push,
            )
            .await?;
            loop {
                let timestamp = now_ms().map_err(|_| "sync failed".to_string())?;
                let settlement = self
                    .settle_after_sync_pull(store, timestamp)
                    .map_err(|error| map_sync_lease_error_from_client(&error))?;
                if !settlement.outbox_changed {
                    break;
                }
                let follow_up = taskveil_sync::run_sync_now_with_key_refresh_and_pre_push(
                    context.clone(),
                    &mut *store,
                    &mut clock,
                    &mut key_refresher,
                    &mut pre_push,
                )
                .await?;
                add_sync_summary(&mut summary, &follow_up);
                if !settlement.has_more {
                    break;
                }
            }
            Ok::<_, String>(summary)
        }
        .await
    }

    fn run_initial_backfill_if_needed(
        &self,
        context: &ActiveSyncContext,
        store: &mut SqliteSyncStore,
    ) -> Result<(), ClientError> {
        if store
            .get_cursor_seq(INITIAL_BACKFILL_CURSOR_NAME)
            .map_err(map_sync_run_error)?
            .is_some()
        {
            return Ok(());
        }

        let lists = self.local_lists_including_archived()?;
        let templates =
            self.with_recurrence_repository(|repository| Ok(repository.list_templates()?))?;
        let schedules =
            self.with_recurrence_repository(|repository| Ok(repository.list_series()?))?;
        let tasks = self.with_task_repository(|repository| Ok(repository.list_all_for_sync()?))?;
        let timer_sessions =
            self.with_timer_repository(|repository| Ok(repository.list_completed()?))?;
        let mut clock = || now_ms().map_err(|error| error.to_string());
        let mut transaction = store
            .begin_write_transaction()
            .map_err(map_sync_run_error)?;
        taskveil_sync::enqueue_backfill(
            &mut transaction,
            &context.keys,
            &context.device_id,
            taskveil_sync::BackfillRecords {
                lists: &lists,
                templates: &templates,
                task_series: &schedules,
                tasks: &tasks,
                timer_sessions: &timer_sessions,
            },
            &mut clock,
        )
        .map_err(map_sync_run_error)?;
        transaction
            .set_cursor(INITIAL_BACKFILL_CURSOR_NAME, 1, now_ms()?)
            .map_err(map_sync_run_error)?;
        transaction.commit().map_err(map_sync_run_error)
    }

    fn sync_readiness(&self) -> Result<SyncReadiness, ClientError> {
        match self.resolve_account_readiness()? {
            AccountReadiness::LoggedOut => return Ok(SyncReadiness::LoggedOut),
            AccountReadiness::CredentialUnavailable => {
                return Ok(SyncReadiness::CredentialUnavailable)
            }
            AccountReadiness::AccountBoundUnavailable => {
                return Ok(SyncReadiness::AccountBoundUnavailable)
            }
            AccountReadiness::Ready => {}
        }
        let account = self.account_state()?;
        let session = account
            .session
            .clone()
            .filter(|session| session.logged_in)
            .ok_or(ClientError::CredentialUnavailable)?;
        let CryptoRuntimeState::Ready(crypto) = &account.crypto else {
            return Ok(SyncReadiness::AccountBoundUnavailable);
        };
        let session_user_id = session
            .user_id
            .as_deref()
            .ok_or(ClientError::IncompleteAccountState)?
            .parse::<taskveil_domain::Uuid>()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        let session_tenant_id = session
            .tenant_id
            .as_deref()
            .ok_or(ClientError::IncompleteAccountState)?
            .parse::<taskveil_domain::Uuid>()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        let session_device_id = session
            .device_id
            .as_deref()
            .ok_or(ClientError::IncompleteAccountState)?
            .parse::<taskveil_domain::Uuid>()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        if session_user_id != crypto.user_id()
            || session_tenant_id != crypto.tenant_id()
            || session_device_id != crypto.device_id()
        {
            return Err(ClientError::ProfileIdentityMismatch);
        }
        let tenant_id = crypto.tenant_id();
        let device_id = crypto.device_id().to_string();
        let keys = crypto.sync_keys().clone();
        let manifest_auth_key =
            taskveil_sync::derive_personal_manifest_auth_key(crypto.master_key())
                .map_err(|_| ClientError::AccountBoundUnavailable)?;
        drop(account);
        let token = self
            .current_access_token()?
            .ok_or(ClientError::CredentialUnavailable)?;
        if token.is_empty() {
            return Ok(SyncReadiness::CredentialUnavailable);
        }
        Ok(SyncReadiness::Ready(Box::new(ActiveSyncContext {
            server_url: token.issuer.clone(),
            tenant_id,
            device_id,
            session_token: taskveil_sync::SecretString::new(token.to_string()),
            keys,
            manifest_auth_key,
        })))
    }
}

fn map_sync_lease_error(error: taskveil_storage::StorageError) -> String {
    if error.is_database_busy() {
        return "database busy".to_string();
    }
    match error {
        taskveil_storage::StorageError::SyncLeaseBusy => "sync lease busy".to_string(),
        taskveil_storage::StorageError::SyncLeaseLost
        | taskveil_storage::StorageError::ProfileRuntimeEpochChanged { .. } => {
            "sync lease lost".to_string()
        }
        _ => "sync failed".to_string(),
    }
}

fn add_sync_summary(target: &mut SyncRunSummary, value: &SyncRunSummary) {
    target.pushed_count += value.pushed_count;
    target.push_acked_count += value.push_acked_count;
    target.push_superseded_count += value.push_superseded_count;
    target.push_conflict_count += value.push_conflict_count;
    target.pulled_count += value.pulled_count;
    target.applied_count += value.applied_count;
    target.deleted_count += value.deleted_count;
    target.decrypt_failed_count += value.decrypt_failed_count;
    target.repush_count += value.repush_count;
    target.missing_key_quarantined_count += value.missing_key_quarantined_count;
    target.corruption_quarantined_count += value.corruption_quarantined_count;
    target.resolved_quarantine_count += value.resolved_quarantine_count;
}

pub(super) fn map_sync_run_error(error: String) -> ClientError {
    match error.as_str() {
        "upgrade required" => ClientError::UpgradeRequired,
        "entitlement required" => ClientError::EntitlementRequired,
        "clock skew retryable" => ClientError::ClockSkewRetryable,
        "credential unavailable" => ClientError::CredentialUnavailable,
        "account-bound unavailable" => ClientError::AccountBoundUnavailable,
        "profile busy" => ClientError::ProfileBusy,
        "credential busy" => ClientError::Busy,
        "sync lease busy" => ClientError::SyncLeaseBusy,
        "sync lease lost" => ClientError::LeaseLost,
        "database busy" => ClientError::DatabaseBusy,
        _ => ClientError::SyncRun,
    }
}

fn map_sync_lease_error_from_client(error: &ClientError) -> String {
    match error {
        ClientError::UpgradeRequired => "upgrade required".to_string(),
        ClientError::EntitlementRequired => "entitlement required".to_string(),
        ClientError::ClockSkewRetryable => "clock skew retryable".to_string(),
        ClientError::CredentialUnavailable => "credential unavailable".to_string(),
        ClientError::AccountBoundUnavailable => "account-bound unavailable".to_string(),
        ClientError::ProfileBusy => "profile busy".to_string(),
        ClientError::Busy => "credential busy".to_string(),
        ClientError::SyncLeaseBusy => "sync lease busy".to_string(),
        ClientError::LeaseLost
        | ClientError::Storage(taskveil_storage::StorageError::SyncLeaseLost)
        | ClientError::Storage(taskveil_storage::StorageError::ProfileRuntimeEpochChanged {
            ..
        }) => "sync lease lost".to_string(),
        ClientError::DatabaseBusy => "database busy".to_string(),
        ClientError::Storage(error) if error.is_database_busy() => "database busy".to_string(),
        _ => "sync failed".to_string(),
    }
}

struct SyncRunningGuard<'a> {
    client: &'a TaskveilClient,
}

impl Drop for SyncRunningGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.client.sync_state() {
            state.running = false;
        }
    }
}

struct ProductionKeyRefresher<'a> {
    client: &'a TaskveilClient,
    lease: taskveil_storage::SyncLease,
}

impl SyncKeyRefresher for ProductionKeyRefresher<'_> {
    fn refresh<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<LocalSyncKeys, String>> + Send + 'a>> {
        Box::pin(async move {
            let loaded_epoch = self.client.loaded_runtime_epoch();
            let keys = self
                .client
                .refresh_tenant_keys_for_sync(self.lease.clone())
                .await
                .map_err(|error| map_sync_lease_error_from_client(&error))?;
            let durable_epoch = taskveil_storage::SqliteProfileCoordinationRepository::new(
                taskveil_storage::open_encrypted(&self.client.db_path, &self.client.db_key())
                    .map_err(map_sync_lease_error)?,
            )
            .load_runtime()
            .map_err(map_sync_lease_error)?
            .runtime_epoch;
            if durable_epoch != loaded_epoch {
                return Err("sync lease lost".to_string());
            }
            Ok(keys)
        })
    }
}

fn sync_status(logged_in: bool, state: &SyncRuntimeState) -> SyncStatus {
    SyncStatus {
        logged_in,
        running: state.running,
        last_success_at: state.last_success_at,
        last_failure_at: state.last_failure_at,
        last_error: state.last_error.clone(),
        pushed_count: state.last_summary.pushed_count,
        push_acked_count: state.last_summary.push_acked_count,
        push_superseded_count: state.last_summary.push_superseded_count,
        pulled_count: state.last_summary.pulled_count,
        applied_count: state.last_summary.applied_count,
        deleted_count: state.last_summary.deleted_count,
        decrypt_failed_count: state.last_summary.decrypt_failed_count,
        repush_count: state.last_summary.repush_count,
        missing_key_quarantined_count: state.last_summary.missing_key_quarantined_count,
        corruption_quarantined_count: state.last_summary.corruption_quarantined_count,
        resolved_quarantine_count: state.last_summary.resolved_quarantine_count,
        upgrade_required: state.last_error.as_deref() == Some("upgrade required"),
    }
}

fn finish_sync_run(
    logged_in: bool,
    state: &mut SyncRuntimeState,
    result: Result<SyncRunSummary, ClientError>,
    timestamp: i64,
) -> (SyncStatus, Option<ClientError>) {
    state.running = false;
    let mut surfaced_error = None;
    match result {
        Ok(summary) => {
            state.last_success_at = Some(timestamp);
            state.last_error = None;
            state.last_summary = summary;
        }
        Err(ClientError::UpgradeRequired) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("upgrade required".to_string());
        }
        Err(ClientError::EntitlementRequired) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("entitlement required".to_string());
            surfaced_error = Some(ClientError::EntitlementRequired);
        }
        Err(ClientError::SyncLeaseBusy) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("sync lease busy".to_string());
            surfaced_error = Some(ClientError::SyncLeaseBusy);
        }
        Err(ClientError::LeaseLost) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("sync lease lost".to_string());
            surfaced_error = Some(ClientError::LeaseLost);
        }
        Err(ClientError::DatabaseBusy) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("database busy".to_string());
            surfaced_error = Some(ClientError::DatabaseBusy);
        }
        Err(error @ ClientError::ClockSkewRetryable) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("clock skew retryable".to_string());
            surfaced_error = Some(error);
        }
        Err(error @ ClientError::CredentialUnavailable) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("credential unavailable".to_string());
            surfaced_error = Some(error);
        }
        Err(error @ ClientError::AccountBoundUnavailable) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("account-bound unavailable".to_string());
            surfaced_error = Some(error);
        }
        Err(error @ ClientError::ProfileBusy) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("profile busy".to_string());
            surfaced_error = Some(error);
        }
        Err(error @ ClientError::Busy) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("credential busy".to_string());
            surfaced_error = Some(error);
        }
        Err(error) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("sync failed".to_string());
            surfaced_error = Some(error);
        }
    }
    (sync_status(logged_in, state), surfaced_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidated_session_is_reported_logged_out_after_failed_sync() {
        let mut state = SyncRuntimeState {
            running: true,
            ..SyncRuntimeState::default()
        };

        let (status, surfaced_error) =
            finish_sync_run(false, &mut state, Err(ClientError::AccountRequest), 42);

        assert!(!status.logged_in);
        assert!(!status.running);
        assert_eq!(status.last_failure_at, Some(42));
        assert_eq!(status.last_error.as_deref(), Some("sync failed"));
        assert!(matches!(surfaced_error, Some(ClientError::AccountRequest)));
    }

    #[test]
    fn coordination_failures_remain_typed_at_the_sync_api_boundary() {
        assert_eq!(
            map_sync_lease_error_from_client(&ClientError::LeaseLost),
            "sync lease lost"
        );
        assert_eq!(
            map_sync_lease_error_from_client(&ClientError::DatabaseBusy),
            "database busy"
        );
        assert_eq!(
            map_sync_lease_error_from_client(&ClientError::SyncLeaseBusy),
            "sync lease busy"
        );
        assert_eq!(
            map_sync_lease_error_from_client(&ClientError::ClockSkewRetryable),
            "clock skew retryable"
        );
        for expected in [
            ClientError::ClockSkewRetryable,
            ClientError::CredentialUnavailable,
            ClientError::AccountBoundUnavailable,
            ClientError::ProfileBusy,
            ClientError::Busy,
            ClientError::SyncLeaseBusy,
            ClientError::LeaseLost,
            ClientError::DatabaseBusy,
        ] {
            let mut state = SyncRuntimeState {
                running: true,
                ..SyncRuntimeState::default()
            };
            let (_, surfaced_error) = finish_sync_run(true, &mut state, Err(expected), 42);
            assert!(matches!(
                surfaced_error,
                Some(
                    ClientError::ClockSkewRetryable
                        | ClientError::CredentialUnavailable
                        | ClientError::AccountBoundUnavailable
                        | ClientError::ProfileBusy
                        | ClientError::Busy
                        | ClientError::SyncLeaseBusy
                        | ClientError::LeaseLost
                        | ClientError::DatabaseBusy
                )
            ));
        }
    }
}
