use crate::*;

// `INDEXED BY` is intentional: fresh databases have no planner statistics and
// otherwise choose the generic deleted_at index. Migration v2 makes these index
// names part of the latest-schema contract, so production and EXPLAIN use the
// same stable plan.
pub(crate) const LIST_HOME_QUERY: &str = "
    WITH RECURSIVE
    home_candidates(id) AS (
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_date_due_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.due_kind = 'date'
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_datetime_due_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.due_kind = 'datetime'
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_scheduled_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.scheduled_at IS NOT NULL
          AND tasks.scheduled_at >= ?1
          AND tasks.scheduled_at < ?2
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_closed_completed_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('done', 'wont_do')
          AND tasks.completed_at IS NOT NULL
          AND tasks.completed_at >= ?1
          AND tasks.completed_at < ?2
    ),
    home_targets(id) AS (
        SELECT tasks.id
        FROM home_candidates
        INNER JOIN tasks ON tasks.id = home_candidates.id
        INNER JOIN lists ON lists.id = tasks.list_id
        WHERE lists.archived_at IS NULL
    ),
    home_scope(id) AS (
        SELECT id FROM home_targets
        UNION
        SELECT child.id
        FROM tasks child
        INNER JOIN home_scope parent ON child.parent_task_id = parent.id
    ),
    home_ancestors(id) AS (
        SELECT tasks.parent_task_id
        FROM tasks
        INNER JOIN home_targets ON home_targets.id = tasks.id
        WHERE tasks.parent_task_id IS NOT NULL
        UNION
        SELECT tasks.parent_task_id
        FROM tasks
        INNER JOIN home_ancestors ancestor ON ancestor.id = tasks.id
        WHERE tasks.parent_task_id IS NOT NULL
    ),
    home_display_scope(id) AS (
        SELECT id FROM home_scope
        UNION
        SELECT id FROM home_ancestors
    )
    SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
           tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on,
           tasks.due_at_ms, tasks.due_time_zone, tasks.scheduled_at,
           tasks.estimated_minutes, tasks.sort_order, tasks.completed_at,
           tasks.closed_reason, tasks.deleted_at, tasks.assignee, tasks.series_id,
           tasks.series_revision, tasks.blueprint_node_key,
           tasks.series_occurrence_at, tasks.created_at, tasks.updated_at,
           lists.name,
           EXISTS(SELECT 1 FROM home_targets WHERE home_targets.id = tasks.id)
    FROM tasks
    INNER JOIN lists ON lists.id = tasks.list_id
    INNER JOIN home_display_scope ON home_display_scope.id = tasks.id
    WHERE lists.archived_at IS NULL
      AND tasks.deleted_at IS NULL
    ORDER BY tasks.due_kind IS NULL ASC,
             CASE tasks.due_kind WHEN 'datetime' THEN 0 WHEN 'date' THEN 1 ELSE 2 END ASC,
             tasks.due_at_ms ASC,
             tasks.due_on ASC,
             tasks.sort_order ASC,
             tasks.id ASC";

pub(crate) const LIST_CALENDAR_OCCURRENCES_QUERY: &str = "
    WITH calendar_targets(id) AS (
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_date_due_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.due_kind = 'date'
          AND tasks.due_on >= ?1
          AND tasks.due_on < ?2
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_datetime_due_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.due_kind = 'datetime'
          AND tasks.due_at_ms >= ?3
          AND tasks.due_at_ms < ?4
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_active_scheduled_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('todo', 'in_progress')
          AND tasks.scheduled_at IS NOT NULL
          AND tasks.scheduled_at >= ?3
          AND tasks.scheduled_at < ?4
        UNION
        SELECT tasks.id
        FROM tasks INDEXED BY idx_tasks_closed_completed_range
        WHERE tasks.deleted_at IS NULL
          AND tasks.status IN ('done', 'wont_do')
          AND tasks.completed_at IS NOT NULL
          AND tasks.completed_at >= ?3
          AND tasks.completed_at < ?4
    )
    SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
           tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on,
           tasks.due_at_ms, tasks.due_time_zone, tasks.scheduled_at,
           tasks.estimated_minutes, tasks.sort_order, tasks.completed_at,
           tasks.closed_reason, tasks.deleted_at, tasks.assignee, tasks.series_id,
           tasks.series_revision, tasks.blueprint_node_key,
           tasks.series_occurrence_at, tasks.created_at, tasks.updated_at,
           lists.name, lists.archived_at
    FROM calendar_targets
    INNER JOIN tasks ON tasks.id = calendar_targets.id
    INNER JOIN lists ON lists.id = tasks.list_id
    ORDER BY tasks.sort_order ASC, tasks.id ASC";

/// SQLite-backed implementation of [`TaskRepository`].
pub struct SqliteTaskRepository {
    connection: Connection,
}

impl SqliteTaskRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn list_subtree_for_sync(&self, task_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_task_subtree_on(&self.connection, task_id)
    }

    pub fn upsert_for_sync(&mut self, task: Task) -> Result<(), StorageError> {
        upsert_task_for_sync_on(&self.connection, task)
    }

    pub fn delete_subtree_for_sync(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        let transaction = self.connection.transaction()?;
        let deleted = delete_task_subtree_on(&transaction, task_id)?;
        transaction.commit()?;
        Ok(deleted)
    }

    /// Updates a task and records the undo snapshot in the same SQLite transaction.
    pub fn update_with_undo(
        &mut self,
        mut before: Task,
        mut after: Task,
        operation_type: TaskUndoOperation,
        created_at: i64,
    ) -> Result<TaskUndoEntry, StorageError> {
        let transaction = self.connection.transaction()?;
        before.list_id = resolve_list_alias_on(&transaction, before.list_id)?;
        after.list_id = resolve_list_alias_on(&transaction, after.list_id)?;
        let entry =
            update_task_with_undo_on(&transaction, before, after, operation_type, created_at)?;
        transaction.commit()?;

        Ok(entry)
    }

    pub fn latest_unconsumed_undo(&self) -> Result<Option<TaskUndoEntry>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, operation_type, task_id, list_id, before_snapshot,
                        after_updated_at, after_deleted_at, after_completed_at,
                        created_at, consumed_at
                 FROM task_undo_entries
                 WHERE consumed_at IS NULL
                   AND operation_type != 'delete'
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                [],
                row_to_task_undo_entry,
            )
            .optional()?
            .transpose()
    }

    pub fn undo_task_operation(
        &mut self,
        undo_id: Uuid,
        consumed_at: i64,
    ) -> Result<Task, StorageError> {
        let transaction = self.connection.transaction()?;
        let restored = undo_task_operation_on(&transaction, undo_id, consumed_at)?;
        transaction.commit()?;

        Ok(restored)
    }
}

impl TaskRepository for SqliteTaskRepository {
    fn get(&self, id: Uuid) -> Result<Task, StorageError> {
        get_task_on(&self.connection, id)
    }

    fn insert(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = resolve_list_alias_on(&self.connection, task.list_id)?;
        insert_task_on(&self.connection, &task)
    }

    fn update(&mut self, mut task: Task) -> Result<(), StorageError> {
        task.list_id = resolve_list_alias_on(&self.connection, task.list_id)?;
        update_task_on(&self.connection, &task)
    }

    fn list_all_for_sync(&self) -> Result<Vec<Task>, StorageError> {
        list_all_tasks_for_sync_on(&self.connection)
    }

    fn list_active_by_list(&self, list_id: Uuid) -> Result<Vec<Task>, StorageError> {
        list_active_tasks_by_list_on(
            &self.connection,
            resolve_list_alias_on(&self.connection, list_id)?,
        )
    }

    fn list_home(
        &self,
        today_start_ms: i64,
        tomorrow_start_ms: i64,
    ) -> Result<Vec<HomeTask>, StorageError> {
        let mut statement = self.connection.prepare(LIST_HOME_QUERY)?;
        let tasks = statement
            .query_map(params![today_start_ms, tomorrow_start_ms], row_to_home_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    }

    fn list_calendar_occurrences(
        &self,
        range: &CalendarRange,
    ) -> Result<Vec<CalendarOccurrence>, StorageError> {
        let mut statement = self.connection.prepare(LIST_CALENDAR_OCCURRENCES_QUERY)?;
        let rows = statement
            .query_map(
                params![
                    range.start_on().as_str(),
                    range.end_on().as_str(),
                    range.start_at().as_millis(),
                    range.end_at().as_millis(),
                ],
                |row| {
                    Ok((
                        row_to_task(row)?,
                        row.get::<_, String>(24)?,
                        row.get::<_, Option<i64>>(25)?.is_some(),
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut occurrences = Vec::new();
        for (task, list_name, list_archived) in rows {
            if matches!(task.status, TaskStatus::Todo | TaskStatus::InProgress) {
                if let Some(due) = task.due.as_ref() {
                    let kind = match due {
                        TaskDue::Date { due_on }
                            if due_on >= range.start_on() && due_on < range.end_on() =>
                        {
                            Some(CalendarOccurrenceKind::DateDue {
                                due_on: due_on.clone(),
                            })
                        }
                        TaskDue::DateTime { due_at, time_zone }
                            if *due_at >= range.start_at() && *due_at < range.end_at() =>
                        {
                            Some(CalendarOccurrenceKind::DateTimeDue {
                                due_at: *due_at,
                                time_zone: time_zone.clone(),
                            })
                        }
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        occurrences.push(CalendarOccurrence {
                            task: task.clone(),
                            list_name: list_name.clone(),
                            list_archived,
                            kind,
                        });
                    }
                }
                if let Some(value) = task.scheduled_at {
                    let scheduled_at = UtcInstant::from_millis(value).map_err(|_| {
                        StorageError::IncompatibleSchema(format!(
                            "task {} contains invalid scheduled_at",
                            task.id
                        ))
                    })?;
                    if scheduled_at >= range.start_at() && scheduled_at < range.end_at() {
                        occurrences.push(CalendarOccurrence {
                            task: task.clone(),
                            list_name: list_name.clone(),
                            list_archived,
                            kind: CalendarOccurrenceKind::Scheduled { scheduled_at },
                        });
                    }
                }
            } else if let Some(value) = task.completed_at {
                let completed_at = UtcInstant::from_millis(value).map_err(|_| {
                    StorageError::IncompatibleSchema(format!(
                        "task {} contains invalid completed_at",
                        task.id
                    ))
                })?;
                if completed_at >= range.start_at() && completed_at < range.end_at() {
                    occurrences.push(CalendarOccurrence {
                        task,
                        list_name,
                        list_archived,
                        kind: CalendarOccurrenceKind::Completed { completed_at },
                    });
                }
            }
        }

        Ok(occurrences)
    }

    fn search_tasks(&self, query: &str) -> Result<Vec<Task>, StorageError> {
        let Some(match_query) = build_fts_prefix_query(query) else {
            return Ok(Vec::new());
        };

        let mut statement = self.connection.prepare(
            "SELECT tasks.id, tasks.list_id, tasks.parent_task_id, tasks.title,
                    tasks.note, tasks.status, tasks.priority, tasks.due_kind, tasks.due_on, tasks.due_at_ms, tasks.due_time_zone,
                    tasks.scheduled_at, tasks.estimated_minutes, tasks.sort_order,
                    tasks.completed_at, tasks.closed_reason, tasks.deleted_at,
                    tasks.assignee, tasks.series_id,
                    tasks.series_revision, tasks.blueprint_node_key,
                    tasks.series_occurrence_at, tasks.created_at, tasks.updated_at
             FROM tasks_fts
             INNER JOIN tasks ON tasks.id = tasks_fts.task_id
             WHERE tasks_fts MATCH ?1
               AND tasks.deleted_at IS NULL
             ORDER BY bm25(tasks_fts) ASC,
                      tasks.updated_at DESC,
                      tasks.id ASC",
        )?;
        let tasks = statement
            .query_map([match_query], row_to_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    }

    fn count_descendants(&self, task_id: Uuid) -> Result<usize, StorageError> {
        count_task_descendants_on(&self.connection, task_id)
    }

    fn delete_subtree(&mut self, task_id: Uuid) -> Result<usize, StorageError> {
        self.get(task_id)?;
        let transaction = self.connection.transaction()?;
        let deleted = delete_task_subtree_on(&transaction, task_id)?;
        transaction.commit()?;
        Ok(deleted)
    }
}

pub(super) fn get_task_on(connection: &Connection, id: Uuid) -> Result<Task, StorageError> {
    let task = connection
        .query_row(
            "SELECT id, list_id, parent_task_id, title, note, status, priority,
                    due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                    completed_at, closed_reason, deleted_at, assignee,
                    series_id, series_revision,
                    blueprint_node_key, series_occurrence_at,
                    created_at, updated_at
             FROM tasks
             WHERE id = ?1",
            [id.to_string()],
            row_to_task,
        )
        .optional()?;

    task.ok_or(StorageError::NotFound(id))
}

pub(super) fn upsert_task_for_sync_on(
    connection: &Connection,
    task: Task,
) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1",
            [task.id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        update_task_on(connection, &task)
    } else {
        insert_task_on(connection, &task)
    }
}

pub(super) fn list_all_tasks_for_sync_on(
    connection: &Connection,
) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         ORDER BY created_at ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(tasks)
}

pub(super) fn list_active_tasks_by_list_on(
    connection: &Connection,
    list_id: Uuid,
) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         WHERE list_id = ?1
         ORDER BY sort_order ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([list_id.to_string()], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tasks)
}

pub(super) fn list_task_subtree_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<Vec<Task>, StorageError> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE subtree(id) AS (
             SELECT id FROM tasks WHERE id = ?1
             UNION ALL
             SELECT tasks.id
             FROM tasks
             INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         SELECT id, list_id, parent_task_id, title, note, status, priority,
                due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                completed_at, closed_reason, deleted_at, assignee,
                series_id, series_revision,
                blueprint_node_key, series_occurrence_at,
                created_at, updated_at
         FROM tasks
         WHERE id IN (SELECT id FROM subtree)
         ORDER BY sort_order ASC, id ASC",
    )?;
    let tasks = statement
        .query_map([task_id.to_string()], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tasks)
}

pub(super) fn insert_task_on(connection: &Connection, task: &Task) -> Result<(), StorageError> {
    let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
    connection.execute(
        "INSERT INTO tasks (
            id, list_id, parent_task_id, title, note, status, priority,
            due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
            completed_at, closed_reason, deleted_at, assignee,
            series_id, series_revision,
            blueprint_node_key, series_occurrence_at,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24
        )",
        params![
            task.id.to_string(),
            task.list_id.to_string(),
            task.parent_task_id.map(|id| id.to_string()),
            task.content.title,
            task.content.note,
            status_to_str(task.status),
            task.content.priority,
            due_kind,
            due_on,
            due_at_ms,
            due_time_zone,
            task.scheduled_at,
            task.content.estimated_minutes,
            task.sort_order,
            task.completed_at,
            task.closed_reason,
            task.deleted_at,
            task.assignee.map(|id| id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_revision.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.blueprint_node_key.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.occurrence_at),
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

pub(super) fn update_task_on(connection: &Connection, task: &Task) -> Result<(), StorageError> {
    let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
    let changed = connection.execute(
        "UPDATE tasks
         SET list_id = ?2,
             parent_task_id = ?3,
             title = ?4,
             note = ?5,
             status = ?6,
             priority = ?7,
             due_kind = ?8,
             due_on = ?9,
             due_at_ms = ?10,
             due_time_zone = ?11,
             scheduled_at = ?12,
             estimated_minutes = ?13,
             sort_order = ?14,
             completed_at = ?15,
             closed_reason = ?16,
             deleted_at = ?17,
             assignee = ?18,
             series_id = ?19,
             series_revision = ?20,
             blueprint_node_key = ?21,
             series_occurrence_at = ?22,
             created_at = ?23,
             updated_at = ?24
         WHERE id = ?1",
        params![
            task.id.to_string(),
            task.list_id.to_string(),
            task.parent_task_id.map(|id| id.to_string()),
            task.content.title,
            task.content.note,
            status_to_str(task.status),
            task.content.priority,
            due_kind,
            due_on,
            due_at_ms,
            due_time_zone,
            task.scheduled_at,
            task.content.estimated_minutes,
            task.sort_order,
            task.completed_at,
            task.closed_reason,
            task.deleted_at,
            task.assignee.map(|id| id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_id.to_string()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.series_revision.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.blueprint_node_key.as_str()),
            task.series_occurrence
                .as_ref()
                .map(|provenance| provenance.occurrence_at),
            task.created_at,
            task.updated_at,
        ],
    )?;

    if changed == 0 {
        return Err(StorageError::NotFound(task.id));
    }

    Ok(())
}

pub(super) fn update_task_with_undo_on(
    connection: &Connection,
    before: Task,
    after: Task,
    operation_type: TaskUndoOperation,
    created_at: i64,
) -> Result<TaskUndoEntry, StorageError> {
    let entry = TaskUndoEntry {
        id: Uuid::now_v7(),
        operation_type,
        task_id: before.id,
        list_id: before.list_id,
        before_snapshot: before,
        after_updated_at: after.updated_at,
        after_deleted_at: after.deleted_at,
        after_completed_at: after.completed_at,
        created_at,
        consumed_at: None,
    };

    update_task_on(connection, &after)?;
    insert_task_undo_on(connection, &entry)?;
    Ok(entry)
}

pub(super) fn undo_task_operation_on(
    connection: &Connection,
    undo_id: Uuid,
    consumed_at: i64,
) -> Result<Task, StorageError> {
    let entry = connection
        .query_row(
            "SELECT id, operation_type, task_id, list_id, before_snapshot,
                    after_updated_at, after_deleted_at, after_completed_at,
                    created_at, consumed_at
             FROM task_undo_entries
             WHERE id = ?1",
            [undo_id.to_string()],
            row_to_task_undo_entry,
        )
        .optional()?
        .transpose()?
        .ok_or(StorageError::NotFound(undo_id))?;

    if entry.consumed_at.is_some() {
        return Err(StorageError::UndoConsumed(undo_id));
    }

    let current = get_task_on(connection, entry.task_id)?;
    if current.updated_at != entry.after_updated_at
        || current.deleted_at != entry.after_deleted_at
        || current.completed_at != entry.after_completed_at
    {
        return Err(StorageError::UndoConflict(entry.task_id));
    }

    update_task_on(connection, &entry.before_snapshot)?;
    let changed = connection.execute(
        "UPDATE task_undo_entries
         SET consumed_at = ?2
         WHERE id = ?1 AND consumed_at IS NULL",
        params![undo_id.to_string(), consumed_at],
    )?;
    if changed == 0 {
        return Err(StorageError::UndoConsumed(undo_id));
    }

    Ok(entry.before_snapshot)
}

fn build_fts_prefix_query(query: &str) -> Option<String> {
    let terms = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn count_task_descendants_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<usize, StorageError> {
    let count: i64 = connection.query_row(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE parent_task_id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         SELECT count(*) FROM subtree",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|_| {
        StorageError::IncompatibleSchema("task descendant count exceeded usize".to_string())
    })
}

pub(super) fn delete_task_subtree_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<usize, StorageError> {
    connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM task_undo_entries
         WHERE task_id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM reminders
         WHERE task_id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    let deleted = connection.execute(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM tasks WHERE id = ?1
            UNION ALL
            SELECT tasks.id
            FROM tasks
            INNER JOIN subtree ON tasks.parent_task_id = subtree.id
         )
         DELETE FROM tasks
         WHERE id IN (SELECT id FROM subtree)",
        [task_id.to_string()],
    )?;
    Ok(deleted)
}

fn insert_task_undo_on(connection: &Connection, entry: &TaskUndoEntry) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO task_undo_entries (
            id, operation_type, task_id, list_id, before_snapshot,
            after_updated_at, after_deleted_at, after_completed_at,
            created_at, consumed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
        )",
        params![
            entry.id.to_string(),
            undo_operation_to_str(entry.operation_type),
            entry.task_id.to_string(),
            entry.list_id.to_string(),
            serde_json::to_string(&entry.before_snapshot)?,
            entry.after_updated_at,
            entry.after_deleted_at,
            entry.after_completed_at,
            entry.created_at,
            entry.consumed_at,
        ],
    )?;
    Ok(())
}
