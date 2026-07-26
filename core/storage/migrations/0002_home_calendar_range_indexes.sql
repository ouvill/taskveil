DROP INDEX idx_tasks_home_targets;

CREATE INDEX idx_tasks_active_date_due_range
    ON tasks(due_on, id)
    WHERE deleted_at IS NULL
      AND status IN ('todo', 'in_progress')
      AND due_kind = 'date';

CREATE INDEX idx_tasks_active_datetime_due_range
    ON tasks(due_at_ms, id)
    WHERE deleted_at IS NULL
      AND status IN ('todo', 'in_progress')
      AND due_kind = 'datetime';

CREATE INDEX idx_tasks_active_scheduled_range
    ON tasks(scheduled_at, id)
    WHERE deleted_at IS NULL
      AND status IN ('todo', 'in_progress')
      AND scheduled_at IS NOT NULL;

CREATE INDEX idx_tasks_closed_completed_range
    ON tasks(completed_at, id)
    WHERE deleted_at IS NULL
      AND status IN ('done', 'wont_do')
      AND completed_at IS NOT NULL;
