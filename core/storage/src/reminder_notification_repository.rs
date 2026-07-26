use crate::*;

const MAX_COMMAND_BATCH: usize = 512;

/// SQLite-backed durable reminder notification command repository.
pub struct SqliteReminderNotificationRepository {
    connection: Connection,
}

impl SqliteReminderNotificationRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl ReminderNotificationRepository for SqliteReminderNotificationRepository {
    fn prepare_reconciliation(
        &mut self,
        now_ms: i64,
    ) -> Result<Vec<ReminderNotificationCommand>, StorageError> {
        let transaction = self.connection.transaction()?;
        rebuild_commands_on(&transaction, now_ms)?;
        let commands = list_commands_on(&transaction, None)?;
        transaction.commit()?;
        Ok(commands)
    }

    fn list_commands(
        &mut self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<ReminderNotificationCommand>, StorageError> {
        if limit == 0 || limit > MAX_COMMAND_BATCH {
            return Err(StorageError::IncompatibleSchema(format!(
                "reminder notification command limit must be between 1 and {MAX_COMMAND_BATCH}"
            )));
        }
        let transaction = self.connection.transaction()?;
        canonicalize_pending_commands_on(&transaction, now_ms)?;
        let commands = list_commands_on(&transaction, Some(limit))?;
        transaction.commit()?;
        Ok(commands)
    }

    fn ack_command(&mut self, reminder_id: Uuid, revision: i64) -> Result<bool, StorageError> {
        let transaction = self.connection.transaction()?;
        let action = transaction
            .query_row(
                "SELECT action
                 FROM reminder_notification_commands
                 WHERE reminder_id = ?1 AND revision = ?2",
                params![reminder_id.to_string(), revision],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(action) = action else {
            transaction.commit()?;
            return Ok(false);
        };
        let deleted = transaction.execute(
            "DELETE FROM reminder_notification_commands
             WHERE reminder_id = ?1 AND revision = ?2",
            params![reminder_id.to_string(), revision],
        )?;
        if deleted > 0 && action == "cancel" {
            transaction.execute(
                "UPDATE reminder_notification_ids
                 SET retired = 1
                 WHERE reminder_id = ?1 AND command_revision = ?2",
                params![reminder_id.to_string(), revision],
            )?;
        }
        transaction.commit()?;
        Ok(deleted > 0)
    }
}

fn rebuild_commands_on(connection: &Connection, now_ms: i64) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO reminder_notification_ids (reminder_id)
         SELECT reminders.id
         FROM reminders
         WHERE NOT EXISTS (
             SELECT 1
             FROM reminder_notification_ids AS mapping
             WHERE mapping.reminder_id = reminders.id
         )
         ORDER BY reminders.id",
        [],
    )?;
    connection.execute(
        "UPDATE reminder_notification_ids
         SET command_revision = command_revision + 1
         WHERE retired = 0
            OR EXISTS (
                SELECT 1
                FROM reminders
                INNER JOIN tasks ON tasks.id = reminders.task_id
                WHERE reminders.id = reminder_notification_ids.reminder_id
                  AND COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
                  AND tasks.status IN ('todo', 'in_progress')
                  AND tasks.deleted_at IS NULL
            )",
        [now_ms],
    )?;
    connection.execute(
        "INSERT INTO reminder_notification_commands (reminder_id, action, revision)
         SELECT mapping.reminder_id,
                CASE WHEN EXISTS (
                    SELECT 1
                    FROM reminders
                    INNER JOIN tasks ON tasks.id = reminders.task_id
                    WHERE reminders.id = mapping.reminder_id
                      AND COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
                      AND tasks.status IN ('todo', 'in_progress')
                      AND tasks.deleted_at IS NULL
                ) THEN 'schedule' ELSE 'cancel' END,
                mapping.command_revision
         FROM reminder_notification_ids AS mapping
         WHERE mapping.retired = 0
            OR EXISTS (
                SELECT 1
                FROM reminders
                INNER JOIN tasks ON tasks.id = reminders.task_id
                WHERE reminders.id = mapping.reminder_id
                  AND COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
                  AND tasks.status IN ('todo', 'in_progress')
                  AND tasks.deleted_at IS NULL
            )
         ON CONFLICT(reminder_id) DO UPDATE SET
             action = excluded.action,
             revision = excluded.revision",
        [now_ms],
    )?;
    Ok(())
}

fn canonicalize_pending_commands_on(
    connection: &Connection,
    now_ms: i64,
) -> Result<(), StorageError> {
    connection.execute(
        "UPDATE reminder_notification_ids
         SET command_revision = command_revision + 1
         WHERE reminder_id IN (
             SELECT command.reminder_id
             FROM reminder_notification_commands AS command
             WHERE command.action != CASE WHEN EXISTS (
                 SELECT 1
                 FROM reminders
                 INNER JOIN tasks ON tasks.id = reminders.task_id
                 WHERE reminders.id = command.reminder_id
                   AND COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
                   AND tasks.status IN ('todo', 'in_progress')
                   AND tasks.deleted_at IS NULL
             ) THEN 'schedule' ELSE 'cancel' END
         )",
        [now_ms],
    )?;
    connection.execute(
        "UPDATE reminder_notification_commands
         SET action = CASE WHEN EXISTS (
                 SELECT 1
                 FROM reminders
                 INNER JOIN tasks ON tasks.id = reminders.task_id
                 WHERE reminders.id = reminder_notification_commands.reminder_id
                   AND COALESCE(reminders.snoozed_until, reminders.remind_at) > ?1
                   AND tasks.status IN ('todo', 'in_progress')
                   AND tasks.deleted_at IS NULL
             ) THEN 'schedule' ELSE 'cancel' END,
             revision = (
                 SELECT command_revision
                 FROM reminder_notification_ids
                 WHERE reminder_id = reminder_notification_commands.reminder_id
             )",
        [now_ms],
    )?;
    Ok(())
}

fn list_commands_on(
    connection: &Connection,
    limit: Option<usize>,
) -> Result<Vec<ReminderNotificationCommand>, StorageError> {
    let limit = limit
        .map(i64::try_from)
        .transpose()
        .map_err(|_| {
            StorageError::IncompatibleSchema(
                "invalid reminder notification command limit".to_string(),
            )
        })?
        .unwrap_or(-1);
    let mut statement = connection.prepare(
        "SELECT command.reminder_id,
                mapping.platform_id,
                command.revision,
                command.action,
                reminders.task_id,
                tasks.list_id,
                COALESCE(reminders.snoozed_until, reminders.remind_at)
         FROM reminder_notification_commands AS command
         INNER JOIN reminder_notification_ids AS mapping
             ON mapping.reminder_id = command.reminder_id
         LEFT JOIN reminders ON reminders.id = command.reminder_id
         LEFT JOIN tasks ON tasks.id = reminders.task_id
         ORDER BY command.revision ASC, command.reminder_id ASC
         LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        let reminder_id = row.get::<_, String>(0)?;
        let platform_id = row.get::<_, i64>(1)?;
        let revision = row.get::<_, i64>(2)?;
        let action = row.get::<_, String>(3)?;
        let task_id = row.get::<_, Option<String>>(4)?;
        let list_id = row.get::<_, Option<String>>(5)?;
        let scheduled_at = row.get::<_, Option<i64>>(6)?;
        Ok((
            reminder_id,
            platform_id,
            revision,
            action,
            task_id,
            list_id,
            scheduled_at,
        ))
    })?;

    rows.map(|row| {
        let (reminder_id, platform_id, revision, action, task_id, list_id, scheduled_at) = row?;
        let reminder_id = Uuid::parse_str(&reminder_id)?;
        let platform_id = i32::try_from(platform_id).map_err(|_| {
            StorageError::IncompatibleSchema(
                "reminder notification platform ID is outside signed 32-bit range".to_string(),
            )
        })?;
        let action = match action.as_str() {
            "schedule" => ReminderNotificationAction::Schedule,
            "cancel" => ReminderNotificationAction::Cancel,
            _ => {
                return Err(StorageError::IncompatibleSchema(
                    "invalid reminder notification command action".to_string(),
                ))
            }
        };
        let task_id = task_id.map(|value| Uuid::parse_str(&value)).transpose()?;
        let list_id = list_id.map(|value| Uuid::parse_str(&value)).transpose()?;
        if action == ReminderNotificationAction::Schedule
            && (task_id.is_none() || list_id.is_none() || scheduled_at.is_none())
        {
            return Err(StorageError::IncompatibleSchema(
                "schedule notification command is missing reminder context".to_string(),
            ));
        }
        Ok(ReminderNotificationCommand {
            reminder_id,
            platform_id,
            revision,
            action,
            task_id,
            list_id,
            scheduled_at,
        })
    })
    .collect()
}
