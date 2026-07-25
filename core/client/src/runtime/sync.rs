use std::{future::Future, pin::Pin};

use taskveil_storage::{TaskRepository, TemplateSeriesRepository, TimerSessionRepository};
use taskveil_sync::{
    account::{AccountClient, AccountClientError},
    ActiveSyncContext, LocalSyncAtomicStore, LocalSyncKeys, LocalSyncStore,
    LocalSyncWriteTransaction, SyncKeyRefresher, SyncRunSummary,
};

use super::{
    now_ms, CryptoRuntimeState, SyncRuntimeState, TaskveilClient, INITIAL_BACKFILL_CURSOR_NAME,
};
use crate::{ClientError, RealtimeTicket, SqliteSyncStore, SyncStatus};

impl TaskveilClient {
    pub fn sync_status(&self) -> Result<SyncStatus, ClientError> {
        let logged_in = self.has_active_sync_context();
        let state = self.sync_state()?;
        Ok(sync_status(logged_in, &state))
    }

    pub async fn sync_now(&self) -> Result<SyncStatus, ClientError> {
        if !self.has_active_sync_context() {
            let state = self.sync_state()?;
            return Ok(sync_status(false, &state));
        }
        {
            let mut state = self.sync_state()?;
            if state.running {
                return Ok(sync_status(true, &state));
            }
            state.running = true;
            state.last_error = None;
        }
        let running = SyncRunningGuard { client: self };

        let operation = match self.begin_operation() {
            Ok(operation) => operation,
            Err(error) => return Err(error),
        };
        let result = self.run_sync_now().await;
        drop(operation);
        let timestamp = now_ms()?;
        let logged_in = self.has_active_sync_context();
        let mut state = self.sync_state()?;
        let (status, entitlement_required) =
            finish_sync_run(logged_in, &mut state, result, timestamp);
        drop(state);
        drop(running);
        if entitlement_required {
            Err(ClientError::EntitlementRequired)
        } else {
            Ok(status)
        }
    }

    /// Fetches a short-lived foreground realtime ticket without exposing the
    /// session token or tenant/device identifiers to the frontend.
    pub async fn realtime_ticket(&self) -> Result<RealtimeTicket, ClientError> {
        let _operation = self.begin_operation()?;
        self.access_token(false).await?;
        let context = self
            .active_sync_context()
            .ok_or(ClientError::AccountRequest)?;
        let client =
            AccountClient::new(&context.server_url).map_err(|_| ClientError::AccountRequest)?;
        let response = match client
            .realtime_ticket(context.tenant_id, context.session_token.expose())
            .await
        {
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

    async fn run_sync_now(&self) -> Result<SyncRunSummary, ClientError> {
        self.ensure_account_runtime_restored()?;
        self.access_token(false).await?;
        match self.run_sync_attempt().await {
            Err(error) if error == "unauthorized" => {
                self.access_token(true).await?;
                self.run_sync_attempt().await.map_err(map_sync_run_error)
            }
            result => result.map_err(map_sync_run_error),
        }
    }

    async fn run_sync_attempt(&self) -> Result<SyncRunSummary, String> {
        let context = self
            .active_sync_context()
            .ok_or_else(|| "sync failed".to_string())?;
        let mut store = SqliteSyncStore::new_secret(self.db_path.clone(), self.db_key());
        let mut clock = || now_ms().map_err(|error| error.to_string());
        let mut key_refresher = ProductionKeyRefresher { client: self };
        let mut pre_push = |store: &mut SqliteSyncStore| {
            self.run_initial_backfill_if_needed(store)
                .map_err(|error| error.to_string())
        };
        let mut summary = taskveil_sync::run_sync_now_with_key_refresh_and_pre_push(
            context.clone(),
            &mut store,
            &mut clock,
            &mut key_refresher,
            &mut pre_push,
        )
        .await?;
        loop {
            let timestamp = now_ms().map_err(|_| "sync failed".to_string())?;
            let settlement = self
                .settle_after_sync_pull(timestamp)
                .map_err(|_| "sync failed".to_string())?;
            if !settlement.outbox_changed {
                break;
            }
            let follow_up = taskveil_sync::run_sync_now_with_key_refresh_and_pre_push(
                context.clone(),
                &mut store,
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
        Ok(summary)
    }

    fn run_initial_backfill_if_needed(
        &self,
        store: &mut SqliteSyncStore,
    ) -> Result<(), ClientError> {
        if self.active_sync_context().is_none()
            || store
                .get_cursor_seq(INITIAL_BACKFILL_CURSOR_NAME)
                .map_err(|_| ClientError::SyncRun)?
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
        let context = self
            .active_sync_context()
            .ok_or(ClientError::AccountRequest)?;
        let mut clock = || now_ms().map_err(|error| error.to_string());
        let mut transaction = store
            .begin_write_transaction()
            .map_err(|_| ClientError::SyncRun)?;
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
        .map_err(|_| ClientError::SyncRun)?;
        transaction
            .set_cursor(INITIAL_BACKFILL_CURSOR_NAME, 1, now_ms()?)
            .map_err(|_| ClientError::SyncRun)?;
        transaction.commit().map_err(|_| ClientError::SyncRun)
    }

    fn active_sync_context(&self) -> Option<ActiveSyncContext> {
        self.ensure_account_runtime_restored().ok()?;
        let account = self.account_state().ok()?;
        let session = account
            .session
            .clone()?
            .logged_in
            .then_some(account.session.clone()?)?;
        let CryptoRuntimeState::Ready(crypto) = &account.crypto else {
            return None;
        };
        let tenant_id = crypto.tenant_id();
        let device_id = crypto.device_id().to_string();
        let keys = crypto.sync_keys().clone();
        let manifest_auth_key =
            taskveil_sync::derive_personal_manifest_auth_key(crypto.master_key()).ok()?;
        drop(account);
        let token = self.current_access_token()?;
        if token.is_empty() || !session.logged_in {
            return None;
        }
        Some(ActiveSyncContext {
            server_url: token.issuer.clone(),
            tenant_id,
            device_id,
            session_token: taskveil_sync::SecretString::new(token.to_string()),
            keys,
            manifest_auth_key,
        })
    }

    fn has_active_sync_context(&self) -> bool {
        self.active_sync_context().is_some()
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

fn map_sync_run_error(error: String) -> ClientError {
    match error.as_str() {
        "upgrade required" => ClientError::UpgradeRequired,
        "entitlement required" => ClientError::EntitlementRequired,
        _ => ClientError::SyncRun,
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
}

impl SyncKeyRefresher for ProductionKeyRefresher<'_> {
    fn refresh<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<LocalSyncKeys, String>> + Send + 'a>> {
        Box::pin(async move {
            self.client
                .refresh_tenant_keys_for_sync()
                .await
                .map_err(|error| error.to_string())
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
) -> (SyncStatus, bool) {
    state.running = false;
    let mut entitlement_required = false;
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
            entitlement_required = true;
        }
        Err(_) => {
            state.last_failure_at = Some(timestamp);
            state.last_error = Some("sync failed".to_string());
        }
    }
    (sync_status(logged_in, state), entitlement_required)
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

        let (status, entitlement_required) =
            finish_sync_run(false, &mut state, Err(ClientError::AccountRequest), 42);

        assert!(!status.logged_in);
        assert!(!status.running);
        assert_eq!(status.last_failure_at, Some(42));
        assert_eq!(status.last_error.as_deref(), Some("sync failed"));
        assert!(!entitlement_required);
    }
}
