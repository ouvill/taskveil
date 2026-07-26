use taskveil_crypto::key_hierarchy::KEY_LEN;
use taskveil_domain::{
    archive_list as domain_archive_list, fractional_index_after, fractional_index_between,
    new_list, new_task, rebalance_ranks, rename_list as domain_rename_list, transition_task,
    unarchive_list as domain_unarchive_list, update_due, update_estimated_minutes, update_note,
    update_priority, update_scheduled_at, validate_parent_for, List, Task, TaskDue, TaskStatus,
    Uuid,
};
use taskveil_storage::{open_encrypted, SqliteWriteTx, StorageError, TaskUndoOperation};

use crate::mutation_service::{enqueue_list_in_transaction, enqueue_task_in_transaction};
use crate::{ClientError, LocalMutationContext, SqliteMutationService};

#[derive(Debug, Clone)]
pub struct CreateTaskInput {
    pub list_id: Uuid,
    pub title: String,
    pub parent_task_id: Option<Uuid>,
    pub due: Option<TaskDue>,
    pub note: Option<String>,
    pub priority: i32,
    pub scheduled_at: Option<i64>,
    pub estimated_minutes: Option<i32>,
    pub now_ms: i64,
}

#[derive(Debug, Clone)]
pub struct SetTaskStatusInput {
    pub task_id: Uuid,
    pub status: TaskStatus,
    pub closed_reason: Option<String>,
    pub now_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ReorderTaskInput {
    pub task_id: Uuid,
    pub previous_task_id: Option<Uuid>,
    pub next_task_id: Option<Uuid>,
    pub now_ms: i64,
}

impl SqliteMutationService {
    pub fn create_list(
        &self,
        name: String,
        now_ms: i64,
        _tenant_id: Uuid,
        _master_key: &[u8; KEY_LEN],
        sync: &LocalMutationContext,
    ) -> Result<List, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let mut lists = transaction.list_lists_including_archived()?;
        lists.sort_by(|a, b| (a.sort_order.as_str(), a.id).cmp(&(b.sort_order.as_str(), b.id)));
        let rank = match fractional_index_after(lists.last().map(|list| list.sort_order.as_str())) {
            Ok(rank) => rank,
            Err(taskveil_domain::DomainError::SortOrderSpaceExhausted) => {
                let ranks = rebalance_ranks(lists.len() + 1)?;
                for (mut list, rank) in lists.into_iter().zip(ranks.iter()) {
                    if list.sort_order != *rank {
                        list.sort_order.clone_from(rank);
                        list.updated_at = now_ms;
                        transaction.update_list(list.clone())?;
                        enqueue_list_in_transaction(&mut transaction, sync, &list, false, now_ms)?;
                    }
                }
                ranks.last().cloned().ok_or(ClientError::Sync)?
            }
            Err(error) => return Err(error.into()),
        };
        let list = new_list(name, rank, now_ms)?;
        require_tenant_key(sync)?;
        transaction.insert_list(list.clone())?;
        enqueue_list_in_transaction(&mut transaction, sync, &list, false, now_ms)?;
        transaction.commit()?;
        Ok(list)
    }

    pub fn reorder_task(
        &self,
        input: ReorderTaskInput,
        sync: &LocalMutationContext,
    ) -> Result<Task, ClientError> {
        if input.previous_task_id == Some(input.task_id)
            || input.next_task_id == Some(input.task_id)
            || (input.previous_task_id.is_some() && input.previous_task_id == input.next_task_id)
        {
            return Err(ClientError::Domain(
                taskveil_domain::DomainError::InvalidSortOrderBoundary,
            ));
        }
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let target = transaction.get_task(input.task_id)?;
        require_tenant_key(sync)?;
        let mut scope = transaction
            .list_active_tasks_by_list(target.list_id)?
            .into_iter()
            .filter(|task| task.parent_task_id == target.parent_task_id && task.id != target.id)
            .collect::<Vec<_>>();
        scope.sort_by(|a, b| (a.sort_order.as_str(), a.id).cmp(&(b.sort_order.as_str(), b.id)));
        let insertion = insertion_index(
            &scope,
            input.previous_task_id,
            input.next_task_id,
            target.list_id,
            target.parent_task_id,
        )?;
        let previous_rank = insertion
            .checked_sub(1)
            .and_then(|index| scope.get(index))
            .map(|task| task.sort_order.as_str());
        let next_rank = scope.get(insertion).map(|task| task.sort_order.as_str());
        let midpoint = fractional_index_between(previous_rank, next_rank).ok();
        let collides = midpoint
            .as_ref()
            .is_some_and(|rank| scope.iter().any(|task| task.sort_order == *rank));
        if let Some(rank) = midpoint.filter(|_| !collides) {
            let mut updated = target;
            updated.sort_order = rank;
            updated.updated_at = input.now_ms;
            transaction.update_task(updated.clone())?;
            enqueue_task_in_transaction(&mut transaction, sync, &updated, false, input.now_ms)?;
            transaction.commit()?;
            return Ok(updated);
        }

        scope.insert(insertion, target);
        let ranks = rebalance_ranks(scope.len())?;
        let mut reordered = None;
        for (mut task, rank) in scope.into_iter().zip(ranks) {
            if task.sort_order != rank {
                task.sort_order = rank;
                task.updated_at = input.now_ms;
                transaction.update_task(task.clone())?;
                enqueue_task_in_transaction(&mut transaction, sync, &task, false, input.now_ms)?;
            }
            if task.id == input.task_id {
                reordered = Some(task);
            }
        }
        transaction.commit()?;
        reordered.ok_or(ClientError::Storage(StorageError::NotFound(input.task_id)))
    }

    pub fn create_task(
        &self,
        input: CreateTaskInput,
        sync: &LocalMutationContext,
    ) -> Result<Task, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let list_id = transaction.get_list(input.list_id)?.id;
        require_tenant_key(sync)?;
        let mut tasks = transaction.list_active_tasks_by_list(list_id)?;
        let last_sibling_sort_order = tasks
            .iter()
            .filter(|task| task.parent_task_id == input.parent_task_id)
            .map(|task| task.sort_order.as_str())
            .max();
        let sort_order = match fractional_index_after(last_sibling_sort_order) {
            Ok(rank) => rank,
            Err(taskveil_domain::DomainError::SortOrderSpaceExhausted) => {
                let mut siblings = tasks
                    .iter()
                    .filter(|task| task.parent_task_id == input.parent_task_id)
                    .cloned()
                    .collect::<Vec<_>>();
                siblings.sort_by(|a, b| {
                    (a.sort_order.as_str(), a.id).cmp(&(b.sort_order.as_str(), b.id))
                });
                for (mut sibling, rank) in siblings.into_iter().zip(rebalance_ranks(
                    tasks
                        .iter()
                        .filter(|task| task.parent_task_id == input.parent_task_id)
                        .count(),
                )?) {
                    if sibling.sort_order != rank {
                        sibling.sort_order = rank;
                        sibling.updated_at = input.now_ms;
                        transaction.update_task(sibling.clone())?;
                        enqueue_task_in_transaction(
                            &mut transaction,
                            sync,
                            &sibling,
                            false,
                            input.now_ms,
                        )?;
                    }
                }
                let refreshed = transaction.list_active_tasks_by_list(list_id)?;
                let last = refreshed
                    .iter()
                    .filter(|task| task.parent_task_id == input.parent_task_id)
                    .map(|task| task.sort_order.as_str())
                    .max();
                fractional_index_after(last)?
            }
            Err(error) => return Err(error.into()),
        };
        let mut task = new_task(
            list_id,
            input.parent_task_id,
            input.title,
            sort_order,
            input.now_ms,
        )?;
        if let Some(note) = input.note {
            task = update_note(task, note, input.now_ms)?;
        }
        if let Some(due) = input.due {
            task = update_due(task, Some(due), input.now_ms)?;
        }
        task = update_priority(task, input.priority, input.now_ms)?;
        task = update_scheduled_at(task, input.scheduled_at, input.now_ms)?;
        task = update_estimated_minutes(task, input.estimated_minutes, input.now_ms)?;
        if let Some(parent_id) = input.parent_task_id {
            if !tasks.iter().any(|existing| existing.id == parent_id) {
                match transaction.get_task(parent_id) {
                    Ok(parent) => tasks.push(parent),
                    Err(StorageError::NotFound(_)) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            validate_parent_for(task.id, list_id, parent_id, &tasks)?;
        }
        transaction.insert_task(task.clone())?;
        enqueue_task_in_transaction(&mut transaction, sync, &task, false, input.now_ms)?;
        transaction.commit()?;
        Ok(task)
    }

    pub fn set_task_status(
        &self,
        input: SetTaskStatusInput,
        sync: &LocalMutationContext,
    ) -> Result<Task, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let before = transaction.get_task(input.task_id)?;
        require_tenant_key(sync)?;
        let updated = transition_task(
            before.clone(),
            input.status,
            input.closed_reason,
            input.now_ms,
        )?;
        if matches!(input.status, TaskStatus::Done | TaskStatus::WontDo) {
            transaction.update_task_with_undo(
                before,
                updated.clone(),
                TaskUndoOperation::Complete,
                input.now_ms,
            )?;
        } else {
            transaction.update_task(updated.clone())?;
        }
        enqueue_task_in_transaction(&mut transaction, sync, &updated, false, input.now_ms)?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn undo_task_operation(
        &self,
        undo_id: Uuid,
        now_ms: i64,
        sync: &LocalMutationContext,
    ) -> Result<Task, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let task = transaction.undo_task_operation(undo_id, now_ms)?;
        require_tenant_key(sync)?;
        enqueue_task_in_transaction(&mut transaction, sync, &task, false, now_ms)?;
        transaction.commit()?;
        Ok(task)
    }

    pub fn rename_list(
        &self,
        list_id: Uuid,
        name: String,
        now_ms: i64,
        sync: &LocalMutationContext,
    ) -> Result<List, ClientError> {
        self.mutate_list(list_id, now_ms, sync, |list| {
            domain_rename_list(list, name, now_ms).map_err(ClientError::from)
        })
    }

    pub fn archive_list(
        &self,
        list_id: Uuid,
        now_ms: i64,
        sync: &LocalMutationContext,
    ) -> Result<List, ClientError> {
        self.mutate_list(list_id, now_ms, sync, |list| {
            if list.archived_at.is_none() && list.is_default {
                return Err(StorageError::DefaultListProtected {
                    operation: "archived",
                    list_id: list.id,
                }
                .into());
            }
            domain_archive_list(list, now_ms).map_err(ClientError::from)
        })
    }

    pub fn unarchive_list(
        &self,
        list_id: Uuid,
        now_ms: i64,
        sync: &LocalMutationContext,
    ) -> Result<List, ClientError> {
        self.mutate_list(list_id, now_ms, sync, |list| {
            domain_unarchive_list(list, now_ms).map_err(ClientError::from)
        })
    }

    fn mutate_list(
        &self,
        list_id: Uuid,
        now_ms: i64,
        sync: &LocalMutationContext,
        mutation: impl FnOnce(List) -> Result<List, ClientError>,
    ) -> Result<List, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key)?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        self.assert_runtime_epoch(&transaction)?;
        let before = transaction.get_list(list_id)?;
        require_tenant_key(sync)?;
        let updated = mutation(before.clone())?;
        if updated == before {
            return Ok(before);
        }
        transaction.update_list(updated.clone())?;
        enqueue_list_in_transaction(&mut transaction, sync, &updated, false, now_ms)?;
        transaction.commit()?;
        Ok(updated)
    }
}

fn insertion_index(
    scope: &[Task],
    previous: Option<Uuid>,
    next: Option<Uuid>,
    list_id: Uuid,
    parent_task_id: Option<Uuid>,
) -> Result<usize, ClientError> {
    let find = |id| {
        scope
            .iter()
            .position(|task| task.id == id)
            .ok_or(ClientError::Storage(StorageError::NotFound(id)))
    };
    let index = match (previous, next) {
        (None, None) => scope.len(),
        (Some(previous), None) => find(previous)? + 1,
        (None, Some(next)) => find(next)?,
        (Some(previous), Some(next)) => {
            let previous_index = find(previous)?;
            let next_index = find(next)?;
            if previous_index + 1 != next_index {
                return Err(ClientError::Domain(
                    taskveil_domain::DomainError::InvalidSortOrderBoundary,
                ));
            }
            next_index
        }
    };
    if scope
        .iter()
        .any(|task| task.list_id != list_id || task.parent_task_id != parent_task_id)
    {
        return Err(ClientError::Domain(
            taskveil_domain::DomainError::InvalidSortOrderBoundary,
        ));
    }
    Ok(index)
}

fn require_tenant_key(sync: &LocalMutationContext) -> Result<(), ClientError> {
    if sync.keys.tenant_root_dek.is_some()
        && !sync.keys.tenant_id.is_nil()
        && sync.keys.tenant_generation > 0
    {
        Ok(())
    } else {
        Err(ClientError::Sync)
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use taskveil_domain::{new_list, new_task};
    use taskveil_storage::{
        ListRepository, OwnedSqliteWriteTx, SettingsRepository, SqliteListRepository,
        SqliteSettingsRepository, SqliteSyncStateRepository, SqliteTaskRepository,
        SyncRecordSemanticState, SyncRecordState, SyncStateRepository, TaskRepository,
    };
    use taskveil_sync::{
        Hlc, LocalSyncKeys, SyncPlaintext, LISTS_COLLECTION, SYNC_LOCAL_HLC_SETTING_KEY,
        TASKS_COLLECTION,
    };
    use tempfile::TempDir;

    use super::*;
    const DB_KEY: [u8; 32] = [0x85; 32];
    const BASE_MS: i64 = 1_799_100_000_000;

    struct Fixture {
        _temp: TempDir,
        mutation_service: SqliteMutationService,
        list: List,
        task: Task,
        sync: LocalMutationContext,
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("client.sqlite3");
        let list = new_list(
            "Inbox".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let task = new_task(
            list.id,
            None,
            "before".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteSettingsRepository::new(connection)
            .set_setting(
                SYNC_LOCAL_HLC_SETTING_KEY,
                &Hlc {
                    wall_ms: BASE_MS - 1,
                    counter: 0,
                    device_id: "device-a".to_string(),
                }
                .encode()
                .unwrap(),
                BASE_MS,
            )
            .unwrap();
        Fixture {
            _temp: temp,
            mutation_service: SqliteMutationService::new(db_path, DB_KEY),
            list: list.clone(),
            task,
            sync: LocalMutationContext {
                device_id: "device-a".to_string(),
                keys: LocalSyncKeys {
                    tenant_id: Uuid::from_u128(100),
                    tenant_root_dek: Some(Zeroizing::new([0x56; 32])),
                    tenant_generation: 1,
                    historical_tenant_root_deks: Vec::new(),
                },
            },
        }
    }

    fn create_input(list_id: Uuid) -> CreateTaskInput {
        CreateTaskInput {
            list_id,
            title: "created".to_string(),
            parent_task_id: None,
            due: Some(TaskDue::date_time(BASE_MS + 60_000, "UTC").unwrap()),
            note: Some("note".to_string()),
            priority: 2,
            scheduled_at: Some(BASE_MS + 30_000),
            estimated_minutes: Some(45),
            now_ms: BASE_MS + 1,
        }
    }

    #[test]
    fn alias_list_mutations_target_the_canonical_list_and_key() {
        let fixture = fixture();
        let alias = new_list("Other Inbox".into(), "a0".into(), BASE_MS).unwrap();
        SqliteListRepository::new(
            open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap(),
        )
        .insert(alias.clone())
        .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .materialize_canonical_list(fixture.list.id)
            .unwrap();
        transaction
            .replace_list_aliases(fixture.list.id, &[alias.id], BASE_MS)
            .unwrap();
        transaction.commit().unwrap();

        let created = fixture
            .mutation_service
            .create_task(create_input(alias.id), &fixture.sync)
            .unwrap();
        assert_eq!(created.list_id, fixture.list.id);
        let renamed = fixture
            .mutation_service
            .rename_list(
                alias.id,
                "Canonical Inbox".into(),
                BASE_MS + 2,
                &fixture.sync,
            )
            .unwrap();
        assert_eq!(renamed.id, fixture.list.id);
        assert_eq!(renamed.name, "Canonical Inbox");

        let state = SqliteSyncStateRepository::new(
            open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap(),
        )
        .get_record_state(TASKS_COLLECTION, created.id)
        .unwrap()
        .unwrap();
        let SyncRecordSemanticState::Live { plaintext_json, .. } = state.state else {
            panic!("live task state");
        };
        let SyncPlaintext::Task(plaintext) = serde_json::from_str(&plaintext_json).unwrap() else {
            panic!("task plaintext");
        };
        assert_eq!(plaintext.placement.value.list_id, fixture.list.id);
    }

    #[test]
    fn task_create_status_and_undo_use_transactional_sync_state() {
        let fixture = fixture();
        let created = fixture
            .mutation_service
            .create_task(create_input(fixture.list.id), &fixture.sync)
            .unwrap();
        assert_eq!(created.content.title, "created");
        assert_eq!(created.content.priority, 2);
        assert_eq!(created.scheduled_at, Some(BASE_MS + 30_000));
        assert_eq!(created.content.estimated_minutes, Some(45));
        assert_ne!(created.sort_order, fixture.task.sort_order);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync_state = SqliteSyncStateRepository::new(connection);
        assert_eq!(sync_state.list_outbox_heads(10).unwrap().len(), 1);
        let state = sync_state
            .get_record_state(TASKS_COLLECTION, created.id)
            .unwrap()
            .unwrap();
        let SyncRecordSemanticState::Live { plaintext_json, .. } = state.state else {
            panic!("expected live task state");
        };
        let SyncPlaintext::Task(plaintext) = serde_json::from_str(&plaintext_json).unwrap() else {
            panic!("expected task plaintext");
        };
        assert_eq!(plaintext.priority.value, 2);
        assert_eq!(plaintext.scheduled_at.value, Some(BASE_MS + 30_000));
        assert_eq!(plaintext.estimated_minutes.value, Some(45));

        let done = fixture
            .mutation_service
            .set_task_status(
                SetTaskStatusInput {
                    task_id: created.id,
                    status: TaskStatus::Done,
                    closed_reason: None,
                    now_ms: BASE_MS + 2,
                },
                &fixture.sync,
            )
            .unwrap();
        assert_eq!(done.status, TaskStatus::Done);
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        let undo = tasks.latest_unconsumed_undo().unwrap().unwrap();
        drop(tasks);

        let restored = fixture
            .mutation_service
            .undo_task_operation(undo.id, BASE_MS + 3, &fixture.sync)
            .unwrap();
        assert_eq!(restored.status, TaskStatus::Todo);
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert_eq!(sync.list_outbox_heads(10).unwrap().len(), 1);
        assert!(sync
            .get_record_state(TASKS_COLLECTION, created.id)
            .unwrap()
            .is_some());
    }

    #[test]
    fn account_bound_reorder_uses_midpoint_and_atomic_typed_placement() {
        let fixture = fixture();
        let created = fixture
            .mutation_service
            .create_task(create_input(fixture.list.id), &fixture.sync)
            .unwrap();
        let reordered = fixture
            .mutation_service
            .reorder_task(
                ReorderTaskInput {
                    task_id: created.id,
                    previous_task_id: None,
                    next_task_id: Some(fixture.task.id),
                    now_ms: BASE_MS + 2,
                },
                &fixture.sync,
            )
            .unwrap();
        assert!(reordered.sort_order < fixture.task.sort_order);
        assert_eq!(reordered.sort_order.len(), 32);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let state = SqliteSyncStateRepository::new(connection)
            .get_record_state(TASKS_COLLECTION, created.id)
            .unwrap()
            .unwrap();
        let SyncRecordSemanticState::Live { plaintext_json, .. } = state.state else {
            panic!("live");
        };
        let SyncPlaintext::Task(plaintext) = serde_json::from_str(&plaintext_json).unwrap() else {
            panic!("task");
        };
        assert_eq!(plaintext.placement.value.rank, reordered.sort_order);
    }

    #[test]
    fn exhausted_gap_rebalances_only_scope_and_rolls_back_on_outbox_failure() {
        let fixture = fixture();
        let mut first = fixture.task.clone();
        first.sort_order = "00000000000000000000000000000000".to_string();
        let second = new_task(
            fixture.list.id,
            None,
            "second".to_string(),
            "00000000000000000000000000000001".to_string(),
            BASE_MS,
        )
        .unwrap();
        let target = new_task(
            fixture.list.id,
            None,
            "target".to_string(),
            "ffffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.update(first.clone()).unwrap();
        tasks.insert(second.clone()).unwrap();
        tasks.insert(target.clone()).unwrap();
        drop(tasks);
        install_trigger(
            &fixture,
            "CREATE TRIGGER fail_rebalance_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT, 'fail'); END;",
        );

        assert!(fixture
            .mutation_service
            .reorder_task(
                ReorderTaskInput {
                    task_id: target.id,
                    previous_task_id: Some(first.id),
                    next_task_id: Some(second.id),
                    now_ms: BASE_MS + 2,
                },
                &fixture.sync,
            )
            .is_err());
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(first.id).unwrap().sort_order, first.sort_order);
        assert_eq!(tasks.get(second.id).unwrap().sort_order, second.sort_order);
        assert_eq!(tasks.get(target.id).unwrap().sort_order, target.sort_order);
    }

    #[test]
    fn exhausted_gap_successfully_rebalances_current_scope_only() {
        let fixture = fixture();
        let mut first = fixture.task.clone();
        first.sort_order = "00000000000000000000000000000000".to_string();
        let second = new_task(
            fixture.list.id,
            None,
            "second".to_string(),
            "00000000000000000000000000000001".to_string(),
            BASE_MS,
        )
        .unwrap();
        let target = new_task(
            fixture.list.id,
            None,
            "target".to_string(),
            "ffffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let other_list = new_list(
            "Other".to_string(),
            "bfffffffffffffffffffffffffffffff".to_string(),
            BASE_MS,
        )
        .unwrap();
        let other = new_task(
            other_list.id,
            None,
            "other".to_string(),
            "00000000000000000000000000000001".to_string(),
            BASE_MS,
        )
        .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(other_list).unwrap();
        drop(lists);
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.update(first.clone()).unwrap();
        tasks.insert(second.clone()).unwrap();
        tasks.insert(target.clone()).unwrap();
        tasks.insert(other.clone()).unwrap();
        drop(tasks);

        let reordered = fixture
            .mutation_service
            .reorder_task(
                ReorderTaskInput {
                    task_id: target.id,
                    previous_task_id: Some(first.id),
                    next_task_id: Some(second.id),
                    now_ms: BASE_MS + 2,
                },
                &fixture.sync,
            )
            .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        let first_after = tasks.get(first.id).unwrap();
        let second_after = tasks.get(second.id).unwrap();
        assert!(first_after.sort_order < reordered.sort_order);
        assert!(reordered.sort_order < second_after.sort_order);
        assert_eq!(tasks.get(other.id).unwrap().sort_order, other.sort_order);
        drop(tasks);
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert_eq!(
            SqliteSyncStateRepository::new(connection)
                .list_outbox_heads(10)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn tail_create_rebalances_when_max_rank_exhausts_space() {
        let fixture = fixture();
        let mut existing = fixture.task.clone();
        existing.sort_order = "ffffffffffffffffffffffffffffffff".to_string();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .update(existing.clone())
            .unwrap();

        let created = fixture
            .mutation_service
            .create_task(create_input(fixture.list.id), &fixture.sync)
            .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let existing = SqliteTaskRepository::new(connection)
            .get(existing.id)
            .unwrap();
        assert_eq!(existing.sort_order.len(), 32);
        assert!(existing.sort_order < created.sort_order);
        assert!(created.sort_order.as_str() < "ffffffffffffffffffffffffffffffff");
    }

    #[test]
    fn list_mutations_commit_with_outbox_and_record_state() {
        let fixture = fixture();
        let renamed = fixture
            .mutation_service
            .rename_list(
                fixture.list.id,
                "Renamed".to_string(),
                BASE_MS + 1,
                &fixture.sync,
            )
            .unwrap();
        assert_eq!(renamed.name, "Renamed");
        let archived = fixture
            .mutation_service
            .archive_list(fixture.list.id, BASE_MS + 2, &fixture.sync)
            .unwrap();
        assert_eq!(archived.archived_at, Some(BASE_MS + 2));
        let active = fixture
            .mutation_service
            .unarchive_list(fixture.list.id, BASE_MS + 3, &fixture.sync)
            .unwrap();
        assert_eq!(active.archived_at, None);

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        assert_eq!(sync.list_outbox_heads(10).unwrap().len(), 1);
        assert!(sync
            .get_record_state(LISTS_COLLECTION, fixture.list.id)
            .unwrap()
            .is_some());
    }

    #[test]
    fn idempotent_archive_and_unarchive_leave_domain_clock_state_and_outbox_unchanged() {
        let fixture = fixture();
        fixture
            .mutation_service
            .archive_list(fixture.list.id, BASE_MS + 1, &fixture.sync)
            .unwrap();
        let archived_before = list_sync_snapshot(&fixture);
        let archived_again = fixture
            .mutation_service
            .archive_list(fixture.list.id, BASE_MS + 2, &fixture.sync)
            .unwrap();
        assert_eq!(archived_again.archived_at, Some(BASE_MS + 1));
        assert_eq!(archived_again.updated_at, BASE_MS + 1);
        assert_eq!(list_sync_snapshot(&fixture), archived_before);

        fixture
            .mutation_service
            .unarchive_list(fixture.list.id, BASE_MS + 3, &fixture.sync)
            .unwrap();
        let active_before = list_sync_snapshot(&fixture);
        let active_again = fixture
            .mutation_service
            .unarchive_list(fixture.list.id, BASE_MS + 4, &fixture.sync)
            .unwrap();
        assert_eq!(active_again.archived_at, None);
        assert_eq!(active_again.updated_at, BASE_MS + 3);
        assert_eq!(list_sync_snapshot(&fixture), active_before);
    }

    #[test]
    fn task_create_outbox_failure_rolls_back_insert_hlc_and_state() {
        let fixture = fixture();
        install_trigger(
            &fixture,
            "CREATE TRIGGER fail_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT, 'fail'); END;",
        );
        let before_hlc = local_hlc(&fixture);
        assert!(fixture
            .mutation_service
            .create_task(create_input(fixture.list.id), &fixture.sync)
            .is_err());

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert_eq!(
            SqliteTaskRepository::new(connection)
                .list_active_by_list(fixture.list.id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(local_hlc(&fixture), before_hlc);
    }

    #[test]
    fn status_outbox_failure_rolls_back_task_undo_hlc_and_state() {
        let fixture = fixture();
        let seeded = seed_record_state(&fixture, TASKS_COLLECTION, fixture.task.id);
        install_trigger(
            &fixture,
            "CREATE TRIGGER fail_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT, 'fail'); END;",
        );
        let before_hlc = local_hlc(&fixture);
        assert!(fixture
            .mutation_service
            .set_task_status(
                SetTaskStatusInput {
                    task_id: fixture.task.id,
                    status: TaskStatus::Done,
                    closed_reason: None,
                    now_ms: BASE_MS + 1,
                },
                &fixture.sync,
            )
            .is_err());

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(fixture.task.id).unwrap().status, TaskStatus::Todo);
        assert!(tasks.latest_unconsumed_undo().unwrap().is_none());
        assert_eq!(local_hlc(&fixture), before_hlc);
        assert_eq!(
            record_state(&fixture, TASKS_COLLECTION, fixture.task.id),
            seeded
        );
    }

    #[test]
    fn undo_outbox_failure_rolls_back_restore_and_consumption() {
        let fixture = fixture();
        let mut tasks = {
            let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
            SqliteTaskRepository::new(connection)
        };
        let done =
            transition_task(fixture.task.clone(), TaskStatus::Done, None, BASE_MS + 1).unwrap();
        let undo = tasks
            .update_with_undo(
                fixture.task.clone(),
                done.clone(),
                TaskUndoOperation::Complete,
                BASE_MS + 1,
            )
            .unwrap();
        drop(tasks);
        seed_record_state(&fixture, TASKS_COLLECTION, fixture.task.id);
        install_trigger(
            &fixture,
            "CREATE TRIGGER fail_outbox BEFORE INSERT ON sync_outbox BEGIN SELECT RAISE(ABORT, 'fail'); END;",
        );

        assert!(fixture
            .mutation_service
            .undo_task_operation(undo.id, BASE_MS + 2, &fixture.sync)
            .is_err());
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let tasks = SqliteTaskRepository::new(connection);
        assert_eq!(tasks.get(fixture.task.id).unwrap(), done);
        assert_eq!(tasks.latest_unconsumed_undo().unwrap().unwrap().id, undo.id);
    }

    #[test]
    fn list_record_state_failure_rolls_back_domain_hlc_and_outbox() {
        let fixture = fixture();
        let seeded = seed_record_state(&fixture, LISTS_COLLECTION, fixture.list.id);
        install_trigger(
            &fixture,
            "CREATE TRIGGER fail_state BEFORE UPDATE ON sync_record_states BEGIN SELECT RAISE(ABORT, 'fail'); END;",
        );
        let before_hlc = local_hlc(&fixture);
        assert!(fixture
            .mutation_service
            .rename_list(
                fixture.list.id,
                "No commit".to_string(),
                BASE_MS + 1,
                &fixture.sync,
            )
            .is_err());

        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert_eq!(
            SqliteListRepository::new(connection)
                .get(fixture.list.id)
                .unwrap()
                .name,
            "Inbox"
        );
        assert_eq!(local_hlc(&fixture), before_hlc);
        assert_eq!(
            record_state(&fixture, LISTS_COLLECTION, fixture.list.id),
            seeded
        );
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert!(SqliteSyncStateRepository::new(connection)
            .list_outbox_heads(10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn create_rejects_missing_parent_with_domain_semantics() {
        let fixture = fixture();
        let mut input = create_input(fixture.list.id);
        input.parent_task_id = Some(Uuid::now_v7());
        assert!(matches!(
            fixture.mutation_service.create_task(input, &fixture.sync),
            Err(ClientError::Domain(
                taskveil_domain::DomainError::ParentNotFound
            ))
        ));
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert!(SqliteSyncStateRepository::new(connection)
            .list_outbox_heads(10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn default_list_archive_is_rejected_without_sync_writes() {
        let fixture = fixture();
        let mut default_list = fixture.list.clone();
        default_list.is_default = true;
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        connection
            .execute(
                "UPDATE lists SET is_default = 1 WHERE id = ?1",
                [default_list.id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            fixture
                .mutation_service
                .archive_list(default_list.id, BASE_MS + 1, &fixture.sync,),
            Err(ClientError::Storage(
                StorageError::DefaultListProtected { .. }
            ))
        ));
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        assert!(SqliteSyncStateRepository::new(connection)
            .list_outbox_heads(10)
            .unwrap()
            .is_empty());
    }

    fn install_trigger(fixture: &Fixture, sql: &str) {
        open_encrypted(fixture.mutation_service.db_path(), &DB_KEY)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }

    fn seed_record_state(fixture: &Fixture, collection: &str, record_id: Uuid) -> String {
        let clock = Hlc {
            wall_ms: BASE_MS - 1,
            counter: 0,
            device_id: "seed".to_string(),
        };
        let plaintext = if collection == TASKS_COLLECTION {
            SyncPlaintext::from_task(&fixture.task, clock.clone()).unwrap()
        } else {
            SyncPlaintext::from_list(&fixture.list, clock.clone()).unwrap()
        };
        let plaintext_json = serde_json::to_string(&plaintext).unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        SqliteSyncStateRepository::new(connection)
            .put_record_state(SyncRecordState {
                record_id,
                collection: collection.to_string(),
                current_revision_hlc: None,
                state: SyncRecordSemanticState::Live {
                    mutation_hlc: clock.encode().unwrap(),
                    plaintext_json: plaintext_json.clone(),
                },
                updated_at: BASE_MS,
            })
            .unwrap();
        plaintext_json
    }

    fn record_state(fixture: &Fixture, collection: &str, record_id: Uuid) -> String {
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let state = SqliteSyncStateRepository::new(connection)
            .get_record_state(collection, record_id)
            .unwrap()
            .unwrap();
        match state.state {
            SyncRecordSemanticState::Live { plaintext_json, .. } => plaintext_json,
            SyncRecordSemanticState::Tombstone { .. } => panic!("expected live state"),
        }
    }

    fn local_hlc(fixture: &Fixture) -> Option<String> {
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        SqliteSettingsRepository::new(connection)
            .get_setting(SYNC_LOCAL_HLC_SETTING_KEY)
            .unwrap()
    }

    fn list_sync_snapshot(
        fixture: &Fixture,
    ) -> (
        List,
        Option<String>,
        Option<SyncRecordState>,
        Vec<taskveil_storage::SyncOutboxEntry>,
    ) {
        let list = SqliteListRepository::new(
            open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap(),
        )
        .get(fixture.list.id)
        .unwrap();
        let connection = open_encrypted(fixture.mutation_service.db_path(), &DB_KEY).unwrap();
        let sync = SqliteSyncStateRepository::new(connection);
        (
            list,
            local_hlc(fixture),
            sync.get_record_state(LISTS_COLLECTION, fixture.list.id)
                .unwrap(),
            sync.list_outbox_heads(10).unwrap(),
        )
    }
}
