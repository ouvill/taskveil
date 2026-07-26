mod convert;

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use convert::{
    local_cursor_to_storage, local_outbox_to_storage, local_quarantine_to_storage,
    local_record_to_storage, storage_alias_to_local, storage_outbox_to_local,
    storage_quarantine_to_local, storage_record_to_local, storage_resync_to_local,
    storage_sweep_to_local,
};
use taskveil_domain::{CompletedTimerSession, List, Task, TaskSeries, TaskTemplate, Uuid};
use taskveil_storage::{
    open_encrypted, InternalMetadataRepository, ListRepository, OwnedSqliteWriteTx,
    SqliteInternalMetadataRepository, SqliteListRepository, SqliteProfileCoordinationRepository,
    SqliteSyncStateRepository, SqliteTaskRepository, SqliteTemplateSeriesRepository,
    SqliteTimerSessionRepository, StorageError, SyncLease, SyncStateRepository, TaskRepository,
    TemplateSeriesRepository, TimerSessionRepository,
};
use taskveil_sync::{
    enqueue::{LocalFullResyncProgress, LocalFullResyncSweepSummary},
    LocalListAlias, LocalMutationSyncStore, LocalSyncAtomicStore, LocalSyncOutboxEntry,
    LocalSyncQuarantineEntry, LocalSyncRecordState, LocalSyncStore, LocalSyncWriteTransaction,
    NewLocalSyncOutboxEntry, StableCursor, SyncCollection,
};
use zeroize::Zeroizing;

pub struct SqliteSyncStore {
    db_path: PathBuf,
    db_key: Zeroizing<[u8; 32]>,
    lease: Option<SyncLease>,
    release_lease_on_drop: bool,
    #[cfg(any(test, feature = "test-support"))]
    coordination: SyncStoreCoordination,
    #[cfg(any(test, feature = "test-support"))]
    preflight_calls: usize,
    #[cfg(any(test, feature = "test-support"))]
    fail_preflight_on_call: Option<usize>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncStoreCoordination {
    LeaseRequired,
    #[cfg(any(test, feature = "test-support"))]
    UnfencedProtocolHarness,
}

pub struct SqliteSyncWriteTx {
    transaction: OwnedSqliteWriteTx,
    lease: Option<SyncLease>,
    runtime_cutover: bool,
}

pub(crate) struct RotationBackfillSnapshot {
    pub lists: Vec<List>,
    pub templates: Vec<TaskTemplate>,
    pub schedules: Vec<TaskSeries>,
    pub tasks: Vec<Task>,
    pub timer_sessions: Vec<CompletedTimerSession>,
}

impl SqliteSyncStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(db_path: PathBuf, db_key: [u8; 32]) -> Self {
        Self {
            db_path,
            db_key: Zeroizing::new(db_key),
            lease: None,
            release_lease_on_drop: false,
            coordination: SyncStoreCoordination::UnfencedProtocolHarness,
            preflight_calls: 0,
            fail_preflight_on_call: None,
        }
    }

    pub(crate) fn new_secret(db_path: PathBuf, db_key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            db_path,
            db_key,
            lease: None,
            release_lease_on_drop: false,
            #[cfg(any(test, feature = "test-support"))]
            coordination: SyncStoreCoordination::LeaseRequired,
            #[cfg(any(test, feature = "test-support"))]
            preflight_calls: 0,
            #[cfg(any(test, feature = "test-support"))]
            fail_preflight_on_call: None,
        }
    }

    pub(crate) fn new_secret_with_lease(
        db_path: PathBuf,
        db_key: Zeroizing<[u8; 32]>,
        lease: SyncLease,
    ) -> Self {
        Self {
            db_path,
            db_key,
            lease: Some(lease),
            release_lease_on_drop: false,
            #[cfg(any(test, feature = "test-support"))]
            coordination: SyncStoreCoordination::LeaseRequired,
            #[cfg(any(test, feature = "test-support"))]
            preflight_calls: 0,
            #[cfg(any(test, feature = "test-support"))]
            fail_preflight_on_call: None,
        }
    }

    /// Injects a lease loss at an exact outer network boundary. This exists
    /// only for protocol harnesses so tests can prove that a missing preflight
    /// would incorrectly allow the corresponding HTTP request.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_preflight_on_call(&mut self, call: usize) {
        assert!(call > 0, "preflight calls are one-based");
        self.fail_preflight_on_call = Some(call);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn preflight_call_count(&self) -> usize {
        self.preflight_calls
    }

    /// Begins the account/device publication transaction while the caller owns
    /// the profile-exclusive OS guard. This is deliberately not a sync-store
    /// constructor: production sync protocol paths must always carry a lease.
    pub(crate) fn begin_profile_cutover_transaction(
        db_path: &Path,
        db_key: &[u8; 32],
    ) -> Result<SqliteSyncWriteTx, String> {
        let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
        let transaction = OwnedSqliteWriteTx::begin(connection).map_err(sync_coordination_error)?;
        Ok(SqliteSyncWriteTx {
            transaction,
            lease: None,
            runtime_cutover: false,
        })
    }

    pub(crate) fn acquire_sync_lease(
        &mut self,
        owner_id: &str,
        runtime_epoch: i64,
        ttl_ms: i64,
    ) -> Result<(), StorageError> {
        if self.lease.is_some() {
            return Err(StorageError::SyncLeaseLost);
        }
        let now = coordination_now_ms()?;
        let connection = open_encrypted(&self.db_path, &self.db_key)?;
        let lease = SqliteProfileCoordinationRepository::new(connection).acquire_sync_lease(
            owner_id,
            now,
            ttl_ms,
            runtime_epoch,
        )?;
        self.lease = Some(lease);
        self.release_lease_on_drop = true;
        Ok(())
    }

    pub(crate) fn release_sync_lease(&mut self) -> Result<(), StorageError> {
        let Some(lease) = self.lease.clone() else {
            return Ok(());
        };
        let connection = open_encrypted(&self.db_path, &self.db_key)?;
        SqliteProfileCoordinationRepository::new(connection)
            .release_sync_lease(&lease, coordination_now_ms()?)?;
        self.lease = None;
        self.release_lease_on_drop = false;
        Ok(())
    }

    pub(crate) fn renew_sync_lease(&mut self) -> Result<Option<SyncLease>, StorageError> {
        let Some(lease) = self.lease.clone() else {
            return Ok(None);
        };
        let connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut coordination = SqliteProfileCoordinationRepository::new(connection);
        let now = coordination_now_ms()?;
        let renewed = coordination.renew_sync_lease(&lease, now, SYNC_LEASE_TTL_MS)?;
        self.lease = Some(renewed.clone());
        Ok(Some(renewed))
    }

    pub(crate) fn active_lease(&self) -> Result<SyncLease, StorageError> {
        self.lease.clone().ok_or(StorageError::SyncLeaseLost)
    }

    /// Runs every outer-store mutation through the same fenced transaction
    /// path used by pull/apply. Sync orchestration intentionally performs
    /// reads between network requests, but it must never be able to publish a
    /// setting, cursor, anchor, quarantine row, or domain row after losing its
    /// lease.
    fn fenced_write<T>(
        &mut self,
        write: impl FnOnce(&mut SqliteSyncWriteTx) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut transaction = self.begin_write_transaction()?;
        let result = write(&mut transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

impl Drop for SqliteSyncStore {
    fn drop(&mut self) {
        if self.release_lease_on_drop {
            let _ = self.release_sync_lease();
        }
    }
}

const SYNC_LEASE_TTL_MS: i64 = 5 * 60 * 1_000;

impl LocalSyncAtomicStore for SqliteSyncStore {
    type WriteTransaction = SqliteSyncWriteTx;

    fn preflight_network_request(&mut self) -> Result<(), String> {
        #[cfg(any(test, feature = "test-support"))]
        if self.coordination == SyncStoreCoordination::UnfencedProtocolHarness
            && self.lease.is_none()
        {
            self.preflight_calls += 1;
            if self.fail_preflight_on_call == Some(self.preflight_calls) {
                return Err("sync lease lost".to_string());
            }
            return Ok(());
        }
        let lease = self
            .renew_sync_lease()
            .map_err(sync_coordination_error)?
            .ok_or_else(|| sync_coordination_error(StorageError::SyncLeaseLost))?;
        let connection =
            open_encrypted(&self.db_path, &self.db_key).map_err(sync_coordination_error)?;
        SqliteProfileCoordinationRepository::new(connection)
            .assert_sync_lease(
                &lease,
                coordination_now_ms().map_err(sync_coordination_error)?,
            )
            .map_err(sync_coordination_error)
    }

    fn begin_write_transaction(&mut self) -> Result<Self::WriteTransaction, String> {
        let lease = self.renew_sync_lease().map_err(sync_coordination_error)?;
        #[cfg(any(test, feature = "test-support"))]
        let unfenced_harness = self.coordination == SyncStoreCoordination::UnfencedProtocolHarness;
        #[cfg(not(any(test, feature = "test-support")))]
        let unfenced_harness = false;
        if lease.is_none() && !unfenced_harness {
            return Err(sync_coordination_error(StorageError::SyncLeaseLost));
        }
        let connection =
            open_encrypted(&self.db_path, &self.db_key).map_err(sync_coordination_error)?;
        let transaction = OwnedSqliteWriteTx::begin(connection).map_err(sync_coordination_error)?;
        if let Some(lease) = lease.as_ref() {
            transaction
                .assert_sync_lease(
                    lease,
                    coordination_now_ms().map_err(sync_coordination_error)?,
                )
                .map_err(sync_coordination_error)?;
        }
        Ok(SqliteSyncWriteTx {
            transaction,
            lease,
            runtime_cutover: false,
        })
    }
}

impl SqliteSyncWriteTx {
    pub(crate) fn rotation_backfill_snapshot(&self) -> Result<RotationBackfillSnapshot, String> {
        Ok(RotationBackfillSnapshot {
            lists: self
                .transaction
                .list_lists_including_archived()
                .map_err(sync_coordination_error)?,
            templates: self
                .transaction
                .list_templates()
                .map_err(sync_coordination_error)?,
            schedules: self
                .transaction
                .list_series()
                .map_err(sync_coordination_error)?,
            tasks: self
                .transaction
                .list_all_tasks_for_sync()
                .map_err(sync_coordination_error)?,
            timer_sessions: self
                .transaction
                .list_timer_sessions_for_sync()
                .map_err(sync_coordination_error)?,
        })
    }

    pub(crate) fn persist_local_crypto_context(
        &mut self,
        identity: crate::LocalCryptoIdentity,
        master_key: &[u8; 32],
        sync_keys: taskveil_sync::LocalSyncKeys,
        now_ms: i64,
    ) -> Result<crate::LocalCryptoContext, String> {
        if !self.runtime_cutover {
            if let Some(lease) = self.lease.as_ref() {
                self.transaction
                    .assert_sync_lease(
                        lease,
                        coordination_now_ms().map_err(sync_coordination_error)?,
                    )
                    .map_err(sync_coordination_error)?;
            }
        }
        let (context, runtime_changed) =
            crate::local_crypto::persist_local_crypto_context_in_transaction(
                &mut self.transaction,
                identity,
                master_key,
                sync_keys,
                now_ms,
            )
            .map_err(sync_coordination_error)?;
        self.runtime_cutover |= runtime_changed;
        Ok(context)
    }

    pub(crate) fn has_runtime_cutover(&self) -> bool {
        self.runtime_cutover
    }

    #[cfg(test)]
    pub(crate) fn bump_runtime_epoch(&mut self, now_ms: i64) -> Result<(), String> {
        self.transaction
            .bump_runtime_epoch(now_ms)
            .map(|_| {
                self.runtime_cutover = true;
            })
            .map_err(sync_coordination_error)
    }
}

impl LocalMutationSyncStore for SqliteSyncStore {
    fn has_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .has_outbox_head(collection.as_str(), record_id)
                .map_err(sync_coordination_error)
        })
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        with_internal_metadata_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_internal_metadata(key)
                .map_err(sync_coordination_error)
        })
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.set_setting(key, value, updated_at))
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.put_outbox_head(entry))
    }

    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_record_state(collection.as_str(), record_id)
                .map(|state| state.map(storage_record_to_local))
                .map_err(sync_coordination_error)
        })
    }

    fn put_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
        state: LocalSyncRecordState,
        updated_at: i64,
    ) -> Result<(), String> {
        self.fenced_write(|transaction| {
            transaction.put_record_state(collection, record_id, state, updated_at)
        })
    }
}

impl LocalSyncStore for SqliteSyncStore {
    fn load_full_resync(&mut self) -> Result<Option<LocalFullResyncProgress>, String> {
        let progress = with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .load_full_resync()
                .map_err(sync_coordination_error)
        })?;
        let awaiting_base_ack = self
            .get_setting(taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY)?
            .is_some_and(|token| !token.is_empty());
        Ok(progress.map(|progress| storage_resync_to_local(progress, awaiting_base_ack)))
    }

    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_outbox_heads(limit)
                .map_err(sync_coordination_error)
        })?
        .into_iter()
        .map(storage_outbox_to_local)
        .collect()
    }

    fn list_all_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_all_outbox_heads(limit)
                .map_err(sync_coordination_error)
        })?
        .into_iter()
        .map(storage_outbox_to_local)
        .collect()
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.ack_outbox_op(op_id))
    }

    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.delete_outbox_head(collection, record_id))
    }

    fn get_cursor_seq(&mut self, name: &str) -> Result<Option<i64>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_cursor(name)
                .map(|cursor| cursor.map(|cursor| cursor.seq))
                .map_err(sync_coordination_error)
        })
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.set_cursor(name, seq, updated_at))
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.delete_cursor(name))
    }

    fn put_quarantine(&mut self, entry: LocalSyncQuarantineEntry) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.put_quarantine(entry))
    }

    fn list_quarantine(&mut self, limit: usize) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_quarantine(limit)
                .map_err(sync_coordination_error)
        })?
        .into_iter()
        .map(storage_quarantine_to_local)
        .collect()
    }

    fn list_replayable_quarantine(
        &mut self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_replayable_quarantine(after, limit)
                .map_err(sync_coordination_error)
        })?
        .into_iter()
        .map(storage_quarantine_to_local)
        .collect()
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.delete_quarantine(record_id))
    }

    fn list_record_states(
        &mut self,
        collection: SyncCollection,
    ) -> Result<Vec<(Uuid, LocalSyncRecordState)>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_record_states(collection.as_str())
                .map(|states| {
                    states
                        .into_iter()
                        .map(|state| (state.record_id, storage_record_to_local(state)))
                        .collect()
                })
                .map_err(sync_coordination_error)
        })
    }

    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .has_live_quarantine(collection.as_str())
                .map_err(sync_coordination_error)
        })
    }

    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_list_aliases()
                .map(|aliases| aliases.into_iter().map(storage_alias_to_local).collect())
                .map_err(sync_coordination_error)
        })
    }

    fn replace_list_aliases(
        &mut self,
        aliases: &[LocalListAlias],
        updated_at: i64,
    ) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.replace_list_aliases(aliases, updated_at))
    }

    fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .resolve_list_alias(list_id)
                .map_err(sync_coordination_error)
        })
    }

    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.materialize_canonical_list(canonical_list_id))
    }

    fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_default()
                .map(|list| list.map(|list| list.id))
                .map_err(sync_coordination_error)
        })
    }

    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get(id) {
                Ok(list) => Ok(Some(list)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(sync_coordination_error(error)),
            }
        })
    }

    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.upsert_list_for_sync(list))
    }

    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String> {
        self.fenced_write(|transaction| transaction.delete_list_and_rehome_tasks_for_sync(list_id))
    }

    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get(id) {
                Ok(task) => Ok(Some(task)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(sync_coordination_error(error)),
            }
        })
    }

    fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_all_for_sync()
                .map(|tasks| {
                    tasks
                        .into_iter()
                        .filter(|task| task.list_id == list_id)
                        .collect()
                })
                .map_err(sync_coordination_error)
        })
    }

    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_all_for_sync()
                .map_err(sync_coordination_error)
        })
    }

    fn list_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<Vec<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_subtree_for_sync(task_id)
                .map_err(sync_coordination_error)
        })
    }

    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.upsert_task_for_sync(task))
    }

    fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, String> {
        self.fenced_write(|transaction| transaction.delete_task_subtree_for_sync(task_id))
    }

    fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_template(id) {
                Ok(template) => Ok(Some(template)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(sync_coordination_error(error)),
            }
        })
    }

    fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.upsert_template_for_sync(template))
    }

    fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.delete_template_for_sync(id))
    }

    fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_series(id) {
                Ok(schedule) => Ok(Some(schedule)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(sync_coordination_error(error)),
            }
        })
    }

    fn upsert_series_for_sync(&mut self, schedule: TaskSeries) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.upsert_series_for_sync(schedule))
    }

    fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.delete_series_for_sync(id))
    }

    fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_completed(id) {
                Ok(session) => Ok(Some(session)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(sync_coordination_error(error)),
            }
        })
    }

    fn upsert_timer_session_for_sync(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<(), String> {
        self.fenced_write(|transaction| transaction.upsert_timer_session_for_sync(session))
    }

    fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.delete_timer_session_for_sync(id))
    }

    fn list_timer_sessions_by_task(
        &mut self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_completed_by_task(task_id)
                .map_err(sync_coordination_error)
        })
    }

    fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
        self.fenced_write(|transaction| transaction.clear_active_timer_for_task(task_id))
    }
}

impl LocalMutationSyncStore for SqliteSyncWriteTx {
    fn has_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        self.transaction
            .has_outbox_head(collection.as_str(), record_id)
            .map_err(sync_coordination_error)
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        self.transaction
            .get_internal_metadata(key)
            .map_err(sync_coordination_error)
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String> {
        self.transaction
            .set_internal_metadata(key, value, updated_at)
            .map_err(sync_coordination_error)
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        self.transaction
            .put_outbox_head(local_outbox_to_storage(entry))
            .map(|_| ())
            .map_err(sync_coordination_error)
    }

    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String> {
        self.transaction
            .get_record_state(collection.as_str(), record_id)
            .map(|state| state.map(storage_record_to_local))
            .map_err(sync_coordination_error)
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
            .map_err(sync_coordination_error)
    }
}

impl LocalSyncStore for SqliteSyncWriteTx {
    fn load_full_resync(&mut self) -> Result<Option<LocalFullResyncProgress>, String> {
        let progress = self
            .transaction
            .load_full_resync()
            .map_err(sync_coordination_error)?;
        let awaiting_base_ack = self
            .transaction
            .get_internal_metadata(taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY)
            .map_err(sync_coordination_error)?
            .is_some_and(|token| !token.is_empty());
        Ok(progress.map(|progress| storage_resync_to_local(progress, awaiting_base_ack)))
    }

    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        self.transaction
            .list_outbox_heads(limit)
            .map_err(sync_coordination_error)?
            .into_iter()
            .map(storage_outbox_to_local)
            .collect()
    }

    fn list_all_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        self.transaction
            .list_all_outbox_heads(limit)
            .map_err(sync_coordination_error)?
            .into_iter()
            .map(storage_outbox_to_local)
            .collect()
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
        self.transaction
            .ack_outbox_op(op_id)
            .map_err(sync_coordination_error)
    }

    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        self.transaction
            .delete_outbox_head(collection.as_str(), record_id)
            .map_err(sync_coordination_error)
    }

    fn get_cursor_seq(&mut self, name: &str) -> Result<Option<i64>, String> {
        self.transaction
            .get_cursor(name)
            .map(|cursor| cursor.map(|cursor| cursor.seq))
            .map_err(sync_coordination_error)
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), String> {
        self.transaction
            .set_cursor(name, seq, updated_at)
            .map_err(sync_coordination_error)
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), String> {
        self.transaction
            .delete_cursor(name)
            .map_err(sync_coordination_error)
    }

    fn put_quarantine(&mut self, entry: LocalSyncQuarantineEntry) -> Result<(), String> {
        self.transaction
            .put_quarantine(local_quarantine_to_storage(entry))
            .map_err(sync_coordination_error)
    }

    fn list_quarantine(&mut self, limit: usize) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        self.transaction
            .list_quarantine(limit)
            .map_err(sync_coordination_error)?
            .into_iter()
            .map(storage_quarantine_to_local)
            .collect()
    }

    fn list_replayable_quarantine(
        &mut self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        self.transaction
            .list_replayable_quarantine(after, limit)
            .map_err(sync_coordination_error)?
            .into_iter()
            .map(storage_quarantine_to_local)
            .collect()
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_quarantine(record_id)
            .map_err(sync_coordination_error)
    }

    fn list_record_states(
        &mut self,
        collection: SyncCollection,
    ) -> Result<Vec<(Uuid, LocalSyncRecordState)>, String> {
        self.transaction
            .list_record_states(collection.as_str())
            .map(|states| {
                states
                    .into_iter()
                    .map(|state| (state.record_id, storage_record_to_local(state)))
                    .collect()
            })
            .map_err(sync_coordination_error)
    }

    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
        self.transaction
            .has_live_quarantine(collection.as_str())
            .map_err(sync_coordination_error)
    }

    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
        self.transaction
            .list_list_aliases()
            .map(|aliases| aliases.into_iter().map(storage_alias_to_local).collect())
            .map_err(sync_coordination_error)
    }

    fn replace_list_aliases(
        &mut self,
        aliases: &[LocalListAlias],
        updated_at: i64,
    ) -> Result<(), String> {
        replace_list_aliases_in_transaction(&mut self.transaction, aliases, updated_at)
    }

    fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String> {
        self.transaction
            .resolve_list_alias(list_id)
            .map_err(sync_coordination_error)
    }

    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
        self.transaction
            .materialize_canonical_list(canonical_list_id)
            .map_err(sync_coordination_error)
    }

    fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
        self.transaction
            .default_list_id()
            .map_err(sync_coordination_error)
    }

    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
        self.transaction
            .get_list(id)
            .map_err(sync_coordination_error)
    }

    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
        self.transaction
            .upsert_list_for_sync(list)
            .map_err(sync_coordination_error)
    }

    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String> {
        self.transaction
            .delete_list_and_rehome_tasks_for_sync(list_id)
            .map_err(sync_coordination_error)
    }

    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
        self.transaction
            .get_task(id)
            .map_err(sync_coordination_error)
    }

    fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String> {
        self.transaction
            .list_tasks_by_list(list_id)
            .map_err(sync_coordination_error)
    }

    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
        self.transaction
            .list_all_tasks_for_sync()
            .map_err(sync_coordination_error)
    }

    fn list_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<Vec<Task>, String> {
        self.transaction
            .list_task_subtree_for_sync(task_id)
            .map_err(sync_coordination_error)
    }

    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
        self.transaction
            .upsert_task_for_sync(task)
            .map_err(sync_coordination_error)
    }

    fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, String> {
        self.transaction
            .delete_task_subtree_for_sync(task_id)
            .map_err(sync_coordination_error)
    }

    fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
        match self.transaction.get_template(id) {
            Ok(template) => Ok(Some(template)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(sync_coordination_error(error)),
        }
    }

    fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
        self.transaction
            .upsert_template(template)
            .map_err(sync_coordination_error)
    }

    fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_template(id)
            .map_err(sync_coordination_error)
    }

    fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
        match self.transaction.get_series(id) {
            Ok(schedule) => Ok(Some(schedule)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(sync_coordination_error(error)),
        }
    }

    fn upsert_series_for_sync(&mut self, schedule: TaskSeries) -> Result<(), String> {
        self.transaction
            .upsert_series(schedule)
            .map_err(sync_coordination_error)
    }

    fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_series(id)
            .map_err(sync_coordination_error)
    }

    fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        match self.transaction.get_timer_session(id) {
            Ok(session) => Ok(Some(session)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(sync_coordination_error(error)),
        }
    }

    fn upsert_timer_session_for_sync(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<(), String> {
        self.transaction
            .insert_timer_session(session)
            .map(|_| ())
            .map_err(sync_coordination_error)
    }

    fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_timer_session(id)
            .map_err(sync_coordination_error)
    }

    fn list_timer_sessions_by_task(
        &mut self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        self.transaction
            .list_timer_sessions_by_task_for_sync(task_id)
            .map_err(sync_coordination_error)
    }

    fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
        self.transaction
            .clear_active_timer_for_task_for_sync(task_id)
            .map_err(sync_coordination_error)
    }
}

impl LocalSyncWriteTransaction for SqliteSyncWriteTx {
    fn start_full_resync(
        &mut self,
        generation_id: Uuid,
        continuity_generation: i64,
        base_seq: i64,
        now_ms: i64,
    ) -> Result<LocalFullResyncProgress, String> {
        self.transaction
            .start_full_resync(generation_id, continuity_generation, base_seq, now_ms)
            .map(|progress| storage_resync_to_local(progress, false))
            .map_err(sync_coordination_error)
    }

    fn mark_full_resync_record(
        &mut self,
        generation_id: Uuid,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<(), String> {
        self.transaction
            .mark_full_resync_record(generation_id, collection.as_str(), record_id)
            .map_err(sync_coordination_error)
    }

    fn advance_full_resync_base(
        &mut self,
        generation_id: Uuid,
        next_cursor: Option<&StableCursor>,
        base_complete: bool,
        now_ms: i64,
    ) -> Result<(), String> {
        let cursor = next_cursor.map(local_cursor_to_storage);
        self.transaction
            .advance_full_resync_base(generation_id, cursor.as_ref(), base_complete, now_ms)
            .map_err(sync_coordination_error)
    }

    fn advance_full_resync_delta(
        &mut self,
        generation_id: Uuid,
        delta_cursor: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.transaction
            .advance_full_resync_delta(generation_id, delta_cursor, now_ms)
            .map_err(sync_coordination_error)
    }

    fn enter_full_resync_sweep(
        &mut self,
        generation_id: Uuid,
        closure_high_water: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.transaction
            .enter_full_resync_sweep(generation_id, closure_high_water, now_ms)
            .map_err(sync_coordination_error)
    }

    fn sweep_full_resync_batch(
        &mut self,
        generation_id: Uuid,
        limit: usize,
        now_ms: i64,
    ) -> Result<LocalFullResyncSweepSummary, String> {
        self.transaction
            .sweep_full_resync_batch(generation_id, limit, now_ms)
            .map(storage_sweep_to_local)
            .map_err(sync_coordination_error)
    }

    fn finalize_full_resync(
        &mut self,
        generation_id: Uuid,
        cursor_name: &str,
        now_ms: i64,
    ) -> Result<i64, String> {
        self.transaction
            .finalize_full_resync(generation_id, cursor_name, now_ms)
            .map_err(sync_coordination_error)
    }

    fn reset_full_resync(&mut self) -> Result<(), String> {
        self.transaction
            .reset_full_resync()
            .map_err(sync_coordination_error)
    }

    fn commit(self) -> Result<(), String> {
        if !self.runtime_cutover {
            if let Some(lease) = self.lease.as_ref() {
                self.transaction
                    .assert_sync_lease(
                        lease,
                        coordination_now_ms().map_err(sync_coordination_error)?,
                    )
                    .map_err(sync_coordination_error)?;
            }
        }
        self.transaction
            .commit()
            .map(|_| ())
            .map_err(sync_coordination_error)
    }
}

fn coordination_now_ms() -> Result<i64, StorageError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StorageError::ProfileCoordinationClockRollback)?;
    i64::try_from(duration.as_millis()).map_err(|_| StorageError::ProfileCoordinationOverflow)
}

fn sync_coordination_error(error: StorageError) -> String {
    if error.is_database_busy() {
        return "database busy".to_string();
    }
    match error {
        StorageError::SyncLeaseBusy => "sync lease busy".to_string(),
        StorageError::SyncLeaseLost | StorageError::ProfileRuntimeEpochChanged { .. } => {
            "sync lease lost".to_string()
        }
        error => error.to_string(),
    }
}

fn replace_list_aliases_in_transaction(
    transaction: &mut OwnedSqliteWriteTx,
    aliases: &[LocalListAlias],
    updated_at: i64,
) -> Result<(), String> {
    let canonical_list_id = if let Some(first) = aliases.first() {
        if aliases.iter().any(|alias| {
            alias.canonical_list_id != first.canonical_list_id
                || alias.alias_list_id == first.canonical_list_id
        }) {
            return Err("invalid canonical Inbox alias set".to_string());
        }
        first.canonical_list_id
    } else {
        let existing = transaction
            .list_list_aliases()
            .map_err(sync_coordination_error)?;
        let Some(first) = existing.first() else {
            return Ok(());
        };
        first.canonical_list_id
    };
    let alias_list_ids = aliases
        .iter()
        .map(|alias| alias.alias_list_id)
        .collect::<Vec<_>>();
    transaction
        .replace_list_aliases(canonical_list_id, &alias_list_ids, updated_at)
        .map_err(sync_coordination_error)
}

fn with_sync_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteSyncStateRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteSyncStateRepository::new(connection);
    f(&mut repository)
}

fn with_internal_metadata_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteInternalMetadataRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteInternalMetadataRepository::new(connection);
    f(&mut repository)
}

fn with_task_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTaskRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteTaskRepository::new(connection);
    f(&mut repository)
}

fn with_recurrence_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTemplateSeriesRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteTemplateSeriesRepository::new(connection);
    f(&mut repository)
}

fn with_timer_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTimerSessionRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteTimerSessionRepository::new(connection);
    f(&mut repository)
}

fn with_list_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteListRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(sync_coordination_error)?;
    let mut repository = SqliteListRepository::new(connection);
    f(&mut repository)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    use super::*;
    use taskveil_domain::{
        new_list, new_task, CompletedTimerSession, SeriesCursor, TaskBlueprint, TaskBlueprintNode,
        TaskContent, TaskSeriesConfig, TimerFinishKind, TimerMode, TASK_BLUEPRINT_SCHEMA_REVISION,
    };
    use taskveil_storage::{ListRepository, LocalProfileBinding, LocalTenantRootKeyBundle};
    use taskveil_sync::{
        enqueue_backfill, EncryptedSyncState, LocalSyncKeys, LocalSyncSemanticState,
        PullFailureReason, SYNC_CURSOR_NAME,
    };
    use tempfile::tempdir;

    const DB_KEY: [u8; 32] = [0x51; 32];

    #[test]
    fn child_sync_lease_actor() {
        let Some(db_path) = std::env::var_os("TASKVEIL_SYNC_LEASE_CHILD_DB") else {
            return;
        };
        let mut store = SqliteSyncStore::new(PathBuf::from(db_path), DB_KEY);
        store.acquire_sync_lease("child", 1, 60_000).unwrap();
        println!("TASKVEIL_SYNC_LEASE_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        if std::env::var_os("TASKVEIL_SYNC_LEASE_CHECK_STALE").is_some() {
            assert_eq!(
                store.preflight_network_request(),
                Err("sync lease lost".to_string())
            );
            match store.begin_write_transaction() {
                Err(error) => assert_eq!(error, "sync lease lost"),
                Ok(_) => panic!("stale child unexpectedly opened a write transaction"),
            }
        }
    }

    #[test]
    fn process_crash_keeps_lease_fenced_until_expiry_then_allows_takeover() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sqlite_sync_store::tests::child_sync_lease_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_SYNC_LEASE_CHILD_DB", &db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_SYNC_LEASE_READY") {
                break;
            }
        }

        let mut contender = SqliteSyncStore::new(db_path, DB_KEY);
        assert!(matches!(
            contender.acquire_sync_lease("parent", 1, 60_000),
            Err(StorageError::SyncLeaseBusy)
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(matches!(
            contender.acquire_sync_lease("parent", 1, 60_000),
            Err(StorageError::SyncLeaseBusy)
        ));
        open_encrypted(&contender.db_path, &DB_KEY)
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        contender.acquire_sync_lease("parent", 1, 60_000).unwrap();
    }

    #[test]
    fn real_child_expiry_takeover_fences_requests_and_commits_without_sleep() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sqlite_sync_store::tests::child_sync_lease_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_SYNC_LEASE_CHILD_DB", &db_path)
            .env("TASKVEIL_SYNC_LEASE_CHECK_STALE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_SYNC_LEASE_READY") {
                break;
            }
        }

        let mut contender = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        assert!(matches!(
            contender.acquire_sync_lease("parent", 1, 60_000),
            Err(StorageError::SyncLeaseBusy)
        ));
        open_encrypted(&db_path, &DB_KEY)
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        contender.acquire_sync_lease("parent", 1, 60_000).unwrap();

        child.stdin.take().unwrap().write_all(b"check\n").unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn lost_lease_is_preserved_as_a_typed_sync_error_across_string_traits() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        store.acquire_sync_lease("owner", 1, 1_000).unwrap();
        open_encrypted(&db_path, &DB_KEY)
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        match store.begin_write_transaction() {
            Err(error) => assert_eq!(error, "sync lease lost"),
            Ok(_) => panic!("expired lease unexpectedly started a write transaction"),
        }
    }

    #[test]
    fn production_sync_store_without_a_lease_fails_closed() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("missing-lease.sqlite3");
        let mut store = SqliteSyncStore::new_secret(db_path, Zeroizing::new(DB_KEY));

        assert_eq!(
            store.preflight_network_request().unwrap_err(),
            "sync lease lost"
        );
        match store.begin_write_transaction() {
            Err(error) => assert_eq!(error, "sync lease lost"),
            Ok(_) => panic!("production sync store opened an unfenced transaction"),
        }
    }

    #[test]
    fn cancel_and_error_drop_release_the_sync_lease() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("lease-drop.sqlite3");

        {
            let mut canceled = SqliteSyncStore::new_secret(db_path.clone(), Zeroizing::new(DB_KEY));
            canceled.acquire_sync_lease("canceled", 1, 60_000).unwrap();
            // Dropping the future that owns this store follows this path.
        }
        {
            let mut after_cancel =
                SqliteSyncStore::new_secret(db_path.clone(), Zeroizing::new(DB_KEY));
            after_cancel
                .acquire_sync_lease("after-cancel", 1, 60_000)
                .unwrap();
        }

        let transport_error = (|| -> Result<(), &'static str> {
            let mut failed = SqliteSyncStore::new_secret(db_path.clone(), Zeroizing::new(DB_KEY));
            failed
                .acquire_sync_lease("failed", 1, 60_000)
                .map_err(|_| "lease")?;
            Err("transport")
        })();
        assert_eq!(transport_error, Err("transport"));
        let mut after_error = SqliteSyncStore::new_secret(db_path, Zeroizing::new(DB_KEY));
        after_error
            .acquire_sync_lease("after-error", 1, 60_000)
            .unwrap();
    }

    #[test]
    fn protocol_harness_can_fail_an_exact_network_preflight() {
        let temp = tempdir().unwrap();
        let mut store = SqliteSyncStore::new(temp.path().join("fault.sqlite3"), DB_KEY);
        store.fail_preflight_on_call(2);

        store.preflight_network_request().unwrap();
        assert_eq!(
            store.preflight_network_request(),
            Err("sync lease lost".to_string())
        );
        assert_eq!(store.preflight_call_count(), 2);
    }

    #[test]
    fn storage_coordination_errors_have_stable_sync_adapter_names() {
        assert_eq!(
            sync_coordination_error(StorageError::SyncLeaseBusy),
            "sync lease busy"
        );
        assert_eq!(
            sync_coordination_error(StorageError::SyncLeaseLost),
            "sync lease lost"
        );
        assert_eq!(
            sync_coordination_error(StorageError::ProfileRuntimeEpochChanged {
                expected: 1,
                actual: 2,
            }),
            "sync lease lost"
        );
    }

    #[test]
    fn sqlite_write_contention_is_preserved_as_database_busy() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let blocker =
            OwnedSqliteWriteTx::begin(open_encrypted(&db_path, &DB_KEY).unwrap()).unwrap();
        let mut contender = SqliteSyncStore::new(db_path, DB_KEY);

        match contender.begin_write_transaction() {
            Err(error) => assert_eq!(error, "database busy"),
            Ok(_) => panic!("contending writer unexpectedly acquired the database"),
        }
        drop(blocker);
    }

    #[test]
    fn takeover_stops_the_old_run_before_the_next_network_request() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut old_store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        old_store.acquire_sync_lease("old-run", 1, 60_000).unwrap();

        open_encrypted(&db_path, &DB_KEY)
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        let mut new_store = SqliteSyncStore::new(db_path, DB_KEY);
        new_store.acquire_sync_lease("new-run", 1, 60_000).unwrap();

        let mut remote_request_count = 0;
        let result = old_store.preflight_network_request().map(|()| {
            remote_request_count += 1;
        });

        assert_eq!(result.unwrap_err(), "sync lease lost");
        assert_eq!(remote_request_count, 0);
    }

    #[test]
    fn epoch_change_aborts_old_store_and_only_a_new_run_can_acquire() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut old_store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        old_store.acquire_sync_lease("old-run", 1, 60_000).unwrap();
        let runtime =
            SqliteProfileCoordinationRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .bump_runtime_epoch(coordination_now_ms().unwrap())
                .unwrap();
        assert_eq!(runtime.runtime_epoch, 2);

        match old_store.begin_write_transaction() {
            Err(error) => assert_eq!(error, "sync lease lost"),
            Ok(_) => panic!("stale run unexpectedly started a write transaction"),
        }
        assert!(matches!(
            old_store.acquire_sync_lease("old-run", runtime.runtime_epoch, 60_000),
            Err(StorageError::SyncLeaseLost)
        ));
        assert_eq!(old_store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(), None);

        let mut new_store = SqliteSyncStore::new(db_path, DB_KEY);
        new_store
            .acquire_sync_lease("new-run", runtime.runtime_epoch, 60_000)
            .unwrap();
        let mut transaction = new_store.begin_write_transaction().unwrap();
        transaction.set_cursor(SYNC_CURSOR_NAME, 7, 100).unwrap();
        transaction.commit().unwrap();
        assert_eq!(new_store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(), Some(7));
    }

    #[test]
    fn runtime_cutover_remains_sticky_across_two_key_persists_in_one_transaction() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("sticky-cutover.sqlite3");
        let tenant_id = Uuid::now_v7();
        let identity = crate::LocalCryptoIdentity {
            tenant_id,
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let keys = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new([0x61; 32])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        };
        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        store.acquire_sync_lease("owner", 1, 60_000).unwrap();
        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .persist_local_crypto_context(identity, &[0x62; 32], keys.clone(), 10)
            .unwrap();
        transaction
            .persist_local_crypto_context(identity, &[0x62; 32], keys, 11)
            .unwrap();
        transaction
            .commit()
            .expect("the first cutover remains authoritative");

        assert_eq!(
            SqliteProfileCoordinationRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .load_runtime()
                .unwrap()
                .runtime_epoch,
            2
        );
    }

    #[test]
    fn takeover_rejects_every_outer_metadata_write_from_the_old_run() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut old_store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        old_store
            .set_setting("sync_upgrade_required", "old", 1)
            .unwrap();
        old_store.set_cursor(SYNC_CURSOR_NAME, 1, 1).unwrap();
        old_store.acquire_sync_lease("old-run", 1, 60_000).unwrap();

        // Deterministically cross the network-wait barrier without sleeping:
        // expire A's durable lease, let B take over and publish its values.
        open_encrypted(&db_path, &DB_KEY)
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        let mut new_store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        new_store.acquire_sync_lease("new-run", 1, 60_000).unwrap();
        new_store
            .set_setting("sync_upgrade_required", "new", 2)
            .unwrap();
        new_store.set_cursor(SYNC_CURSOR_NAME, 2, 2).unwrap();

        for key in [
            "sync_upgrade_required",
            taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
            taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
            taskveil_sync::KEY_ROTATION_PENDING_METADATA_KEY,
        ] {
            assert_eq!(
                old_store.set_setting(key, "stale", 3).unwrap_err(),
                "sync lease lost"
            );
        }
        assert_eq!(
            old_store.set_cursor(SYNC_CURSOR_NAME, 3, 3).unwrap_err(),
            "sync lease lost"
        );
        assert_eq!(
            old_store.delete_cursor(SYNC_CURSOR_NAME).unwrap_err(),
            "sync lease lost"
        );
        assert_eq!(
            old_store
                .materialize_canonical_list(Uuid::now_v7())
                .unwrap_err(),
            "sync lease lost"
        );
        match old_store.begin_write_transaction() {
            Err(error) => assert_eq!(error, "sync lease lost"),
            Ok(_) => panic!("stale run unexpectedly opened a resync write transaction"),
        }

        let observer = SqliteSyncStore::new(db_path, DB_KEY);
        let mut observer = observer;
        assert_eq!(
            observer.get_setting("sync_upgrade_required").unwrap(),
            Some("new".to_string())
        );
        assert_eq!(observer.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(), Some(2));
        for key in [
            taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
            taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
            taskveil_sync::KEY_ROTATION_PENDING_METADATA_KEY,
        ] {
            assert_eq!(observer.get_setting(key).unwrap(), None);
        }
    }

    #[derive(Clone)]
    struct AdapterFixtures {
        canonical: List,
        alias: List,
        task: Task,
        template: TaskTemplate,
        series: TaskSeries,
        timer: CompletedTimerSession,
        live_outbox_op_id: Uuid,
        tombstone_outbox_op_id: Uuid,
        live_quarantine_id: Uuid,
        tombstone_quarantine_id: Uuid,
    }

    impl AdapterFixtures {
        fn new() -> Self {
            let canonical = new_list("Canonical".into(), "a0".into(), 1).unwrap();
            let alias = new_list("Alias".into(), "a1".into(), 1).unwrap();
            let task =
                new_task(canonical.id, None, "Contract task".into(), "a0".into(), 1).unwrap();
            let blueprint = TaskBlueprint {
                schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                nodes: vec![TaskBlueprintNode {
                    node_key: "root".into(),
                    parent_node_key: None,
                    sibling_order: 0,
                    content: TaskContent {
                        title: "Contract template task".into(),
                        note: String::new(),
                        priority: 0,
                        estimated_minutes: Some(15),
                    },
                }],
            };
            let template = TaskTemplate {
                id: Uuid::now_v7(),
                name: "Contract template".into(),
                default_list_id: Some(canonical.id),
                blueprint: blueprint.clone(),
                blueprint_revision: "template-r1".into(),
                created_at: 1,
                updated_at: 1,
            };
            let series = TaskSeries {
                id: Uuid::now_v7(),
                config: TaskSeriesConfig {
                    blueprint,
                    target_list_id: Some(canonical.id),
                    rrule: "FREQ=DAILY".into(),
                    starts_at: 10_000,
                    time_zone: "UTC".into(),
                    enabled: true,
                    config_revision: "series-r1".into(),
                    config_parent_revision: None,
                    config_effective_from: 1,
                    lineage: Vec::new(),
                },
                cursor: SeriesCursor::Pending(10_000),
                created_at: 1,
                updated_at: 1,
            };
            let timer = CompletedTimerSession {
                id: Uuid::now_v7(),
                task_id: task.id,
                mode: TimerMode::Stopwatch,
                finish_kind: TimerFinishKind::Completed,
                started_at: 1_000,
                ended_at: 5_000,
                active_duration_ms: 3_000,
                created_at: 5_100,
            };
            Self {
                canonical,
                alias,
                task,
                template,
                series,
                timer,
                live_outbox_op_id: Uuid::now_v7(),
                tombstone_outbox_op_id: Uuid::now_v7(),
                live_quarantine_id: Uuid::now_v7(),
                tombstone_quarantine_id: Uuid::now_v7(),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct AdapterSnapshot {
        setting: Option<String>,
        outbox: Vec<LocalSyncOutboxEntry>,
        live_record: Option<LocalSyncRecordState>,
        tombstone_record: Option<LocalSyncRecordState>,
        cursor: Option<i64>,
        quarantine: Vec<LocalSyncQuarantineEntry>,
        aliases: Vec<LocalListAlias>,
        resolved_alias: Uuid,
        list: Option<List>,
        task: Option<Task>,
        tasks: Vec<Task>,
        template: Option<TaskTemplate>,
        series: Option<TaskSeries>,
        timer: Option<CompletedTimerSession>,
        timers: Vec<CompletedTimerSession>,
    }

    fn seed_adapter_contract<S: LocalSyncStore>(store: &mut S, fixtures: &AdapterFixtures) {
        store.set_setting("adapter_contract", "value", 10).unwrap();
        store
            .upsert_list_for_sync(fixtures.canonical.clone())
            .unwrap();
        store.upsert_list_for_sync(fixtures.alias.clone()).unwrap();
        store.upsert_task_for_sync(fixtures.task.clone()).unwrap();
        store
            .upsert_template_for_sync(fixtures.template.clone())
            .unwrap();
        store
            .upsert_series_for_sync(fixtures.series.clone())
            .unwrap();
        store
            .upsert_timer_session_for_sync(fixtures.timer.clone())
            .unwrap();
        store
            .materialize_canonical_list(fixtures.canonical.id)
            .unwrap();
        store
            .replace_list_aliases(
                &[LocalListAlias {
                    alias_list_id: fixtures.alias.id,
                    canonical_list_id: fixtures.canonical.id,
                }],
                11,
            )
            .unwrap();

        store
            .put_outbox_head(NewLocalSyncOutboxEntry {
                op_id: fixtures.live_outbox_op_id,
                record_id: fixtures.canonical.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: "10:0:device".into(),
                state: EncryptedSyncState::Live {
                    mutation_hlc: "10:0:device".into(),
                    blob: vec![1, 2, 3],
                },
                created_at: 10,
            })
            .unwrap();
        store
            .put_outbox_head(NewLocalSyncOutboxEntry {
                op_id: fixtures.tombstone_outbox_op_id,
                record_id: fixtures.task.id,
                collection: SyncCollection::Tasks,
                base_revision_hlc: Some("9:0:device".into()),
                revision_hlc: "11:0:device".into(),
                state: EncryptedSyncState::Tombstone {
                    delete_hlc: "11:0:device".into(),
                },
                created_at: 11,
            })
            .unwrap();
        store
            .put_record_state(
                SyncCollection::Lists,
                fixtures.canonical.id,
                LocalSyncRecordState {
                    current_revision_hlc: Some("10:0:device".into()),
                    state: LocalSyncSemanticState::Live {
                        mutation_hlc: "10:0:device".into(),
                        plaintext_json: "{\"kind\":\"list\"}".into(),
                    },
                },
                10,
            )
            .unwrap();
        store
            .put_record_state(
                SyncCollection::Tasks,
                fixtures.task.id,
                LocalSyncRecordState {
                    current_revision_hlc: Some("11:0:device".into()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: "11:0:device".into(),
                    },
                },
                11,
            )
            .unwrap();
        store.set_cursor("adapter_cursor", 42, 12).unwrap();
        store
            .put_quarantine(LocalSyncQuarantineEntry {
                record_id: fixtures.live_quarantine_id,
                collection: SyncCollection::Templates,
                seq: 12,
                revision_hlc: "12:0:device".into(),
                state: EncryptedSyncState::Live {
                    mutation_hlc: "12:0:device".into(),
                    blob: vec![4, 5, 6],
                },
                reason: PullFailureReason::InvalidPlaintext,
                required_list_id: None,
                first_failed_at: 12,
                last_failed_at: 12,
                attempt_count: 1,
            })
            .unwrap();
        store
            .put_quarantine(LocalSyncQuarantineEntry {
                record_id: fixtures.tombstone_quarantine_id,
                collection: SyncCollection::TaskSeries,
                seq: 13,
                revision_hlc: "13:0:device".into(),
                state: EncryptedSyncState::Tombstone {
                    delete_hlc: "13:0:device".into(),
                },
                reason: PullFailureReason::InvalidPlaintext,
                required_list_id: None,
                first_failed_at: 13,
                last_failed_at: 13,
                attempt_count: 1,
            })
            .unwrap();
    }

    fn adapter_snapshot<S: LocalSyncStore>(
        store: &mut S,
        fixtures: &AdapterFixtures,
    ) -> AdapterSnapshot {
        AdapterSnapshot {
            setting: store.get_setting("adapter_contract").unwrap(),
            outbox: store.list_outbox_heads(10).unwrap(),
            live_record: store
                .get_record_state(SyncCollection::Lists, fixtures.canonical.id)
                .unwrap(),
            tombstone_record: store
                .get_record_state(SyncCollection::Tasks, fixtures.task.id)
                .unwrap(),
            cursor: store.get_cursor_seq("adapter_cursor").unwrap(),
            quarantine: store.list_quarantine(10).unwrap(),
            aliases: store.list_list_aliases().unwrap(),
            resolved_alias: store.resolve_list_alias(fixtures.alias.id).unwrap(),
            list: store.get_list(fixtures.canonical.id).unwrap(),
            task: store.get_task(fixtures.task.id).unwrap(),
            tasks: store
                .list_tasks_by_list_for_sync(fixtures.canonical.id)
                .unwrap(),
            template: store.get_template(fixtures.template.id).unwrap(),
            series: store.get_series(fixtures.series.id).unwrap(),
            timer: store.get_timer_session(fixtures.timer.id).unwrap(),
            timers: store.list_timer_sessions_by_task(fixtures.task.id).unwrap(),
        }
    }

    #[test]
    fn direct_and_committed_transaction_adapters_persist_equivalent_contracts() {
        let direct_temp = tempdir().unwrap();
        let transaction_temp = tempdir().unwrap();
        let fixtures = AdapterFixtures::new();
        let mut direct = SqliteSyncStore::new(direct_temp.path().join("direct.sqlite3"), DB_KEY);
        let mut transactional =
            SqliteSyncStore::new(transaction_temp.path().join("transaction.sqlite3"), DB_KEY);

        seed_adapter_contract(&mut direct, &fixtures);
        let direct_snapshot = adapter_snapshot(&mut direct, &fixtures);

        let mut transaction = transactional.begin_write_transaction().unwrap();
        seed_adapter_contract(&mut transaction, &fixtures);
        transaction.commit().unwrap();
        let transaction_snapshot = adapter_snapshot(&mut transactional, &fixtures);

        assert_eq!(transaction_snapshot, direct_snapshot);
        assert!(matches!(
            direct_snapshot.outbox[0].state,
            EncryptedSyncState::Live { .. }
        ));
        assert!(matches!(
            direct_snapshot.outbox[1].state,
            EncryptedSyncState::Tombstone { .. }
        ));
        assert!(matches!(
            direct_snapshot.quarantine[0].state,
            EncryptedSyncState::Live { .. }
        ));
        assert!(matches!(
            direct_snapshot.quarantine[1].state,
            EncryptedSyncState::Tombstone { .. }
        ));
    }

    #[test]
    fn transaction_adapter_drop_rolls_back_all_contract_categories() {
        let temp = tempdir().unwrap();
        let fixtures = AdapterFixtures::new();
        let mut store = SqliteSyncStore::new(temp.path().join("rollback.sqlite3"), DB_KEY);
        {
            let mut transaction = store.begin_write_transaction().unwrap();
            seed_adapter_contract(&mut transaction, &fixtures);
        }

        assert_eq!(store.get_setting("adapter_contract").unwrap(), None);
        assert!(store.list_outbox_heads(10).unwrap().is_empty());
        assert_eq!(
            store
                .get_record_state(SyncCollection::Lists, fixtures.canonical.id)
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .get_record_state(SyncCollection::Tasks, fixtures.task.id)
                .unwrap(),
            None
        );
        assert_eq!(store.get_cursor_seq("adapter_cursor").unwrap(), None);
        assert!(store.list_quarantine(10).unwrap().is_empty());
        assert!(store.list_list_aliases().unwrap().is_empty());
        assert_eq!(
            store.resolve_list_alias(fixtures.alias.id).unwrap(),
            fixtures.alias.id
        );
        assert_eq!(store.get_list(fixtures.canonical.id).unwrap(), None);
        assert_eq!(store.get_task(fixtures.task.id).unwrap(), None);
        assert!(store
            .list_tasks_by_list_for_sync(fixtures.canonical.id)
            .unwrap()
            .is_empty());
        assert_eq!(store.get_template(fixtures.template.id).unwrap(), None);
        assert_eq!(store.get_series(fixtures.series.id).unwrap(), None);
        assert_eq!(store.get_timer_session(fixtures.timer.id).unwrap(), None);
        assert!(store
            .list_timer_sessions_by_task(fixtures.task.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn canonical_inbox_contracts_are_available_on_store_and_transaction_adapters() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("aliases.sqlite3");
        let canonical = new_list("Canonical".into(), "a0".into(), 1).unwrap();
        let alias = new_list("Alias".into(), "a1".into(), 1).unwrap();
        let task = new_task(alias.id, None, "late".into(), "a0".into(), 1).unwrap();
        let mut lists = SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
        lists.insert(canonical.clone()).unwrap();
        lists.insert(alias.clone()).unwrap();
        drop(lists);
        SqliteTaskRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .insert(task.clone())
            .unwrap();

        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        store
            .put_record_state(
                SyncCollection::Lists,
                canonical.id,
                LocalSyncRecordState {
                    current_revision_hlc: Some("1:0:device".into()),
                    state: LocalSyncSemanticState::Live {
                        mutation_hlc: "1:0:device".into(),
                        plaintext_json: "{}".into(),
                    },
                },
                1,
            )
            .unwrap();
        store
            .put_quarantine(LocalSyncQuarantineEntry {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Lists,
                seq: 1,
                revision_hlc: "1:0:device".into(),
                state: EncryptedSyncState::Live {
                    mutation_hlc: "1:0:device".into(),
                    blob: vec![1],
                },
                reason: PullFailureReason::InvalidPlaintext,
                required_list_id: None,
                first_failed_at: 1,
                last_failed_at: 1,
                attempt_count: 1,
            })
            .unwrap();
        store.materialize_canonical_list(canonical.id).unwrap();
        store
            .replace_list_aliases(
                &[LocalListAlias {
                    alias_list_id: alias.id,
                    canonical_list_id: canonical.id,
                }],
                2,
            )
            .unwrap();

        assert_eq!(
            store.list_record_states(SyncCollection::Lists).unwrap()[0].0,
            canonical.id
        );
        assert!(store.has_live_quarantine(SyncCollection::Lists).unwrap());
        assert_eq!(store.resolve_list_alias(alias.id).unwrap(), canonical.id);
        assert_eq!(store.list_list_aliases().unwrap().len(), 1);
        assert_eq!(store.list_all_tasks_for_sync().unwrap(), vec![task]);

        let mut transaction = store.begin_write_transaction().unwrap();
        assert_eq!(
            transaction.resolve_list_alias(alias.id).unwrap(),
            canonical.id
        );
        assert_eq!(transaction.list_list_aliases().unwrap().len(), 1);
        assert_eq!(
            transaction
                .list_record_states(SyncCollection::Lists)
                .unwrap()
                .len(),
            1
        );
        assert!(transaction
            .has_live_quarantine(SyncCollection::Lists)
            .unwrap());
        assert_eq!(transaction.list_all_tasks_for_sync().unwrap().len(), 1);
        transaction.replace_list_aliases(&[], 3).unwrap();
        transaction.commit().unwrap();
        assert!(store.list_list_aliases().unwrap().is_empty());
        assert_eq!(store.resolve_list_alias(alias.id).unwrap(), alias.id);
    }

    #[test]
    fn transactional_seed_rolls_back_and_committed_seed_survives_absence_sweep() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let list = new_list(
            "Local".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            1,
        )
        .unwrap();
        let tenant_id = Uuid::now_v7();
        let mut crypto = OwnedSqliteWriteTx::begin(open_encrypted(&db_path, &DB_KEY).unwrap())
            .expect("begin local crypto seed");
        crypto
            .bind_tenant_roots(
                LocalProfileBinding {
                    tenant_id,
                    user_id: Uuid::now_v7(),
                    device_id: Uuid::now_v7(),
                    bound_at: 1,
                    updated_at: 1,
                },
                &[LocalTenantRootKeyBundle {
                    tenant_id,
                    generation: 1,
                    wrapped_tenant_root_dek: vec![2],
                    updated_at: 1,
                }],
            )
            .unwrap();
        crypto.commit().unwrap();
        let mut repository = SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
        repository.insert(list.clone()).unwrap();
        drop(repository);

        let keys = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some([0x33; 32].into()),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        };
        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        let mut now = || Ok(10);
        {
            let mut transaction = store.begin_write_transaction().unwrap();
            enqueue_backfill(
                &mut transaction,
                &keys,
                "device",
                taskveil_sync::BackfillRecords {
                    lists: std::slice::from_ref(&list),
                    templates: &[],
                    task_series: &[],
                    tasks: &[],
                    timer_sessions: &[],
                },
                &mut now,
            )
            .unwrap();
            // Simulate a crash before the seed generation commits.
        }
        assert!(store.list_outbox_heads(10).unwrap().is_empty());

        let mut transaction = store.begin_write_transaction().unwrap();
        enqueue_backfill(
            &mut transaction,
            &keys,
            "device",
            taskveil_sync::BackfillRecords {
                lists: std::slice::from_ref(&list),
                templates: &[],
                task_series: &[],
                tasks: &[],
                timer_sessions: &[],
            },
            &mut now,
        )
        .unwrap();
        transaction.set_cursor("initial_backfill", 1, 11).unwrap();
        transaction.commit().unwrap();
        assert_eq!(store.list_outbox_heads(10).unwrap().len(), 1);

        let generation_id = Uuid::now_v7();
        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .start_full_resync(generation_id, 1, 0, 20)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 21)
            .unwrap();
        transaction
            .enter_full_resync_sweep(generation_id, 0, 22)
            .unwrap();
        transaction.commit().unwrap();

        loop {
            let mut transaction = store.begin_write_transaction().unwrap();
            let swept = transaction
                .sweep_full_resync_batch(generation_id, 1, 23)
                .unwrap();
            transaction.commit().unwrap();
            if swept.scanned_records == 0 {
                break;
            }
        }
        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .finalize_full_resync(generation_id, SYNC_CURSOR_NAME, 24)
            .unwrap();
        transaction.commit().unwrap();

        assert_eq!(store.list_outbox_heads(10).unwrap().len(), 1);
        assert!(store.get_list(list.id).unwrap().is_some());
        assert_eq!(store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(), Some(0));
    }

    #[test]
    fn durable_completion_token_exposes_awaiting_ack_until_atomically_cleared() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut store = SqliteSyncStore::new(db_path, DB_KEY);
        let generation_id = Uuid::now_v7();
        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .start_full_resync(generation_id, 7, 11, 100)
            .unwrap();
        transaction
            .advance_full_resync_base(generation_id, None, true, 101)
            .unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
                "completion-token",
                101,
            )
            .unwrap();
        transaction.commit().unwrap();

        assert_eq!(
            store.load_full_resync().unwrap().unwrap().phase,
            taskveil_sync::enqueue::LocalFullResyncPhase::BaseAwaitingAck
        );

        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
                "",
                102,
            )
            .unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            store.load_full_resync().unwrap().unwrap().phase,
            taskveil_sync::enqueue::LocalFullResyncPhase::Delta
        );
    }

    #[test]
    fn invalid_resync_restart_atomically_clears_progress_marks_and_both_tokens() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("profile.sqlite3");
        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        let generation_id = Uuid::now_v7();
        let mut transaction = store.begin_write_transaction().unwrap();
        transaction
            .start_full_resync(generation_id, 7, 11, 100)
            .unwrap();
        transaction
            .mark_full_resync_record(generation_id, SyncCollection::Tasks, Uuid::now_v7())
            .unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
                "page-token",
                100,
            )
            .unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
                "completion-token",
                100,
            )
            .unwrap();
        transaction.commit().unwrap();
        open_encrypted(&db_path, &DB_KEY)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_resync_reset_token
                 BEFORE UPDATE ON internal_metadata
                 WHEN NEW.key = 'sync_full_resync_completion_token' AND NEW.value = ''
                 BEGIN SELECT RAISE(ABORT, 'fail reset token'); END;",
            )
            .unwrap();

        let mut transaction = store.begin_write_transaction().unwrap();
        transaction.reset_full_resync().unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
                "",
                101,
            )
            .unwrap();
        assert!(transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
                "",
                101,
            )
            .is_err());
        drop(transaction);
        assert!(store.load_full_resync().unwrap().is_some());
        assert_eq!(
            store
                .get_setting(taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY)
                .unwrap()
                .as_deref(),
            Some("page-token")
        );
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM sync_full_resync_marks", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        connection
            .execute_batch("DROP TRIGGER fail_resync_reset_token;")
            .unwrap();

        let mut transaction = store.begin_write_transaction().unwrap();
        transaction.reset_full_resync().unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
                "",
                102,
            )
            .unwrap();
        transaction
            .set_setting(
                taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY,
                "",
                102,
            )
            .unwrap();
        transaction.commit().unwrap();
        assert!(store.load_full_resync().unwrap().is_none());
        assert_eq!(
            open_encrypted(&db_path, &DB_KEY)
                .unwrap()
                .query_row("SELECT count(*) FROM sync_full_resync_marks", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }
}
