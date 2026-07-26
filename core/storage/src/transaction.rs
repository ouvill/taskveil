use crate::local_crypto_repository::{
    bind_tenant_roots_on, load_local_profile_binding_on, load_local_tenant_roots_on,
};
use crate::profile_coordination::{
    assert_runtime_epoch_on, assert_sync_lease_on, bump_runtime_epoch_on,
};
use crate::reminder_repository::{
    clear_task_reminders_on, create_task_reminder_on, delete_reminder_on, snooze_reminder_on,
    update_reminder_on,
};
use crate::*;

/// A short-lived SQLite write transaction shared by domain and sync-state writes.
///
/// The transaction starts with [`TransactionBehavior::Immediate`] so concurrent
/// desktop frontends serialize before reading and incrementing the local HLC.
/// Dropping this value without calling [`Self::commit`] rolls back every write.
pub struct SqliteWriteTx<'connection> {
    transaction: Transaction<'connection>,
}

impl<'connection> SqliteWriteTx<'connection> {
    pub fn begin(connection: &'connection mut Connection) -> Result<Self, StorageError> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(Self { transaction })
    }

    pub fn get_task(&self, id: Uuid) -> Result<Task, StorageError> {
        get_task_on(&self.transaction, id)
    }

    pub fn get_list(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(&self.transaction, self.resolve_list_alias(id)?)
    }

    pub fn default_list_id(&self) -> Result<Option<Uuid>, StorageError> {
        get_default_list_on(&self.transaction).map(|list| list.map(|list| list.id))
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(&self.transaction, list_id)
    }

    pub fn list_active_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(&self.transaction, self.resolve_list_alias(list_id)?)
    }

    /// Snapshots every task rooted at `task_id` inside this write transaction.
    ///
    /// Callers can use the returned domain records to prepare per-record sync
    /// tombstones before deleting the same subtree and committing atomically.
    pub fn list_task_subtree(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(&self.transaction, task_id)
    }

    /// Snapshots every task stored in `list_id` inside this write transaction.
    ///
    /// This intentionally includes records regardless of `deleted_at`: a
    /// physical list deletion must account for every persisted task record.
    pub fn list_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(&self.transaction, self.resolve_list_alias(list_id)?)
    }

    pub fn list_all_tasks_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(&self.transaction)
    }

    pub fn get_timer_session(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(&self.transaction, id)
    }

    pub fn start_active_timer_session(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        start_active_timer_session_on(&self.transaction, session, updated_at)
    }

    pub fn update_active_timer_session(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        update_active_timer_session_on(&self.transaction, session, updated_at)
    }

    pub fn clear_active_timer_session(
        &mut self,
        expected_session_id: Uuid,
    ) -> Result<bool, StorageError> {
        Ok(self.transaction.execute(
            "DELETE FROM active_timer_session WHERE singleton = 1 AND session_id = ?1",
            [expected_session_id.to_string()],
        )? == 1)
    }

    pub fn list_timer_sessions_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.transaction,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    pub fn insert_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(&self.transaction, session)
    }

    pub fn finish_active_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        let active = self
            .transaction
            .query_row(
                "SELECT session_id, task_id, mode, phase, state, started_at,
                        last_resumed_at, accumulated_active_ms, target_duration_ms
                 FROM active_timer_session WHERE singleton = 1",
                [],
                row_to_active_timer_session,
            )
            .optional()?
            .transpose()?
            .ok_or_else(|| {
                StorageError::IncompatibleSchema("active timer session is missing".to_string())
            })?;
        if active.session_id != session.id
            || active.task_id != Some(session.task_id)
            || active.mode != session.mode
            || active.phase != TimerPhase::Work
            || active.started_at != session.started_at
        {
            return Err(StorageError::IncompatibleSchema(
                "completed timer session does not match active work".to_string(),
            ));
        }
        let expected_duration = restored_active_duration_ms(&active, session.ended_at)
            .map_err(StorageError::InvalidActiveTimerUpdate)?;
        if session.active_duration_ms != expected_duration {
            return Err(StorageError::CompletedTimerDurationMismatch {
                expected_ms: expected_duration,
                actual_ms: session.active_duration_ms,
            });
        }
        let inserted = insert_completed_timer_session_on(&self.transaction, session)?;
        self.transaction
            .execute("DELETE FROM active_timer_session WHERE singleton = 1", [])?;
        Ok(inserted)
    }

    pub fn delete_timer_session(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .transaction
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }

    pub fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.transaction.execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    pub fn list_lists_including_archived(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.transaction.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE NOT EXISTS (
                 SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
             )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(lists)
    }

    pub fn insert_task(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = self.resolve_list_alias(task.list_id)?;
        insert_task_on(&self.transaction, &task)
    }

    pub fn insert_list(&mut self, list: List) -> Result<(), StorageError> {
        insert_list_on(&self.transaction, &list)
    }

    pub fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(&self.transaction, id)
    }

    pub fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(&self.transaction)
    }

    pub fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.transaction, &template)
    }

    pub fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(&self.transaction, id)
    }

    pub fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(&self.transaction, id)
    }

    pub fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(&self.transaction)
    }

    pub fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(&self.transaction, now_ms)
    }

    pub fn upsert_series(&mut self, schedule: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.transaction, &schedule)
    }

    pub fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(&self.transaction, id)
    }

    pub fn update_task(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = self.resolve_list_alias(task.list_id)?;
        update_task_on(&self.transaction, &task)
    }

    pub fn update_list(&mut self, list: List) -> Result<(), StorageError> {
        update_list_on(&self.transaction, &list)
    }

    /// Physically deletes a task and all descendants in this write transaction.
    ///
    /// Related reminders and undo entries are removed by the same operation.
    /// Dropping the transaction without committing rolls every deletion back.
    pub fn delete_task_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        self.get_task(task_id)?;
        delete_task_subtree_on(&self.transaction, task_id)
    }

    /// Physically deletes a non-default List and moves its Tasks to the
    /// Tenant's default Inbox in this write transaction.
    ///
    /// Task history, reminders, and timer relations remain intact. Dropping the
    /// transaction without committing rolls every placement change back.
    pub fn delete_list_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = self.resolve_list_alias(list_id)?;
        let list = get_list_on(&self.transaction, list_id)?;
        if list.is_default {
            return Err(StorageError::DefaultListProtected {
                operation: "deleted",
                list_id,
            });
        }
        delete_list_and_rehome_tasks_for_sync_on(&self.transaction, list_id)
    }

    pub fn update_task_with_undo(
        &mut self,
        mut before: Task,
        mut after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        before.list_id = self.resolve_list_alias(before.list_id)?;
        after.list_id = self.resolve_list_alias(after.list_id)?;
        update_task_with_undo_on(&self.transaction, before, after, operation_type, created_at)
    }

    pub fn update_with_undo(
        &mut self,
        before: Task,
        after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        self.update_task_with_undo(before, after, operation_type, created_at)
    }

    pub fn undo_task_operation(
        &mut self,
        undo_id: Uuid,
        consumed_at: i64,
    ) -> Result<Task, StorageError> {
        undo_task_operation_on(&self.transaction, undo_id, consumed_at)
    }

    pub fn get_app_setting(&self, key: AppSettingKey) -> Result<Option<String>, StorageError> {
        get_app_setting_on(&self.transaction, key)
    }

    pub fn set_app_setting(
        &mut self,
        key: AppSettingKey,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_app_setting_on(&self.transaction, key, value, updated_at)
    }

    pub fn get_internal_metadata(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_internal_metadata_on(&self.transaction, key)
    }

    pub fn set_internal_metadata(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_internal_metadata_on(&self.transaction, key, value, updated_at)
    }

    pub fn create_task_reminder(
        &mut self,
        task_id: Uuid,
        remind_at: i64,
        created_at: i64,
    ) -> Result<Reminder, StorageError> {
        create_task_reminder_on(&self.transaction, task_id, remind_at, created_at)
    }

    pub fn update_reminder(
        &mut self,
        reminder_id: Uuid,
        remind_at: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        update_reminder_on(&self.transaction, reminder_id, remind_at, updated_at)
    }

    pub fn delete_reminder(&mut self, reminder_id: Uuid) -> Result<Reminder, StorageError> {
        delete_reminder_on(&self.transaction, reminder_id)
    }

    pub fn clear_task_reminders(&mut self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        clear_task_reminders_on(&self.transaction, task_id)
    }

    pub fn snooze_reminder(
        &mut self,
        reminder_id: Uuid,
        snoozed_until: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        snooze_reminder_on(&self.transaction, reminder_id, snoozed_until, updated_at)
    }

    pub fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(&self.transaction, entry)
    }

    pub fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(&self.transaction, limit)
    }

    pub fn list_all_outbox_heads(
        &self,
        limit: usize,
    ) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_all_outbox_heads_on(&self.transaction, limit)
    }

    pub fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(&self.transaction, collection, record_id)
    }

    pub fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(&self.transaction, op_id)
    }

    pub fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(&self.transaction, collection, record_id)
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.transaction, collection, record_id)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.transaction, state)
    }

    pub fn commit(self) -> Result<(), StorageError> {
        self.transaction.commit().map_err(StorageError::from)
    }

    pub fn assert_profile_runtime_epoch(&self, expected: i64) -> Result<(), StorageError> {
        assert_runtime_epoch_on(&self.transaction, expected)
    }

    pub fn assert_sync_lease(&self, lease: &SyncLease, now_ms: i64) -> Result<(), StorageError> {
        assert_sync_lease_on(&self.transaction, lease, now_ms)
    }

    pub fn bump_runtime_epoch(&mut self, now_ms: i64) -> Result<ProfileRuntimeState, StorageError> {
        bump_runtime_epoch_on(&self.transaction, now_ms)
    }
}

/// An owned `BEGIN IMMEDIATE` transaction for sync runs that must move across
/// adapter boundaries without borrowing or self-referencing a connection.
///
/// Calling [`Self::commit`] or [`Self::rollback`] returns the opened
/// connection. Dropping an unfinished value rolls every write back.
pub struct OwnedSqliteWriteTx {
    connection: Option<Connection>,
}

impl OwnedSqliteWriteTx {
    pub fn begin(connection: Connection) -> Result<Self, StorageError> {
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            connection: Some(connection),
        })
    }

    fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("active owned transaction always has a connection")
    }

    pub fn get_internal_metadata(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_internal_metadata_on(self.connection(), key)
    }

    pub fn set_internal_metadata(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_internal_metadata_on(self.connection(), key, value, updated_at)
    }

    pub fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(self.connection(), entry)
    }

    pub fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(self.connection(), limit)
    }

    pub fn list_all_outbox_heads(
        &self,
        limit: usize,
    ) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_all_outbox_heads_on(self.connection(), limit)
    }

    pub fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(self.connection(), collection, record_id)
    }

    pub fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(self.connection(), op_id)
    }

    pub fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(self.connection(), collection, record_id)
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(self.connection(), collection, record_id)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(self.connection(), state)
    }

    /// Replaces the complete generation-indexed Tenant Root cache after the
    /// caller has authenticated and compared its semantic key material.
    pub fn bind_tenant_roots(
        &mut self,
        binding: LocalProfileBinding,
        tenant_roots: &[LocalTenantRootKeyBundle],
    ) -> Result<bool, StorageError> {
        bind_tenant_roots_on(self.connection(), binding, tenant_roots)
    }

    pub fn load_local_crypto_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError> {
        load_local_profile_binding_on(self.connection())
    }

    pub fn load_tenant_roots(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<LocalTenantRootKeyBundle>, StorageError> {
        load_local_tenant_roots_on(self.connection(), tenant_id)
    }

    pub fn bump_runtime_epoch(&mut self, now_ms: i64) -> Result<ProfileRuntimeState, StorageError> {
        bump_runtime_epoch_on(self.connection(), now_ms)
    }

    pub fn assert_profile_runtime_epoch(&self, expected: i64) -> Result<(), StorageError> {
        assert_runtime_epoch_on(self.connection(), expected)
    }

    pub fn assert_sync_lease(&self, lease: &SyncLease, now_ms: i64) -> Result<(), StorageError> {
        assert_sync_lease_on(self.connection(), lease, now_ms)
    }

    pub fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError> {
        get_cursor_on(self.connection(), name)
    }

    pub fn set_cursor(
        &mut self,
        name: &str,
        seq: i64,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_cursor_on(self.connection(), name, seq, updated_at)
    }

    pub fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError> {
        delete_cursor_on(self.connection(), name)
    }

    pub fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(self.connection(), entry)
    }

    pub fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(self.connection(), limit)
    }

    pub fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(self.connection(), after, limit)
    }

    pub fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(self.connection(), record_id)
    }

    /// Lists raw durable semantic states. Callers must perform typed plaintext
    /// validation before treating live rows as canonical Inbox candidates.
    pub fn list_record_states(
        &self,
        collection: &str,
    ) -> Result<Vec<SyncRecordState>, StorageError> {
        list_record_states_on(self.connection(), collection)
    }

    /// Returns true for any unresolved live head in the requested collection,
    /// including non-replayable corruption and unsupported plaintext failures.
    pub fn has_live_quarantine(&self, collection: &str) -> Result<bool, StorageError> {
        has_live_quarantine_on(self.connection(), collection)
    }

    pub fn list_list_aliases(&self) -> Result<Vec<ListAlias>, StorageError> {
        list_list_aliases_on(self.connection())
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(self.connection(), list_id)
    }

    pub fn materialize_canonical_list(
        &mut self,
        canonical_list_id: Uuid,
    ) -> Result<(), StorageError> {
        materialize_canonical_list_on(self.connection(), canonical_list_id)
    }

    pub fn replace_list_aliases(
        &mut self,
        canonical_list_id: Uuid,
        alias_list_ids: &[Uuid],
        updated_at: i64,
    ) -> Result<(), StorageError> {
        replace_list_aliases_on(
            self.connection(),
            canonical_list_id,
            alias_list_ids,
            updated_at,
        )
    }

    pub fn load_full_resync(&self) -> Result<Option<FullResyncProgress>, StorageError> {
        load_full_resync_on(self.connection())
    }

    pub fn start_full_resync(
        &mut self,
        generation_id: Uuid,
        continuity_generation: i64,
        base_seq: i64,
        now_ms: i64,
    ) -> Result<FullResyncProgress, StorageError> {
        start_full_resync_on(
            self.connection(),
            generation_id,
            continuity_generation,
            base_seq,
            now_ms,
        )
    }

    pub fn mark_full_resync_record(
        &mut self,
        generation_id: Uuid,
        collection: &str,
        record_id: Uuid,
    ) -> Result<(), StorageError> {
        mark_full_resync_record_on(self.connection(), generation_id, collection, record_id)
    }

    pub fn advance_full_resync_base(
        &mut self,
        generation_id: Uuid,
        next_cursor: Option<&FullResyncStableCursor>,
        base_complete: bool,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        advance_full_resync_base_on(
            self.connection(),
            generation_id,
            next_cursor,
            base_complete,
            now_ms,
        )
    }

    pub fn advance_full_resync_delta(
        &mut self,
        generation_id: Uuid,
        delta_cursor: i64,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        advance_full_resync_delta_on(self.connection(), generation_id, delta_cursor, now_ms)
    }

    pub fn enter_full_resync_sweep(
        &mut self,
        generation_id: Uuid,
        closure_high_water: i64,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        enter_full_resync_sweep_on(self.connection(), generation_id, closure_high_water, now_ms)
    }

    pub fn sweep_full_resync_batch(
        &mut self,
        generation_id: Uuid,
        limit: usize,
        now_ms: i64,
    ) -> Result<FullResyncSweepSummary, StorageError> {
        sweep_full_resync_batch_on(self.connection(), generation_id, limit, now_ms)
    }

    pub fn finalize_full_resync(
        &mut self,
        generation_id: Uuid,
        cursor_name: &str,
        now_ms: i64,
    ) -> Result<i64, StorageError> {
        finalize_full_resync_on(self.connection(), generation_id, cursor_name, now_ms)
    }

    pub fn reset_full_resync(&mut self) -> Result<(), StorageError> {
        reset_full_resync_on(self.connection())
    }

    pub fn default_list_id(&self) -> Result<Option<Uuid>, StorageError> {
        get_default_list_on(self.connection()).map(|list| list.map(|list| list.id))
    }

    pub fn list_lists_including_archived(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection().prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE NOT EXISTS (
                 SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
             )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?;
        Ok(lists)
    }

    pub fn get_list(&self, id: Uuid) -> Result<Option<List>, StorageError> {
        optional_not_found(get_list_on(self.connection(), id))
    }

    pub fn upsert_list_for_sync(&mut self, list: List) -> Result<(), StorageError> {
        upsert_list_for_sync_on(self.connection(), list)
    }

    pub fn delete_list_and_rehome_tasks_for_sync(
        &mut self,
        list_id: Uuid,
    ) -> Result<usize, StorageError> {
        delete_list_and_rehome_tasks_for_sync_on(self.connection(), list_id)
    }

    pub fn get_task(&self, id: Uuid) -> Result<Option<Task>, StorageError> {
        optional_not_found(get_task_on(self.connection(), id))
    }

    pub fn list_all_tasks_by_list_for_sync(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(self.connection(), list_id)
    }

    pub fn list_all_tasks_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(self.connection())
    }

    /// Raw per-list enumeration used by existing sync and deletion adapters.
    pub fn list_tasks_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        self.list_all_tasks_by_list_for_sync(list_id)
    }

    pub fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), StorageError> {
        upsert_task_for_sync_on(self.connection(), task)
    }

    pub fn delete_task_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        delete_task_subtree_on(self.connection(), task_id)
    }

    pub fn list_task_subtree_for_sync(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(self.connection(), task_id)
    }

    pub fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(self.connection(), id)
    }

    pub fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(self.connection())
    }

    pub fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(self.connection(), &template)
    }

    pub fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(self.connection(), id)
    }

    pub fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(self.connection(), id)
    }

    pub fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(self.connection())
    }

    pub fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(self.connection(), now_ms)
    }

    pub fn upsert_series(&mut self, schedule: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(self.connection(), &schedule)
    }

    pub fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(self.connection(), id)
    }

    pub fn list_timer_sessions_by_task_for_sync(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            self.connection(),
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    pub fn list_timer_sessions_for_sync(&self) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            self.connection(),
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions ORDER BY started_at, id",
            [],
        )
    }

    pub fn clear_active_timer_for_task_for_sync(
        &mut self,
        task_id: Uuid,
    ) -> Result<bool, StorageError> {
        Ok(self.connection().execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    pub fn get_timer_session(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(self.connection(), id)
    }

    pub fn insert_timer_session(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(self.connection(), session)
    }

    pub fn delete_timer_session(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .connection()
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }

    pub fn commit(mut self) -> Result<Connection, StorageError> {
        self.connection().execute_batch("COMMIT")?;
        Ok(self
            .connection
            .take()
            .expect("committed owned transaction has a connection"))
    }

    pub fn rollback(mut self) -> Result<Connection, StorageError> {
        self.connection().execute_batch("ROLLBACK")?;
        Ok(self
            .connection
            .take()
            .expect("rolled back owned transaction has a connection"))
    }
}

impl Drop for OwnedSqliteWriteTx {
    fn drop(&mut self) {
        if let Some(connection) = &self.connection {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
}
