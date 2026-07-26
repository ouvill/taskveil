CREATE TABLE reminder_notification_ids (
    platform_id INTEGER PRIMARY KEY AUTOINCREMENT
        CHECK (platform_id BETWEEN 1 AND 2147483647),
    reminder_id TEXT NOT NULL UNIQUE,
    command_revision INTEGER NOT NULL DEFAULT 0
        CHECK (command_revision >= 0),
    retired INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1))
);

CREATE TABLE reminder_notification_commands (
    reminder_id TEXT PRIMARY KEY NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('schedule', 'cancel')),
    revision INTEGER NOT NULL CHECK (revision > 0)
);

INSERT INTO reminder_notification_ids (reminder_id)
SELECT id
FROM reminders
ORDER BY id;

UPDATE reminder_notification_ids
SET command_revision = 1;

INSERT INTO reminder_notification_commands (reminder_id, action, revision)
SELECT mapping.reminder_id,
       CASE
           WHEN tasks.status IN ('todo', 'in_progress')
                AND tasks.deleted_at IS NULL
           THEN 'schedule'
           ELSE 'cancel'
       END,
       mapping.command_revision
FROM reminder_notification_ids AS mapping
LEFT JOIN reminders ON reminders.id = mapping.reminder_id
LEFT JOIN tasks ON tasks.id = reminders.task_id;

CREATE TRIGGER reminders_notification_ai
AFTER INSERT ON reminders
BEGIN
    INSERT INTO reminder_notification_ids (reminder_id)
    VALUES (NEW.id)
    ON CONFLICT(reminder_id) DO NOTHING;

    UPDATE reminder_notification_ids
    SET command_revision = command_revision + 1,
        retired = 0
    WHERE reminder_id = NEW.id;

    INSERT INTO reminder_notification_commands (reminder_id, action, revision)
    SELECT mapping.reminder_id,
           CASE
               WHEN tasks.status IN ('todo', 'in_progress')
                    AND tasks.deleted_at IS NULL
               THEN 'schedule'
               ELSE 'cancel'
           END,
           mapping.command_revision
    FROM reminder_notification_ids AS mapping
    INNER JOIN tasks ON tasks.id = NEW.task_id
    WHERE mapping.reminder_id = NEW.id
    ON CONFLICT(reminder_id) DO UPDATE SET
        action = excluded.action,
        revision = excluded.revision;
END;

CREATE TRIGGER reminders_notification_au
AFTER UPDATE OF task_id, remind_at, snoozed_until ON reminders
BEGIN
    UPDATE reminder_notification_ids
    SET command_revision = command_revision + 1,
        retired = 0
    WHERE reminder_id = NEW.id;

    INSERT INTO reminder_notification_commands (reminder_id, action, revision)
    SELECT mapping.reminder_id,
           CASE
               WHEN tasks.status IN ('todo', 'in_progress')
                    AND tasks.deleted_at IS NULL
               THEN 'schedule'
               ELSE 'cancel'
           END,
           mapping.command_revision
    FROM reminder_notification_ids AS mapping
    INNER JOIN tasks ON tasks.id = NEW.task_id
    WHERE mapping.reminder_id = NEW.id
    ON CONFLICT(reminder_id) DO UPDATE SET
        action = excluded.action,
        revision = excluded.revision;
END;

CREATE TRIGGER reminders_notification_ad
AFTER DELETE ON reminders
BEGIN
    UPDATE reminder_notification_ids
    SET command_revision = command_revision + 1,
        retired = 0
    WHERE reminder_id = OLD.id;

    INSERT INTO reminder_notification_commands (reminder_id, action, revision)
    SELECT reminder_id, 'cancel', command_revision
    FROM reminder_notification_ids
    WHERE reminder_id = OLD.id
    ON CONFLICT(reminder_id) DO UPDATE SET
        action = excluded.action,
        revision = excluded.revision;
END;

CREATE TRIGGER tasks_notification_au
AFTER UPDATE OF status, deleted_at, list_id ON tasks
WHEN OLD.status IS NOT NEW.status
     OR OLD.deleted_at IS NOT NEW.deleted_at
     OR OLD.list_id IS NOT NEW.list_id
BEGIN
    UPDATE reminder_notification_ids
    SET command_revision = command_revision + 1,
        retired = 0
    WHERE reminder_id IN (
        SELECT id FROM reminders WHERE task_id = NEW.id
    );

    INSERT INTO reminder_notification_commands (reminder_id, action, revision)
    SELECT mapping.reminder_id,
           CASE
               WHEN NEW.status IN ('todo', 'in_progress')
                    AND NEW.deleted_at IS NULL
               THEN 'schedule'
               ELSE 'cancel'
           END,
           mapping.command_revision
    FROM reminder_notification_ids AS mapping
    INNER JOIN reminders ON reminders.id = mapping.reminder_id
    WHERE reminders.task_id = NEW.id
    ON CONFLICT(reminder_id) DO UPDATE SET
        action = excluded.action,
        revision = excluded.revision;
END;

CREATE TRIGGER tasks_notification_ad
AFTER DELETE ON tasks
BEGIN
    UPDATE reminder_notification_ids
    SET command_revision = command_revision + 1,
        retired = 0
    WHERE reminder_id IN (
        SELECT id FROM reminders WHERE task_id = OLD.id
    );

    INSERT INTO reminder_notification_commands (reminder_id, action, revision)
    SELECT mapping.reminder_id, 'cancel', mapping.command_revision
    FROM reminder_notification_ids AS mapping
    INNER JOIN reminders ON reminders.id = mapping.reminder_id
    WHERE reminders.task_id = OLD.id
    ON CONFLICT(reminder_id) DO UPDATE SET
        action = excluded.action,
        revision = excluded.revision;
END;
