use crate::*;

/// account bindingとMK-wrapped Tenant Root Key cacheの読込を担うリポジトリ。
///
/// cache更新はgeneration履歴とsemantic key identityを一体で検証する必要があるため、
/// 単一rootを置換する公開repository APIは提供しない。
pub trait LocalCryptoRepository {
    fn load_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError>;
    fn load_tenant_root(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<LocalTenantRootKeyBundle>, StorageError>;
}

/// タスクの永続化を担うリポジトリ。
///
/// SQLite(SQLCipher)実装は [`SqliteTaskRepository`] を参照。同期シグネチャのみを定義する。
pub trait TaskRepository {
    fn get(&self, id: Uuid) -> Result<Task, StorageError>;
    fn insert(&mut self, task: Task) -> Result<(), StorageError>;
    fn update(&mut self, task: Task) -> Result<(), StorageError>;
    fn list_all_for_sync(&self) -> Result<Vec<Task>, StorageError>;
    fn list_active_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError>;
    fn list_home(
        &self,
        today_start_ms: i64,
        tomorrow_start_ms: i64,
    ) -> Result<Vec<HomeTask>, StorageError>;
    fn list_calendar_occurrences(
        &self,
        range: &CalendarRange,
    ) -> Result<Vec<CalendarOccurrence>, StorageError>;
    fn search_tasks(&self, query: &str) -> Result<Vec<Task>, StorageError>;
    fn count_descendants(&self, task_id: Uuid) -> Result<usize, StorageError>;
    fn delete_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError>;
}

/// Tenant-scoped templates and independent Task Series.
pub trait TemplateSeriesRepository {
    fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError>;
    fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError>;
    fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError>;
    fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError>;
    fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError>;
    fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError>;
    fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError>;
    fn upsert_series(&mut self, series: TaskSeries) -> Result<(), StorageError>;
    fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError>;
}

/// Device-local active Timer and immutable completed work sessions.
pub trait TimerSessionRepository {
    fn load_active(&self) -> Result<Option<ActiveTimerSession>, StorageError>;
    fn start_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError>;
    fn update_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError>;
    fn clear_active(&mut self, expected_session_id: Uuid) -> Result<bool, StorageError>;
    fn clear_active_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError>;
    fn get_completed(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError>;
    /// Returns false for an exact immutable replay and rejects differing data.
    fn insert_completed(&mut self, session: CompletedTimerSession) -> Result<bool, StorageError>;
    fn list_completed(&self) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn list_completed_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn list_completed_by_list(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError>;
    fn delete_completed(&mut self, id: Uuid) -> Result<bool, StorageError>;
}

/// リストの永続化を担うリポジトリ。
///
/// SQLite(SQLCipher)実装は [`SqliteListRepository`] を参照。
pub trait ListRepository {
    fn get(&self, id: Uuid) -> Result<List, StorageError>;
    fn insert(&mut self, list: List) -> Result<(), StorageError>;
    fn update(&mut self, list: List) -> Result<(), StorageError>;
    fn list_all(&self) -> Result<Vec<List>, StorageError>;
    fn list_archived(&self) -> Result<Vec<List>, StorageError>;
    fn get_default(&self) -> Result<Option<List>, StorageError>;
    fn ensure_default_list(&mut self, name: String, now_ms: i64) -> Result<List, StorageError>;
    fn count_tasks(&self, list_id: Uuid) -> Result<usize, StorageError>;
    fn delete_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError>;
}

/// Frontend-owned application preferences.
pub trait AppSettingsRepository {
    fn get_app_setting(&self, key: AppSettingKey) -> Result<Option<String>, StorageError>;
    fn set_app_setting(
        &mut self,
        key: AppSettingKey,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError>;
}

/// Account, sync, migration, and runtime metadata.
///
/// This repository is internal to the Rust client boundary and must not be
/// exposed as an arbitrary key/value frontend API.
pub trait InternalMetadataRepository {
    fn get_internal_metadata(&self, key: &str) -> Result<Option<String>, StorageError>;
    fn set_internal_metadata(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError>;
}

/// リマインダーの永続化を担うリポジトリ。
pub trait ReminderRepository {
    fn create_task_reminder(
        &mut self,
        task_id: Uuid,
        remind_at: i64,
        created_at: i64,
    ) -> Result<Reminder, StorageError>;
    fn update_reminder(
        &mut self,
        reminder_id: Uuid,
        remind_at: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError>;
    fn delete_reminder(&mut self, reminder_id: Uuid) -> Result<Reminder, StorageError>;
    fn clear_task_reminders(&mut self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_task_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_task_subtree_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_list_reminders(&self, list_id: Uuid) -> Result<Vec<Reminder>, StorageError>;
    fn list_pending_reminders(&self, now_ms: i64) -> Result<Vec<Reminder>, StorageError>;
    fn snooze_reminder(
        &mut self,
        reminder_id: Uuid,
        snoozed_until: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError>;
}

/// 同期outboxとpull cursorの永続化を担うリポジトリ。
pub trait SyncStateRepository {
    fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError>;
    fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError>;
    /// Lists every pending head, including records currently quarantined.
    fn list_all_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError>;
    fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError>;
    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError>;
    fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError>;
    fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError>;
    fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError>;
    fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError>;
    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), StorageError>;
    fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError>;
    fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError>;
    fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError>;
    fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError>;
    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError>;
}
