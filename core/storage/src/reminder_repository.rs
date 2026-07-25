use crate::*;

/// SQLite-backed implementation of [`ReminderRepository`].
pub struct SqliteReminderRepository {
    connection: Connection,
}

impl SqliteReminderRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl ReminderRepository for SqliteReminderRepository {
    fn create_task_reminder(
        &mut self,
        task_id: Uuid,
        remind_at: i64,
        created_at: i64,
    ) -> Result<Reminder, StorageError> {
        ensure_task_open_for_reminder(&self.connection, task_id)?;
        validate_reminder_time(remind_at, created_at)?;
        let reminder = Reminder {
            id: Uuid::now_v7(),
            task_id,
            remind_at,
            snoozed_until: None,
            created_at,
        };
        let transaction = self.connection.transaction()?;
        let reminder_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM reminders WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )?;
        if reminder_count >= MAX_REMINDERS_PER_TASK as i64 {
            return Err(StorageError::ReminderLimitReached {
                limit: MAX_REMINDERS_PER_TASK,
            });
        }
        ensure_unique_reminder_time_on(&transaction, task_id, remind_at, None)?;
        insert_reminder_on(&transaction, &reminder)?;
        transaction.commit()?;
        Ok(reminder)
    }

    fn update_reminder(
        &mut self,
        reminder_id: Uuid,
        remind_at: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        validate_reminder_time(remind_at, updated_at)?;
        let current = get_reminder_on(&self.connection, reminder_id)?;
        ensure_task_open_for_reminder(&self.connection, current.task_id)?;
        let transaction = self.connection.transaction()?;
        ensure_unique_reminder_time_on(
            &transaction,
            current.task_id,
            remind_at,
            Some(reminder_id),
        )?;
        transaction.execute(
            "UPDATE reminders
             SET remind_at = ?2, snoozed_until = NULL
             WHERE id = ?1",
            params![reminder_id.to_string(), remind_at],
        )?;
        let reminder = get_reminder_on(&transaction, reminder_id)?;
        transaction.commit()?;
        Ok(reminder)
    }

    fn delete_reminder(&mut self, reminder_id: Uuid) -> Result<Reminder, StorageError> {
        let reminder = get_reminder_on(&self.connection, reminder_id)?;
        self.connection.execute(
            "DELETE FROM reminders WHERE id = ?1",
            [reminder_id.to_string()],
        )?;
        Ok(reminder)
    }

    fn clear_task_reminders(&mut self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        let reminders = list_task_reminders_on(&self.connection, task_id)?;
        delete_task_reminders_on(&self.connection, task_id)?;
        Ok(reminders)
    }

    fn list_task_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        list_task_reminders_on(&self.connection, task_id)
    }

    fn list_task_subtree_reminders(&self, task_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        ensure_task_exists(&self.connection, task_id)?;
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT id FROM tasks WHERE id = ?1
                 UNION ALL
                 SELECT tasks.id
                 FROM tasks
                 INNER JOIN subtree ON tasks.parent_task_id = subtree.id
             )
             SELECT id, task_id, remind_at, snoozed_until, created_at
             FROM reminders
             WHERE task_id IN (SELECT id FROM subtree)
             ORDER BY COALESCE(snoozed_until, remind_at) ASC, created_at ASC, id ASC",
        )?;
        let reminders = statement
            .query_map([task_id.to_string()], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn list_list_reminders(&self, list_id: Uuid) -> Result<Vec<Reminder>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT reminders.id, reminders.task_id, reminders.remind_at,
                    reminders.snoozed_until, reminders.created_at
             FROM reminders
             INNER JOIN tasks ON tasks.id = reminders.task_id
             WHERE tasks.list_id = ?1
             ORDER BY COALESCE(reminders.snoozed_until, reminders.remind_at) ASC,
                      reminders.created_at ASC,
                      reminders.id ASC",
        )?;
        let reminders = statement
            .query_map([list_id.to_string()], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn list_pending_reminders(&self, now_ms: i64) -> Result<Vec<Reminder>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT reminders.id, reminders.task_id, reminders.remind_at,
                    reminders.snoozed_until, reminders.created_at
             FROM reminders
             INNER JOIN tasks ON tasks.id = reminders.task_id
             WHERE COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
               AND tasks.status IN ('todo', 'in_progress')
               AND tasks.deleted_at IS NULL
             ORDER BY COALESCE(reminders.snoozed_until, reminders.remind_at) ASC,
                      reminders.created_at ASC,
                      reminders.id ASC",
        )?;
        let reminders = statement
            .query_map([now_ms], row_to_reminder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reminders)
    }

    fn snooze_reminder(
        &mut self,
        reminder_id: Uuid,
        snoozed_until: i64,
        updated_at: i64,
    ) -> Result<Reminder, StorageError> {
        let reminder = get_reminder_on(&self.connection, reminder_id)?;
        ensure_task_open_for_reminder(&self.connection, reminder.task_id)?;
        validate_reminder_time(snoozed_until, updated_at)?;
        let changed = self.connection.execute(
            "UPDATE reminders
             SET snoozed_until = ?2
             WHERE id = ?1",
            params![reminder_id.to_string(), snoozed_until],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound(reminder_id));
        }
        self.connection
            .query_row(
                "SELECT id, task_id, remind_at, snoozed_until, created_at
                 FROM reminders
                 WHERE id = ?1",
                [reminder_id.to_string()],
                row_to_reminder,
            )
            .map_err(StorageError::from)
    }
}

fn ensure_task_exists(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?1 LIMIT 1",
            [task_id.to_string()],
            |_| Ok(()),
        )
        .optional()?;
    exists.ok_or(StorageError::NotFound(task_id))
}

fn list_task_reminders_on(
    connection: &Connection,
    task_id: Uuid,
) -> Result<Vec<Reminder>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT id, task_id, remind_at, snoozed_until, created_at
         FROM reminders
         WHERE task_id = ?1
         ORDER BY COALESCE(snoozed_until, remind_at) ASC, created_at ASC, id ASC",
    )?;
    let reminders = statement
        .query_map([task_id.to_string()], row_to_reminder)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(reminders)
}

fn get_reminder_on(connection: &Connection, reminder_id: Uuid) -> Result<Reminder, StorageError> {
    connection
        .query_row(
            "SELECT id, task_id, remind_at, snoozed_until, created_at
             FROM reminders
             WHERE id = ?1",
            [reminder_id.to_string()],
            row_to_reminder,
        )
        .optional()?
        .ok_or(StorageError::NotFound(reminder_id))
}

fn ensure_task_open_for_reminder(
    connection: &Connection,
    task_id: Uuid,
) -> Result<(), StorageError> {
    let state = connection
        .query_row(
            "SELECT status, deleted_at FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?
        .ok_or(StorageError::NotFound(task_id))?;
    if matches!(state.0.as_str(), "todo" | "in_progress") && state.1.is_none() {
        Ok(())
    } else {
        Err(StorageError::ReminderTaskClosed)
    }
}

fn validate_reminder_time(remind_at: i64, now_ms: i64) -> Result<(), StorageError> {
    if remind_at > now_ms {
        Ok(())
    } else {
        Err(StorageError::ReminderTimeNotFuture)
    }
}

fn ensure_unique_reminder_time_on(
    connection: &Connection,
    task_id: Uuid,
    remind_at: i64,
    excluding_id: Option<Uuid>,
) -> Result<(), StorageError> {
    let duplicate: bool = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM reminders
             WHERE task_id = ?1
               AND remind_at = ?2
               AND (?3 IS NULL OR id != ?3)
         )",
        params![
            task_id.to_string(),
            remind_at,
            excluding_id.map(|id| id.to_string())
        ],
        |row| row.get(0),
    )?;
    if duplicate {
        Err(StorageError::DuplicateReminderTime)
    } else {
        Ok(())
    }
}

fn insert_reminder_on(connection: &Connection, reminder: &Reminder) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO reminders (id, task_id, remind_at, snoozed_until, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            reminder.id.to_string(),
            reminder.task_id.to_string(),
            reminder.remind_at,
            reminder.snoozed_until,
            reminder.created_at,
        ],
    )?;
    Ok(())
}

fn delete_task_reminders_on(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    connection.execute(
        "DELETE FROM reminders WHERE task_id = ?1",
        [task_id.to_string()],
    )?;
    Ok(())
}
