use std::collections::{HashMap, HashSet};

use taskveil_domain::{
    calculate_streak, next_occurrence_after, series_task_id, validate_and_normalize_rrule,
    virtual_next_occurrence_after_end, RevisionBoundary, SeriesCursor, SeriesOccurrenceRef, Streak,
    StreakOccurrence, Task, TaskBlueprint, TaskBlueprintNode, TaskSeries, TaskSeriesConfig,
    TaskStatus, TaskTemplate, Uuid, SETTLEMENT_BATCH_SIZE, TASK_BLUEPRINT_SCHEMA_REVISION,
};
use taskveil_storage::{
    open_encrypted, SqliteWriteTx, StorageError, TaskRepository, TemplateSeriesRepository,
};

use crate::mutation_service::{
    enqueue_task_in_transaction, enqueue_task_series_in_transaction,
    enqueue_template_in_transaction, next_revision_in_transaction,
};
use crate::{ClientError, LocalMutationContext};

use super::{now_ms, LocalMutationState, TaskveilClient};

#[derive(Debug, Clone)]
pub struct CreateTemplateCommand {
    pub name: String,
    pub default_list_id: Option<Uuid>,
    pub blueprint: TaskBlueprint,
}

#[derive(Debug, Clone)]
pub struct SaveTemplateCommand {
    pub task_id: Uuid,
    pub name: String,
    pub default_list_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdateTemplateCommand {
    pub template_id: Uuid,
    pub name: String,
    pub default_list_id: Option<Uuid>,
    pub blueprint: Option<TaskBlueprint>,
}

#[derive(Debug, Clone)]
pub struct ReplaceTaskBlueprintCommand {
    pub template_id: Uuid,
    pub task_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct CreateTaskSeriesFromTemplateCommand {
    pub template_id: Uuid,
    pub rrule: String,
    pub starts_at: i64,
    pub time_zone: String,
}

#[derive(Debug, Clone)]
pub struct CreateTaskSeriesFromTaskCommand {
    pub task_id: Uuid,
    pub target_list_id: Option<Uuid>,
    pub rrule: String,
    pub starts_at: i64,
    pub time_zone: String,
}

#[derive(Debug, Clone)]
pub struct UpdateTaskSeriesCommand {
    pub series_id: Uuid,
    pub blueprint: TaskBlueprint,
    pub target_list_id: Option<Uuid>,
    pub rrule: String,
    pub starts_at: i64,
    pub time_zone: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettlementSummary {
    pub generated_occurrences: u32,
    pub generated_tasks: u32,
    pub has_more: bool,
    pub outbox_changed: bool,
}

impl TaskveilClient {
    pub fn validate_recurrence_rule(
        &self,
        rrule: String,
        starts_at: i64,
        time_zone: String,
    ) -> Result<String, ClientError> {
        Ok(validate_and_normalize_rrule(&rrule, starts_at, &time_zone)?)
    }

    pub fn get_templates(&self) -> Result<Vec<TaskTemplate>, ClientError> {
        let _guard = self.operation_guard()?;
        self.with_recurrence_repository(|repository| Ok(repository.list_templates()?))
    }

    pub fn get_task_series(&self) -> Result<Vec<TaskSeries>, ClientError> {
        let _guard = self.operation_guard()?;
        self.with_recurrence_repository(|repository| Ok(repository.list_series()?))
    }

    pub fn create_template(
        &self,
        command: CreateTemplateCommand,
    ) -> Result<TaskTemplate, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            command.blueprint.validate()?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let template = TaskTemplate {
                id: Uuid::now_v7(),
                name: command.name,
                default_list_id: command.default_list_id,
                blueprint: command.blueprint,
                blueprint_revision: reserve_revision(&mut transaction, &state, now)?,
                created_at: now,
                updated_at: now,
            };
            template.validate()?;
            transaction.upsert_template(template.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_template_in_transaction(&mut transaction, sync, &template, false, now)?;
            }
            transaction.commit()?;
            Ok(template)
        })
    }

    pub fn save_task_as_template(
        &self,
        command: SaveTemplateCommand,
    ) -> Result<TaskTemplate, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let blueprint = blueprint_from_subtree(
                &transaction.list_task_subtree(command.task_id)?,
                command.task_id,
            )?;
            let revision = reserve_revision(&mut transaction, &state, now)?;
            let template = TaskTemplate {
                id: Uuid::now_v7(),
                name: command.name,
                default_list_id: command.default_list_id,
                blueprint,
                blueprint_revision: revision,
                created_at: now,
                updated_at: now,
            };
            template.validate()?;
            transaction.upsert_template(template.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_template_in_transaction(&mut transaction, sync, &template, false, now)?;
            }
            transaction.commit()?;
            Ok(template)
        })
    }

    pub fn update_template(
        &self,
        command: UpdateTemplateCommand,
    ) -> Result<TaskTemplate, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let mut template = transaction.get_template(command.template_id)?;
            template.name = command.name;
            template.default_list_id = command.default_list_id;
            if let Some(blueprint) = command.blueprint {
                blueprint.validate()?;
                template.blueprint = blueprint;
                template.blueprint_revision = reserve_revision(&mut transaction, &state, now)?;
            }
            template.updated_at = now;
            template.validate()?;
            transaction.upsert_template(template.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_template_in_transaction(&mut transaction, sync, &template, false, now)?;
            }
            transaction.commit()?;
            Ok(template)
        })
    }

    pub fn replace_template_blueprint(
        &self,
        command: ReplaceTaskBlueprintCommand,
    ) -> Result<TaskTemplate, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let blueprint = blueprint_from_subtree(
                &transaction.list_task_subtree(command.task_id)?,
                command.task_id,
            )?;
            let mut template = transaction.get_template(command.template_id)?;
            template.blueprint = blueprint;
            template.blueprint_revision = reserve_revision(&mut transaction, &state, now)?;
            template.updated_at = now;
            template.validate()?;
            transaction.upsert_template(template.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_template_in_transaction(&mut transaction, sync, &template, false, now)?;
            }
            transaction.commit()?;
            Ok(template)
        })
    }

    pub fn instantiate_template(&self, template_id: Uuid) -> Result<Vec<Task>, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let template = transaction.get_template(template_id)?;
            let list_id = resolve_target_list(&transaction, template.default_list_id)?;
            let tasks = instantiate_blueprint(&template.blueprint, None, list_id, now)?;
            for task in &tasks {
                transaction.insert_task(task.clone())?;
                if let Some(sync) = sync_context(&state) {
                    enqueue_task_in_transaction(&mut transaction, sync, task, false, now)?;
                }
            }
            transaction.commit()?;
            Ok(tasks)
        })
    }

    pub fn create_task_series_from_template(
        &self,
        command: CreateTaskSeriesFromTemplateCommand,
    ) -> Result<TaskSeries, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let normalized = validate_and_normalize_rrule(
                &command.rrule,
                command.starts_at,
                &command.time_zone,
            )?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let revision = reserve_revision(&mut transaction, &state, now)?;
            let cursor = first_cursor(
                &normalized,
                command.starts_at,
                &command.time_zone,
                command.starts_at.saturating_sub(1),
            )?;
            let template = transaction.get_template(command.template_id)?;
            let series = TaskSeries {
                id: Uuid::now_v7(),
                config: TaskSeriesConfig {
                    blueprint: template.blueprint,
                    target_list_id: template.default_list_id,
                    rrule: normalized,
                    starts_at: command.starts_at,
                    time_zone: command.time_zone,
                    enabled: true,
                    config_revision: revision,
                    config_parent_revision: None,
                    config_effective_from: now,
                    lineage: Vec::new(),
                },
                cursor,
                created_at: now,
                updated_at: now,
            };
            series.validate()?;
            transaction.upsert_series(series.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_task_series_in_transaction(&mut transaction, sync, &series, false, now)?;
            }
            transaction.commit()?;
            Ok(series)
        })
    }

    pub fn create_task_series_from_task(
        &self,
        command: CreateTaskSeriesFromTaskCommand,
    ) -> Result<TaskSeries, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let normalized = validate_and_normalize_rrule(
                &command.rrule,
                command.starts_at,
                &command.time_zone,
            )?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let subtree = transaction.list_task_subtree(command.task_id)?;
            let source_list_id = subtree
                .iter()
                .find(|task| task.id == command.task_id)
                .map(|task| task.list_id)
                .ok_or(StorageError::NotFound(command.task_id))?;
            let blueprint = blueprint_from_subtree(&subtree, command.task_id)?;
            let revision = reserve_revision(&mut transaction, &state, now)?;
            let cursor = first_cursor(
                &normalized,
                command.starts_at,
                &command.time_zone,
                command.starts_at.saturating_sub(1),
            )?;
            let series = TaskSeries {
                id: Uuid::now_v7(),
                config: TaskSeriesConfig {
                    blueprint,
                    target_list_id: Some(command.target_list_id.unwrap_or(source_list_id)),
                    rrule: normalized,
                    starts_at: command.starts_at,
                    time_zone: command.time_zone,
                    enabled: true,
                    config_revision: revision,
                    config_parent_revision: None,
                    config_effective_from: now,
                    lineage: Vec::new(),
                },
                cursor,
                created_at: now,
                updated_at: now,
            };
            series.validate()?;
            transaction.upsert_series(series.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_task_series_in_transaction(&mut transaction, sync, &series, false, now)?;
            }
            transaction.commit()?;
            Ok(series)
        })
    }

    pub fn update_task_series(
        &self,
        command: UpdateTaskSeriesCommand,
    ) -> Result<TaskSeries, ClientError> {
        let _guard = self.operation_guard()?;
        command.blueprint.validate()?;
        let normalized =
            validate_and_normalize_rrule(&command.rrule, command.starts_at, &command.time_zone)?;

        // Settlement may span several bounded transactions. Keep it outside
        // the automatically replayed edit phase: every batch asserts the
        // loaded epoch before its first write, and an epoch change after a
        // committed batch aborts the command instead of replaying already
        // published settlement side effects.
        let settlement_state = self.local_mutation_state()?;
        ensure_available(&settlement_state)?;
        self.settle_all_before_edit(&settlement_state, now_ms()?)?;

        self.retry_runtime_epoch_once(|| {
            let command = command.clone();
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let normalized = normalized.clone();
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let mut series = transaction.get_series(command.series_id)?;
            let previous_revision = series.config.config_revision.clone();
            push_boundary(
                &mut series.config.lineage,
                &previous_revision,
                series.config.config_parent_revision.clone(),
                series.config.config_effective_from,
            );
            series.config.blueprint = command.blueprint;
            series.config.target_list_id = command.target_list_id;
            series.config.rrule = normalized;
            series.config.starts_at = command.starts_at;
            series.config.time_zone = command.time_zone;
            series.config.enabled = command.enabled;
            series.config.config_parent_revision = Some(previous_revision);
            series.config.config_revision = reserve_revision(&mut transaction, &state, now)?;
            series.config.config_effective_from = now;
            let cursor_after = now.min(series.config.starts_at.saturating_sub(1));
            series.cursor = first_cursor(
                &series.config.rrule,
                series.config.starts_at,
                &series.config.time_zone,
                cursor_after,
            )?;
            series.updated_at = now;
            series.validate()?;
            transaction.upsert_series(series.clone())?;
            if let Some(sync) = sync_context(&state) {
                enqueue_task_series_in_transaction(&mut transaction, sync, &series, false, now)?;
            }
            transaction.commit()?;
            Ok(series)
        })
    }

    pub fn delete_series(&self, series_id: Uuid) -> Result<(), ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let series = transaction.get_series(series_id)?;
            if let Some(sync) = sync_context(&state) {
                enqueue_task_series_in_transaction(&mut transaction, sync, &series, true, now)?;
            }
            transaction.delete_series(series_id)?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn delete_template(&self, template_id: Uuid) -> Result<(), ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            let now = now_ms()?;
            let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
            let mut transaction = SqliteWriteTx::begin(&mut connection)?;
            transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
            let template = transaction.get_template(template_id)?;
            if let Some(sync) = sync_context(&state) {
                enqueue_template_in_transaction(&mut transaction, sync, &template, true, now)?;
            }
            transaction.delete_template(template_id)?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Settles at most 100 due occurrences. Frontends yield and call again
    /// while `has_more` is true.
    pub fn settle_due_series(&self, at_ms: i64) -> Result<SettlementSummary, ClientError> {
        let _guard = self.operation_guard()?;
        self.retry_runtime_epoch_once(|| {
            let state = self.local_mutation_state()?;
            ensure_available(&state)?;
            self.settle_batch(&state, at_ms, None)
        })
    }

    pub fn get_series_streak(&self, series_id: Uuid, at_ms: i64) -> Result<Streak, ClientError> {
        let _guard = self.operation_guard()?;
        let series =
            self.with_recurrence_repository(|repository| Ok(repository.get_series(series_id)?))?;
        let mut roots = self.with_task_repository(|repository| {
            Ok(repository
                .list_all_for_sync()?
                .into_iter()
                .filter(|task| {
                    task.parent_task_id.is_none()
                        && task
                            .series_occurrence
                            .as_ref()
                            .is_some_and(|value| value.series_id == series_id)
                })
                .collect::<Vec<_>>())
        })?;
        roots.sort_by_key(|task| {
            task.series_occurrence
                .as_ref()
                .map(|value| value.occurrence_at)
        });
        let mut occurrences = Vec::with_capacity(roots.len());
        for (index, task) in roots.iter().enumerate() {
            let occurrence_at = task
                .series_occurrence
                .as_ref()
                .expect("filtered recurrence")
                .occurrence_at;
            let deadline_at = roots
                .get(index + 1)
                .and_then(|next| {
                    next.series_occurrence
                        .as_ref()
                        .map(|value| value.occurrence_at)
                })
                .or_else(|| series.cursor.next_run_at())
                .or(virtual_next_occurrence_after_end(
                    &series.config.rrule,
                    series.config.starts_at,
                    &series.config.time_zone,
                    occurrence_at,
                )?)
                .unwrap_or(i64::MAX);
            occurrences.push(StreakOccurrence {
                occurrence_at,
                deadline_at,
                status: task.status,
                completed_at: task.completed_at,
            });
        }
        Ok(calculate_streak(&occurrences, at_ms))
    }

    pub(super) fn settle_after_sync_pull(
        &self,
        store: &mut crate::SqliteSyncStore,
        at_ms: i64,
    ) -> Result<SettlementSummary, ClientError> {
        let state = self.local_mutation_state()?;
        ensure_available(&state)?;
        let lease = store.renew_sync_lease()?.ok_or(ClientError::LeaseLost)?;
        self.settle_batch(&state, at_ms, Some(&lease))
    }

    fn settle_all_before_edit(
        &self,
        state: &LocalMutationState,
        at_ms: i64,
    ) -> Result<(), ClientError> {
        loop {
            let summary = self.settle_batch(state, at_ms, None)?;
            if !summary.has_more {
                return Ok(());
            }
        }
    }

    fn settle_batch(
        &self,
        state: &LocalMutationState,
        at_ms: i64,
        lease: Option<&taskveil_storage::SyncLease>,
    ) -> Result<SettlementSummary, ClientError> {
        let mut connection = open_encrypted(&self.db_path, &self.db_key())?;
        let mut transaction = SqliteWriteTx::begin(&mut connection)?;
        transaction.assert_profile_runtime_epoch(self.loaded_runtime_epoch())?;
        if let Some(lease) = lease {
            transaction.assert_sync_lease(lease, now_ms()?)?;
        }
        let reconciled = reconcile_superseded_tasks(&mut transaction, state, at_ms)?;
        let series = transaction.list_due_series(at_ms)?;
        let mut summary = SettlementSummary {
            outbox_changed: reconciled && sync_context(state).is_some(),
            ..SettlementSummary::default()
        };
        let mut remaining = u32::from(SETTLEMENT_BATCH_SIZE);
        for mut series in series {
            if remaining == 0 {
                break;
            }
            let list_id = resolve_target_list(&transaction, series.config.target_list_id)?;
            while remaining > 0 {
                let SeriesCursor::Pending(occurrence_at) = series.cursor else {
                    break;
                };
                if occurrence_at > at_ms || !series.config.enabled {
                    break;
                }
                let root_id = series_task_id(
                    series.id,
                    &series.config.config_revision,
                    occurrence_at,
                    root_node(&series.config.blueprint)?.node_key.as_str(),
                );
                let exists = match transaction.get_task(root_id) {
                    Ok(_) => true,
                    Err(StorageError::NotFound(_)) => false,
                    Err(error) => return Err(error.into()),
                };
                if !exists {
                    let tasks = instantiate_blueprint(
                        &series.config.blueprint,
                        Some((&series, occurrence_at)),
                        list_id,
                        occurrence_at,
                    )?;
                    for task in &tasks {
                        transaction.insert_task(task.clone())?;
                        if let Some(sync) = sync_context(state) {
                            enqueue_task_in_transaction(
                                &mut transaction,
                                sync,
                                task,
                                false,
                                at_ms,
                            )?;
                        }
                    }
                    summary.generated_tasks += u32::try_from(tasks.len()).unwrap_or(u32::MAX);
                }
                summary.generated_occurrences += 1;
                remaining -= 1;
                series.cursor = first_cursor(
                    &series.config.rrule,
                    series.config.starts_at,
                    &series.config.time_zone,
                    occurrence_at,
                )?;
            }
            series.updated_at = at_ms;
            transaction.upsert_series(series.clone())?;
            if let Some(sync) = sync_context(state) {
                enqueue_task_series_in_transaction(&mut transaction, sync, &series, false, at_ms)?;
                summary.outbox_changed = true;
            }
        }
        summary.has_more = transaction.list_due_series(at_ms)?.iter().any(|series| {
            series.config.enabled
                && series
                    .cursor
                    .next_run_at()
                    .is_some_and(|next| next <= at_ms)
        });
        if let Some(lease) = lease {
            transaction.assert_sync_lease(lease, now_ms()?)?;
        }
        transaction.commit()?;
        Ok(summary)
    }
}

fn ensure_available(state: &LocalMutationState) -> Result<(), ClientError> {
    if matches!(state, LocalMutationState::AccountBoundUnavailable) {
        Err(ClientError::AccountBoundUnavailable)
    } else {
        Ok(())
    }
}

fn sync_context(state: &LocalMutationState) -> Option<&LocalMutationContext> {
    match state {
        LocalMutationState::Ready(sync) => Some(sync),
        LocalMutationState::Anonymous | LocalMutationState::AccountBoundUnavailable => None,
    }
}

fn reserve_revision(
    transaction: &mut SqliteWriteTx<'_>,
    state: &LocalMutationState,
    now: i64,
) -> Result<String, ClientError> {
    let device_id = sync_context(state)
        .map(|sync| sync.device_id.as_str())
        .unwrap_or("anonymous-local");
    next_revision_in_transaction(transaction, device_id, now)
}

fn first_cursor(
    rrule: &str,
    starts_at: i64,
    time_zone: &str,
    after: i64,
) -> Result<SeriesCursor, ClientError> {
    Ok(next_occurrence_after(rrule, starts_at, time_zone, after)?
        .map_or(SeriesCursor::Exhausted, SeriesCursor::Pending))
}

fn push_boundary(
    lineage: &mut Vec<RevisionBoundary>,
    revision: &str,
    parent_revision: Option<String>,
    effective_from: i64,
) {
    if lineage.iter().all(|entry| entry.revision != revision) {
        lineage.push(RevisionBoundary {
            revision: revision.to_string(),
            parent_revision,
            effective_from,
        });
        lineage.sort_by_key(|entry| entry.effective_from);
    }
}

fn blueprint_from_subtree(tasks: &[Task], root_id: Uuid) -> Result<TaskBlueprint, ClientError> {
    let ids = tasks.iter().map(|task| task.id).collect::<HashSet<_>>();
    let mut sibling_orders = HashMap::<Option<Uuid>, Vec<&Task>>::new();
    for task in tasks {
        let parent = task.parent_task_id.filter(|id| ids.contains(id));
        sibling_orders.entry(parent).or_default().push(task);
    }
    for siblings in sibling_orders.values_mut() {
        siblings.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.id.cmp(&right.id))
        });
    }
    let mut order_by_id = HashMap::new();
    for siblings in sibling_orders.values() {
        for (index, task) in siblings.iter().enumerate() {
            order_by_id.insert(task.id, u32::try_from(index).unwrap_or(u32::MAX));
        }
    }
    let blueprint = TaskBlueprint {
        schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
        nodes: tasks
            .iter()
            .map(|task| TaskBlueprintNode {
                node_key: task.id.to_string(),
                parent_node_key: if task.id == root_id {
                    None
                } else {
                    task.parent_task_id
                        .filter(|id| ids.contains(id))
                        .map(|id| id.to_string())
                },
                sibling_order: *order_by_id.get(&task.id).unwrap_or(&0),
                content: task.content.clone(),
            })
            .collect(),
    };
    blueprint.validate()?;
    Ok(blueprint)
}

fn resolve_target_list(
    transaction: &SqliteWriteTx<'_>,
    preferred: Option<Uuid>,
) -> Result<Uuid, ClientError> {
    if let Some(preferred) = preferred {
        match transaction.get_list(preferred) {
            Ok(list) if list.archived_at.is_none() => return Ok(list.id),
            Ok(_) | Err(StorageError::NotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    transaction.default_list_id()?.ok_or_else(|| {
        StorageError::IncompatibleSchema("canonical Inbox is missing".to_string()).into()
    })
}

fn root_node(blueprint: &TaskBlueprint) -> Result<&TaskBlueprintNode, ClientError> {
    blueprint
        .nodes
        .iter()
        .find(|node| node.parent_node_key.is_none())
        .ok_or(taskveil_domain::RecurrenceError::InvalidRootCount.into())
}

fn instantiate_blueprint(
    blueprint: &TaskBlueprint,
    series_occurrence: Option<(&TaskSeries, i64)>,
    list_id: Uuid,
    created_at: i64,
) -> Result<Vec<Task>, ClientError> {
    let mut ids = HashMap::new();
    for node in &blueprint.nodes {
        let id = if let Some((series, occurrence_at)) = series_occurrence {
            series_task_id(
                series.id,
                &series.config.config_revision,
                occurrence_at,
                &node.node_key,
            )
        } else {
            Uuid::now_v7()
        };
        ids.insert(node.node_key.as_str(), id);
    }
    let mut pending = blueprint.nodes.iter().collect::<Vec<_>>();
    let mut tasks = Vec::with_capacity(pending.len());
    let mut materialized = HashSet::new();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|node| {
            if node
                .parent_node_key
                .as_ref()
                .is_some_and(|parent| !materialized.contains(parent.as_str()))
            {
                return true;
            }
            let is_root = node.parent_node_key.is_none();
            tasks.push(Task {
                id: ids[node.node_key.as_str()],
                list_id,
                parent_task_id: node
                    .parent_node_key
                    .as_ref()
                    .and_then(|parent| ids.get(parent.as_str()).copied()),
                content: node.content.clone(),
                status: TaskStatus::Todo,
                due: None,
                scheduled_at: series_occurrence
                    .filter(|_| is_root)
                    .map(|(_, occurrence_at)| occurrence_at),
                sort_order: format!("{:032x}", u128::from(node.sibling_order) + 1),
                completed_at: None,
                closed_reason: None,
                deleted_at: None,
                assignee: None,
                series_occurrence: series_occurrence.map(|(series, occurrence_at)| {
                    SeriesOccurrenceRef {
                        series_id: series.id,
                        series_revision: series.config.config_revision.clone(),
                        occurrence_at,
                        blueprint_node_key: node.node_key.clone(),
                    }
                }),
                created_at,
                updated_at: created_at,
            });
            materialized.insert(node.node_key.as_str());
            false
        });
        if pending.len() == before {
            return Err(taskveil_domain::RecurrenceError::InvalidParent.into());
        }
    }
    Ok(tasks)
}

fn reconcile_superseded_tasks(
    transaction: &mut SqliteWriteTx<'_>,
    state: &LocalMutationState,
    now: i64,
) -> Result<bool, ClientError> {
    let series = transaction.list_series()?;
    let by_series = series
        .iter()
        .map(|series| (series.id, series))
        .collect::<HashMap<_, _>>();
    let mut roots = transaction
        .list_all_tasks_for_sync()?
        .into_iter()
        .filter(|task| task.parent_task_id.is_none() && task.series_occurrence.is_some())
        .collect::<Vec<_>>();
    roots.sort_by_key(|task| task.id);
    let mut changed = false;
    for root in roots {
        let provenance = root
            .series_occurrence
            .as_ref()
            .expect("filtered provenance");
        let Some(series) = by_series.get(&provenance.series_id) else {
            // Task Series deletion intentionally keeps generated tasks.
            continue;
        };
        let valid = revision_accepts_occurrence(
            &series.config.lineage,
            &series.config.config_revision,
            series.config.config_parent_revision.as_deref(),
            series.config.config_effective_from,
            &provenance.series_revision,
            provenance.occurrence_at,
        );
        if valid {
            continue;
        }
        let subtree = transaction.list_task_subtree(root.id)?;
        if let Some(sync) = sync_context(state) {
            for task in &subtree {
                enqueue_task_in_transaction(transaction, sync, task, true, now)?;
            }
        }
        transaction.delete_task_subtree(root.id)?;
        changed = true;
    }
    Ok(changed)
}

fn revision_accepts_occurrence(
    lineage: &[RevisionBoundary],
    current_revision: &str,
    current_parent: Option<&str>,
    current_effective_from: i64,
    candidate_revision: &str,
    occurrence_at: i64,
) -> bool {
    let mut boundaries = lineage.to_vec();
    if boundaries
        .iter()
        .all(|entry| entry.revision != current_revision)
    {
        boundaries.push(RevisionBoundary {
            revision: current_revision.to_string(),
            parent_revision: current_parent.map(str::to_string),
            effective_from: current_effective_from,
        });
    }
    boundaries.sort_by_key(|entry| entry.effective_from);
    let Some(index) = boundaries
        .iter()
        .position(|entry| entry.revision == candidate_revision)
    else {
        return false;
    };
    let lower = if index == 0 {
        i64::MIN
    } else {
        boundaries[index].effective_from
    };
    let upper = boundaries
        .get(index + 1)
        .map(|entry| entry.effective_from)
        .unwrap_or(i64::MAX);
    occurrence_at >= lower && occurrence_at < upper
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use taskveil_domain::{new_list, new_task, TaskContent};
    use taskveil_storage::{
        ListRepository, SqliteListRepository, SqliteTaskRepository, TaskRepository,
    };
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    use super::*;
    use crate::runtime::{AccountRuntimeState, CryptoRuntimeState, SyncRuntimeState};

    const DB_KEY: [u8; 32] = [0xb7; 32];

    fn anonymous_client_fixture() -> (TempDir, TaskveilClient, taskveil_domain::List, Task) {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("recurrence.sqlite3");
        let mut inbox = new_list("Inbox".into(), "a0".into(), 1_700_000_000_000).unwrap();
        inbox.is_default = true;
        let task = new_task(
            inbox.id,
            None,
            "Repeat me".into(),
            "a0".into(),
            1_700_000_000_000,
        )
        .unwrap();
        SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .insert(inbox.clone())
            .unwrap();
        SqliteTaskRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .insert(task.clone())
            .unwrap();
        let client = TaskveilClient {
            db_dir: temp.path().to_path_buf(),
            profile_coordinator: TaskveilClient::pinned_test_coordinator(temp.path(), &db_path),
            db_path,
            db_key: Mutex::new(Zeroizing::new(DB_KEY)),
            account: Mutex::new(AccountRuntimeState {
                session: None,
                session_restored: true,
                loaded_credential_generation: None,
                crypto: CryptoRuntimeState::Anonymous,
            }),
            sync: Mutex::new(SyncRuntimeState::default()),
            runtime_epoch: std::sync::atomic::AtomicI64::new(1),
            capsule_generation: std::sync::atomic::AtomicU64::new(1),
        };
        (temp, client, inbox, task)
    }

    #[test]
    fn revision_windows_reject_losing_and_post_cutover_old_instances() {
        let lineage = vec![RevisionBoundary {
            revision: "old".to_string(),
            parent_revision: None,
            effective_from: 100,
        }];
        assert!(revision_accepts_occurrence(
            &lineage,
            "new",
            Some("old"),
            200,
            "old",
            199,
        ));
        assert!(!revision_accepts_occurrence(
            &lineage,
            "new",
            Some("old"),
            200,
            "old",
            200,
        ));
        assert!(revision_accepts_occurrence(
            &lineage,
            "new",
            Some("old"),
            200,
            "new",
            200,
        ));
        assert!(!revision_accepts_occurrence(
            &lineage,
            "new",
            Some("old"),
            200,
            "losing-child",
            250,
        ));
    }

    #[test]
    fn series_blueprint_is_content_only_and_deterministic() {
        let template = TaskTemplate {
            id: Uuid::now_v7(),
            name: "Review".to_string(),
            default_list_id: None,
            blueprint: TaskBlueprint {
                schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                nodes: vec![
                    TaskBlueprintNode {
                        node_key: "root".to_string(),
                        parent_node_key: None,
                        sibling_order: 0,
                        content: TaskContent {
                            title: "Review".to_string(),
                            note: "".to_string(),
                            priority: 1,
                            estimated_minutes: Some(30),
                        },
                    },
                    TaskBlueprintNode {
                        node_key: "child".to_string(),
                        parent_node_key: Some("root".to_string()),
                        sibling_order: 0,
                        content: TaskContent {
                            title: "Collect".to_string(),
                            note: "Notes".to_string(),
                            priority: 0,
                            estimated_minutes: None,
                        },
                    },
                ],
            },
            blueprint_revision: "template-r1".to_string(),
            created_at: 1,
            updated_at: 1,
        };
        let series = TaskSeries {
            id: Uuid::now_v7(),
            config: TaskSeriesConfig {
                blueprint: template.blueprint.clone(),
                target_list_id: None,
                rrule: "FREQ=DAILY".to_string(),
                starts_at: 1_800_000_000_000,
                time_zone: "UTC".to_string(),
                enabled: true,
                config_revision: "series-r1".to_string(),
                config_parent_revision: None,
                config_effective_from: 1,
                lineage: Vec::new(),
            },
            cursor: SeriesCursor::Pending(1_800_000_000_000),
            created_at: 1,
            updated_at: 1,
        };
        let first = instantiate_blueprint(
            &template.blueprint,
            Some((&series, series.config.starts_at)),
            Uuid::now_v7(),
            series.config.starts_at,
        )
        .unwrap();
        let second = instantiate_blueprint(
            &template.blueprint,
            Some((&series, series.config.starts_at)),
            first[0].list_id,
            series.config.starts_at,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].scheduled_at, Some(series.config.starts_at));
        assert_eq!(first[1].scheduled_at, None);
        assert!(first.iter().all(|task| task.due.is_none()));
        assert!(first.iter().all(|task| task.status == TaskStatus::Todo));
    }

    #[test]
    fn client_settlement_batches_long_offline_catchup_and_is_idempotent() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let template = client
            .save_task_as_template(SaveTemplateCommand {
                task_id: source.id,
                name: "Daily review".into(),
                default_list_id: Some(inbox.id),
            })
            .unwrap();
        let day_ms = 24 * 60 * 60 * 1000;
        let at_ms = 1_800_000_000_000;
        let starts_at = at_ms - 104 * day_ms;
        let series = client
            .create_task_series_from_template(CreateTaskSeriesFromTemplateCommand {
                template_id: template.id,
                rrule: "FREQ=DAILY;COUNT=105".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();

        let first = client.settle_due_series(at_ms).unwrap();
        assert_eq!(first.generated_occurrences, 100);
        assert_eq!(first.generated_tasks, 100);
        assert!(first.has_more);
        let second = client.settle_due_series(at_ms).unwrap();
        assert_eq!(second.generated_occurrences, 5);
        assert_eq!(second.generated_tasks, 5);
        assert!(!second.has_more);
        let replay = client.settle_due_series(at_ms).unwrap();
        assert_eq!(replay.generated_occurrences, 0);
        assert_eq!(replay.generated_tasks, 0);

        let tasks = client.get_tasks(inbox.id).unwrap();
        let generated = tasks
            .iter()
            .filter(|task| {
                task.series_occurrence
                    .as_ref()
                    .is_some_and(|value| value.series_id == series.id)
            })
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 105);
        assert_eq!(
            generated
                .iter()
                .map(|task| task.id)
                .collect::<HashSet<_>>()
                .len(),
            105
        );
        assert!(generated.iter().all(|task| task.due.is_none()));
        assert!(generated.iter().all(|task| task.scheduled_at
            == task
                .series_occurrence
                .as_ref()
                .map(|value| value.occurrence_at)));
        let series = client.get_task_series().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].cursor, SeriesCursor::Exhausted);

        client.delete_series(series[0].id).unwrap();
        let retained = client
            .get_tasks(inbox.id)
            .unwrap()
            .into_iter()
            .filter(|task| task.series_occurrence.is_some())
            .count();
        assert_eq!(retained, 105);
    }

    #[test]
    fn paused_series_generates_nothing_and_resume_generates_once() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let starts_at = now_ms().unwrap() + 60_000;
        let series = client
            .create_task_series_from_task(CreateTaskSeriesFromTaskCommand {
                task_id: source.id,
                target_list_id: Some(inbox.id),
                rrule: "FREQ=DAILY;COUNT=1".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();

        let resumed_series = client
            .update_task_series(UpdateTaskSeriesCommand {
                series_id: series.id,
                blueprint: series.config.blueprint.clone(),
                target_list_id: series.config.target_list_id,
                rrule: series.config.rrule.clone(),
                starts_at,
                time_zone: "UTC".into(),
                enabled: false,
            })
            .unwrap();
        assert_eq!(
            client.settle_due_series(starts_at).unwrap(),
            SettlementSummary::default()
        );

        client
            .update_task_series(UpdateTaskSeriesCommand {
                series_id: series.id,
                blueprint: series.config.blueprint.clone(),
                target_list_id: series.config.target_list_id,
                rrule: series.config.rrule,
                starts_at,
                time_zone: "UTC".into(),
                enabled: true,
            })
            .unwrap();
        assert_eq!(resumed_series.cursor, SeriesCursor::Pending(starts_at));
        let resumed = client.settle_due_series(starts_at).unwrap();
        assert_eq!(resumed.generated_occurrences, 1);
        assert_eq!(resumed.generated_tasks, 1);
        assert_eq!(
            client.settle_due_series(starts_at).unwrap(),
            SettlementSummary::default()
        );
    }

    #[test]
    fn series_update_replaces_blueprint_and_target_as_one_future_config() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let starts_at = now_ms().unwrap() + 60_000;
        let series = client
            .create_task_series_from_task(CreateTaskSeriesFromTaskCommand {
                task_id: source.id,
                target_list_id: Some(inbox.id),
                rrule: "FREQ=DAILY;COUNT=1".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();
        let mut blueprint = series.config.blueprint.clone();
        blueprint.nodes[0].content.title = "Updated series content".into();

        let updated = client
            .update_task_series(UpdateTaskSeriesCommand {
                series_id: series.id,
                blueprint: blueprint.clone(),
                target_list_id: None,
                rrule: series.config.rrule,
                starts_at,
                time_zone: "UTC".into(),
                enabled: true,
            })
            .unwrap();

        assert_eq!(updated.config.blueprint, blueprint);
        assert_eq!(updated.config.target_list_id, None);
        assert_eq!(
            updated.config.config_parent_revision,
            Some(series.config.config_revision)
        );
        let generated = client.settle_due_series(starts_at).unwrap();
        assert_eq!(generated.generated_occurrences, 1);
        assert!(client
            .get_tasks(inbox.id)
            .unwrap()
            .iter()
            .any(|task| task.content.title == "Updated series content"));
    }

    #[test]
    fn stale_series_cursor_replay_does_not_duplicate_generated_tasks() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let starts_at = 1_900_000_000_000;
        let day_ms = 24 * 60 * 60 * 1_000;
        let series = client
            .create_task_series_from_task(CreateTaskSeriesFromTaskCommand {
                task_id: source.id,
                target_list_id: Some(inbox.id),
                rrule: "FREQ=DAILY;COUNT=2".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();
        let first = client.settle_due_series(starts_at + day_ms).unwrap();
        assert_eq!(first.generated_tasks, 2);

        let mut connection = open_encrypted(&client.db_path, &client.db_key()).unwrap();
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        let mut stale = transaction.get_series(series.id).unwrap();
        stale.cursor = SeriesCursor::Pending(starts_at);
        transaction.upsert_series(stale).unwrap();
        transaction.commit().unwrap();

        let replay = client.settle_due_series(starts_at + day_ms).unwrap();
        assert_eq!(replay.generated_occurrences, 2);
        assert_eq!(replay.generated_tasks, 0);
        assert_eq!(
            client
                .get_tasks(inbox.id)
                .unwrap()
                .iter()
                .filter(|task| {
                    task.series_occurrence
                        .as_ref()
                        .is_some_and(|value| value.series_id == series.id)
                })
                .count(),
            2
        );
    }

    #[test]
    fn template_update_and_delete_do_not_change_copied_task_series() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let template = client
            .save_task_as_template(SaveTemplateCommand {
                task_id: source.id,
                name: "Original".into(),
                default_list_id: Some(inbox.id),
            })
            .unwrap();
        let starts_at = 1_900_000_000_000;
        let series = client
            .create_task_series_from_template(CreateTaskSeriesFromTemplateCommand {
                template_id: template.id,
                rrule: "FREQ=DAILY;COUNT=1".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();
        let copied_blueprint = series.config.blueprint.clone();

        client
            .update_template(UpdateTemplateCommand {
                template_id: template.id,
                name: "Changed".into(),
                default_list_id: Some(inbox.id),
                blueprint: Some(TaskBlueprint {
                    schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                    nodes: vec![TaskBlueprintNode {
                        node_key: "replacement".into(),
                        parent_node_key: None,
                        sibling_order: 0,
                        content: TaskContent {
                            title: "Replacement".into(),
                            note: String::new(),
                            priority: 0,
                            estimated_minutes: None,
                        },
                    }],
                }),
            })
            .unwrap();
        client.delete_template(template.id).unwrap();

        let stored = client
            .get_task_series()
            .unwrap()
            .into_iter()
            .find(|value| value.id == series.id)
            .unwrap();
        assert_eq!(stored.config.blueprint, copied_blueprint);
        client.settle_due_series(starts_at).unwrap();
        let generated = client
            .get_tasks(inbox.id)
            .unwrap()
            .into_iter()
            .find(|task| {
                task.series_occurrence
                    .as_ref()
                    .is_some_and(|value| value.series_id == series.id)
            })
            .unwrap();
        assert_eq!(generated.content.title, source.content.title);
    }

    #[test]
    fn client_creates_and_edits_template_blueprint_directly() {
        let (_temp, client, inbox, _source) = anonymous_client_fixture();
        let template = client
            .create_template(CreateTemplateCommand {
                name: "Release".into(),
                default_list_id: Some(inbox.id),
                blueprint: TaskBlueprint {
                    schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                    nodes: vec![TaskBlueprintNode {
                        node_key: "root".into(),
                        parent_node_key: None,
                        sibling_order: 0,
                        content: TaskContent {
                            title: "Prepare release".into(),
                            note: String::new(),
                            priority: 0,
                            estimated_minutes: None,
                        },
                    }],
                },
            })
            .unwrap();
        let original_revision = template.blueprint_revision.clone();
        let updated = client
            .update_template(UpdateTemplateCommand {
                template_id: template.id,
                name: "Release checklist".into(),
                default_list_id: Some(inbox.id),
                blueprint: Some(TaskBlueprint {
                    schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                    nodes: vec![
                        template.blueprint.nodes[0].clone(),
                        TaskBlueprintNode {
                            node_key: "publish".into(),
                            parent_node_key: Some("root".into()),
                            sibling_order: 0,
                            content: TaskContent {
                                title: "Publish notes".into(),
                                note: String::new(),
                                priority: 0,
                                estimated_minutes: None,
                            },
                        },
                    ],
                }),
            })
            .unwrap();

        assert_eq!(updated.name, "Release checklist");
        assert_eq!(updated.blueprint.nodes.len(), 2);
        assert_ne!(updated.blueprint_revision, original_revision);
    }

    #[test]
    fn client_creates_task_series_directly_from_task_subtree() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let starts_at = 1_900_000_000_000;
        let series = client
            .create_task_series_from_task(CreateTaskSeriesFromTaskCommand {
                task_id: source.id,
                target_list_id: None,
                rrule: "FREQ=WEEKLY;COUNT=1".into(),
                starts_at,
                time_zone: "UTC".into(),
            })
            .unwrap();

        assert_eq!(series.config.target_list_id, Some(inbox.id));
        assert_eq!(series.config.blueprint.nodes.len(), 1);
        assert_eq!(
            series.config.blueprint.nodes[0].content.title,
            source.content.title
        );
    }

    #[test]
    fn manual_instantiation_falls_back_from_archived_default_to_inbox() {
        let (_temp, client, inbox, source) = anonymous_client_fixture();
        let mut archived = new_list("Archived".into(), "a1".into(), source.created_at).unwrap();
        archived.archived_at = Some(source.created_at + 1);
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(archived.clone())
            .unwrap();
        let template = client
            .save_task_as_template(SaveTemplateCommand {
                task_id: source.id,
                name: "Fallback".into(),
                default_list_id: Some(archived.id),
            })
            .unwrap();

        let tasks = client.instantiate_template(template.id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].list_id, inbox.id);
        assert!(tasks[0].series_occurrence.is_none());
        assert_eq!(tasks[0].id.get_version_num(), 7);
    }
}
