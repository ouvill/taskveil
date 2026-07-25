mod convert;

use std::path::{Path, PathBuf};

use convert::{
    local_cursor_to_storage, local_outbox_to_storage, local_quarantine_to_storage,
    local_record_to_storage, storage_alias_to_local, storage_outbox_to_local,
    storage_quarantine_to_local, storage_record_to_local, storage_resync_to_local,
    storage_sweep_to_local,
};
use taskveil_domain::{CompletedTimerSession, List, Task, TaskSeries, TaskTemplate, Uuid};
use taskveil_storage::{
    open_encrypted, ListRepository, OwnedSqliteWriteTx, SettingsRepository, SqliteListRepository,
    SqliteSettingsRepository, SqliteSyncStateRepository, SqliteTaskRepository,
    SqliteTemplateSeriesRepository, SqliteTimerSessionRepository, StorageError,
    SyncStateRepository, TaskRepository, TemplateSeriesRepository, TimerSessionRepository,
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
}

pub struct SqliteSyncWriteTx {
    transaction: OwnedSqliteWriteTx,
}

impl SqliteSyncStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(db_path: PathBuf, db_key: [u8; 32]) -> Self {
        Self::new_secret(db_path, Zeroizing::new(db_key))
    }

    pub(crate) fn new_secret(db_path: PathBuf, db_key: Zeroizing<[u8; 32]>) -> Self {
        Self { db_path, db_key }
    }
}

impl LocalSyncAtomicStore for SqliteSyncStore {
    type WriteTransaction = SqliteSyncWriteTx;

    fn begin_write_transaction(&mut self) -> Result<Self::WriteTransaction, String> {
        let connection =
            open_encrypted(&self.db_path, &self.db_key).map_err(|error| error.to_string())?;
        let transaction =
            OwnedSqliteWriteTx::begin(connection).map_err(|error| error.to_string())?;
        Ok(SqliteSyncWriteTx { transaction })
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
                .map_err(|error| error.to_string())
        })
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        with_settings_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_setting(key)
                .map_err(|error| error.to_string())
        })
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String> {
        with_settings_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .set_setting(key, value, updated_at)
                .map_err(|error| error.to_string())
        })
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .put_outbox_head(local_outbox_to_storage(entry))
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
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
                .map_err(|error| error.to_string())
        })
    }

    fn put_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
        state: LocalSyncRecordState,
        updated_at: i64,
    ) -> Result<(), String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .put_record_state(local_record_to_storage(
                    collection, record_id, state, updated_at,
                ))
                .map_err(|error| error.to_string())
        })
    }
}

impl LocalSyncStore for SqliteSyncStore {
    fn load_full_resync(&mut self) -> Result<Option<LocalFullResyncProgress>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .load_full_resync()
                .map(|progress| progress.map(storage_resync_to_local))
                .map_err(|error| error.to_string())
        })
    }

    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_outbox_heads(limit)
                .map_err(|error| error.to_string())
        })?
        .into_iter()
        .map(storage_outbox_to_local)
        .collect()
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .ack_outbox_op(op_id)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_outbox_head(collection.as_str(), record_id)
                .map_err(|error| error.to_string())
        })
    }

    fn get_cursor_seq(&mut self, name: &str) -> Result<Option<i64>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_cursor(name)
                .map(|cursor| cursor.map(|cursor| cursor.seq))
                .map_err(|error| error.to_string())
        })
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .set_cursor(name, seq, updated_at)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_cursor(name)
                .map_err(|error| error.to_string())
        })
    }

    fn put_quarantine(&mut self, entry: LocalSyncQuarantineEntry) -> Result<(), String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .put_quarantine(local_quarantine_to_storage(entry))
                .map_err(|e| e.to_string())
        })
    }

    fn list_quarantine(&mut self, limit: usize) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository.list_quarantine(limit).map_err(|e| e.to_string())
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
                .map_err(|e| e.to_string())
        })?
        .into_iter()
        .map(storage_quarantine_to_local)
        .collect()
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_quarantine(record_id)
                .map_err(|e| e.to_string())
        })
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
                .map_err(|error| error.to_string())
        })
    }

    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .has_live_quarantine(collection.as_str())
                .map_err(|error| error.to_string())
        })
    }

    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_list_aliases()
                .map(|aliases| aliases.into_iter().map(storage_alias_to_local).collect())
                .map_err(|error| error.to_string())
        })
    }

    fn replace_list_aliases(
        &mut self,
        aliases: &[LocalListAlias],
        updated_at: i64,
    ) -> Result<(), String> {
        let connection =
            open_encrypted(&self.db_path, &self.db_key).map_err(|error| error.to_string())?;
        let mut transaction =
            OwnedSqliteWriteTx::begin(connection).map_err(|error| error.to_string())?;
        replace_list_aliases_in_transaction(&mut transaction, aliases, updated_at)?;
        transaction
            .commit()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String> {
        with_sync_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .resolve_list_alias(list_id)
                .map_err(|error| error.to_string())
        })
    }

    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
        let connection =
            open_encrypted(&self.db_path, &self.db_key).map_err(|error| error.to_string())?;
        let mut transaction =
            OwnedSqliteWriteTx::begin(connection).map_err(|error| error.to_string())?;
        transaction
            .materialize_canonical_list(canonical_list_id)
            .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .get_default()
                .map(|list| list.map(|list| list.id))
                .map_err(|error| error.to_string())
        })
    }

    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get(id) {
                Ok(list) => Ok(Some(list)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .upsert_for_sync(list)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String> {
        with_list_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_and_rehome_tasks_for_sync(list_id)
                .map_err(|error| error.to_string())
        })
    }

    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get(id) {
                Ok(task) => Ok(Some(task)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
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
                .map_err(|error| error.to_string())
        })
    }

    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_all_for_sync()
                .map_err(|error| error.to_string())
        })
    }

    fn list_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<Vec<Task>, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_subtree_for_sync(task_id)
                .map_err(|error| error.to_string())
        })
    }

    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .upsert_for_sync(task)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, String> {
        with_task_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_subtree_for_sync(task_id)
                .map_err(|error| error.to_string())
        })
    }

    fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_template(id) {
                Ok(template) => Ok(Some(template)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .upsert_template_for_sync(template)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_template(id)
                .map_err(|error| error.to_string())
        })
    }

    fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_series(id) {
                Ok(schedule) => Ok(Some(schedule)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn upsert_series_for_sync(&mut self, schedule: TaskSeries) -> Result<(), String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .upsert_series_for_sync(schedule)
                .map_err(|error| error.to_string())
        })
    }

    fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        with_recurrence_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_series(id)
                .map_err(|error| error.to_string())
        })
    }

    fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            match repository.get_completed(id) {
                Ok(session) => Ok(Some(session)),
                Err(StorageError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    fn upsert_timer_session_for_sync(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<(), String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .insert_completed(session)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .delete_completed(id)
                .map_err(|error| error.to_string())
        })
    }

    fn list_timer_sessions_by_task(
        &mut self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .list_completed_by_task(task_id)
                .map_err(|error| error.to_string())
        })
    }

    fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
        with_timer_repository(&self.db_path, &self.db_key, |repository| {
            repository
                .clear_active_for_task(task_id)
                .map_err(|error| error.to_string())
        })
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
            .map_err(|error| error.to_string())
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        self.transaction
            .get_setting(key)
            .map_err(|error| error.to_string())
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), String> {
        self.transaction
            .set_setting(key, value, updated_at)
            .map_err(|error| error.to_string())
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        self.transaction
            .put_outbox_head(local_outbox_to_storage(entry))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String> {
        self.transaction
            .get_record_state(collection.as_str(), record_id)
            .map(|state| state.map(storage_record_to_local))
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
    }
}

impl LocalSyncStore for SqliteSyncWriteTx {
    fn load_full_resync(&mut self) -> Result<Option<LocalFullResyncProgress>, String> {
        self.transaction
            .load_full_resync()
            .map(|progress| progress.map(storage_resync_to_local))
            .map_err(|error| error.to_string())
    }

    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        self.transaction
            .list_outbox_heads(limit)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(storage_outbox_to_local)
            .collect()
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
        self.transaction
            .ack_outbox_op(op_id)
            .map_err(|error| error.to_string())
    }

    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        self.transaction
            .delete_outbox_head(collection.as_str(), record_id)
            .map_err(|error| error.to_string())
    }

    fn get_cursor_seq(&mut self, name: &str) -> Result<Option<i64>, String> {
        self.transaction
            .get_cursor(name)
            .map(|cursor| cursor.map(|cursor| cursor.seq))
            .map_err(|error| error.to_string())
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), String> {
        self.transaction
            .set_cursor(name, seq, updated_at)
            .map_err(|error| error.to_string())
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), String> {
        self.transaction
            .delete_cursor(name)
            .map_err(|error| error.to_string())
    }

    fn put_quarantine(&mut self, entry: LocalSyncQuarantineEntry) -> Result<(), String> {
        self.transaction
            .put_quarantine(local_quarantine_to_storage(entry))
            .map_err(|e| e.to_string())
    }

    fn list_quarantine(&mut self, limit: usize) -> Result<Vec<LocalSyncQuarantineEntry>, String> {
        self.transaction
            .list_quarantine(limit)
            .map_err(|e| e.to_string())?
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
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(storage_quarantine_to_local)
            .collect()
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_quarantine(record_id)
            .map_err(|e| e.to_string())
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
            .map_err(|error| error.to_string())
    }

    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
        self.transaction
            .has_live_quarantine(collection.as_str())
            .map_err(|error| error.to_string())
    }

    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
        self.transaction
            .list_list_aliases()
            .map(|aliases| aliases.into_iter().map(storage_alias_to_local).collect())
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
    }

    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
        self.transaction
            .materialize_canonical_list(canonical_list_id)
            .map_err(|error| error.to_string())
    }

    fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
        self.transaction
            .default_list_id()
            .map_err(|error| error.to_string())
    }

    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
        self.transaction
            .get_list(id)
            .map_err(|error| error.to_string())
    }

    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
        self.transaction
            .upsert_list_for_sync(list)
            .map_err(|error| error.to_string())
    }

    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String> {
        self.transaction
            .delete_list_and_rehome_tasks_for_sync(list_id)
            .map_err(|error| error.to_string())
    }

    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
        self.transaction
            .get_task(id)
            .map_err(|error| error.to_string())
    }

    fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String> {
        self.transaction
            .list_tasks_by_list(list_id)
            .map_err(|error| error.to_string())
    }

    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
        self.transaction
            .list_all_tasks_for_sync()
            .map_err(|error| error.to_string())
    }

    fn list_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<Vec<Task>, String> {
        self.transaction
            .list_task_subtree_for_sync(task_id)
            .map_err(|error| error.to_string())
    }

    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
        self.transaction
            .upsert_task_for_sync(task)
            .map_err(|error| error.to_string())
    }

    fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, String> {
        self.transaction
            .delete_task_subtree_for_sync(task_id)
            .map_err(|error| error.to_string())
    }

    fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
        match self.transaction.get_template(id) {
            Ok(template) => Ok(Some(template)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
        self.transaction
            .upsert_template(template)
            .map_err(|error| error.to_string())
    }

    fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_template(id)
            .map_err(|error| error.to_string())
    }

    fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
        match self.transaction.get_series(id) {
            Ok(schedule) => Ok(Some(schedule)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn upsert_series_for_sync(&mut self, schedule: TaskSeries) -> Result<(), String> {
        self.transaction
            .upsert_series(schedule)
            .map_err(|error| error.to_string())
    }

    fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_series(id)
            .map_err(|error| error.to_string())
    }

    fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        match self.transaction.get_timer_session(id) {
            Ok(session) => Ok(Some(session)),
            Err(StorageError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn upsert_timer_session_for_sync(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<(), String> {
        self.transaction
            .insert_timer_session(session)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        self.transaction
            .delete_timer_session(id)
            .map_err(|error| error.to_string())
    }

    fn list_timer_sessions_by_task(
        &mut self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        self.transaction
            .list_timer_sessions_by_task_for_sync(task_id)
            .map_err(|error| error.to_string())
    }

    fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
        self.transaction
            .clear_active_timer_for_task_for_sync(task_id)
            .map_err(|error| error.to_string())
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
            .map(storage_resync_to_local)
            .map_err(|error| error.to_string())
    }

    fn mark_full_resync_record(
        &mut self,
        generation_id: Uuid,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<(), String> {
        self.transaction
            .mark_full_resync_record(generation_id, collection.as_str(), record_id)
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
    }

    fn advance_full_resync_delta(
        &mut self,
        generation_id: Uuid,
        delta_cursor: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.transaction
            .advance_full_resync_delta(generation_id, delta_cursor, now_ms)
            .map_err(|error| error.to_string())
    }

    fn enter_full_resync_sweep(
        &mut self,
        generation_id: Uuid,
        closure_high_water: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.transaction
            .enter_full_resync_sweep(generation_id, closure_high_water, now_ms)
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
    }

    fn finalize_full_resync(
        &mut self,
        generation_id: Uuid,
        cursor_name: &str,
        now_ms: i64,
    ) -> Result<i64, String> {
        self.transaction
            .finalize_full_resync(generation_id, cursor_name, now_ms)
            .map_err(|error| error.to_string())
    }

    fn commit(self) -> Result<(), String> {
        self.transaction
            .commit()
            .map(|_| ())
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())?;
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
        .map_err(|error| error.to_string())
}

fn with_sync_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteSyncStateRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteSyncStateRepository::new(connection);
    f(&mut repository)
}

fn with_settings_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteSettingsRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteSettingsRepository::new(connection);
    f(&mut repository)
}

fn with_task_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTaskRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteTaskRepository::new(connection);
    f(&mut repository)
}

fn with_recurrence_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTemplateSeriesRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteTemplateSeriesRepository::new(connection);
    f(&mut repository)
}

fn with_timer_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteTimerSessionRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteTimerSessionRepository::new(connection);
    f(&mut repository)
}

fn with_list_repository<T>(
    db_path: &Path,
    db_key: &[u8; 32],
    f: impl FnOnce(&mut SqliteListRepository) -> Result<T, String>,
) -> Result<T, String> {
    let connection = open_encrypted(db_path, db_key).map_err(|error| error.to_string())?;
    let mut repository = SqliteListRepository::new(connection);
    f(&mut repository)
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskveil_domain::{
        new_list, new_task, CompletedTimerSession, SeriesCursor, TaskBlueprint, TaskBlueprintNode,
        TaskContent, TaskSeriesConfig, TimerFinishKind, TimerMode, TASK_BLUEPRINT_SCHEMA_REVISION,
    };
    use taskveil_storage::{
        ListRepository, LocalCryptoRepository, LocalProfileBinding, LocalTenantRootKeyBundle,
        SqliteLocalCryptoRepository,
    };
    use taskveil_sync::{
        enqueue_backfill, EncryptedSyncState, LocalSyncKeys, LocalSyncSemanticState,
        PullFailureReason, SYNC_CURSOR_NAME,
    };
    use tempfile::tempdir;

    const DB_KEY: [u8; 32] = [0x51; 32];

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
        let mut crypto =
            SqliteLocalCryptoRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
        crypto
            .bind_tenant_root(
                LocalProfileBinding {
                    tenant_id,
                    user_id: Uuid::now_v7(),
                    device_id: Uuid::now_v7(),
                    bound_at: 1,
                    updated_at: 1,
                },
                &LocalTenantRootKeyBundle {
                    tenant_id,
                    generation: 1,
                    wrapped_tenant_root_dek: vec![2],
                    updated_at: 1,
                },
            )
            .unwrap();
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
}
