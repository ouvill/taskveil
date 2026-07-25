CREATE TABLE lists (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    icon TEXT NOT NULL,
    sort_order TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER,
    is_default INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX idx_lists_single_default
    ON lists(is_default)
    WHERE is_default = 1;

CREATE INDEX idx_lists_sort_order ON lists(sort_order);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    list_id TEXT NOT NULL,
    parent_task_id TEXT,
    title TEXT NOT NULL,
    note TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL,
    due_kind TEXT,
    due_on TEXT,
    due_at_ms INTEGER,
    due_time_zone TEXT,
    scheduled_at INTEGER,
    estimated_minutes INTEGER,
    sort_order TEXT NOT NULL,
    completed_at INTEGER,
    closed_reason TEXT,
    deleted_at INTEGER,
    assignee TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    series_id TEXT,
    series_revision TEXT,
    blueprint_node_key TEXT,
    series_occurrence_at INTEGER,
    CHECK (
        (due_kind IS NULL AND due_on IS NULL AND due_at_ms IS NULL AND due_time_zone IS NULL)
        OR (due_kind = 'date' AND due_on IS NOT NULL AND due_at_ms IS NULL AND due_time_zone IS NULL)
        OR (due_kind = 'datetime' AND due_on IS NULL AND due_at_ms IS NOT NULL AND due_time_zone IS NOT NULL)
    ),
    CHECK (
        (series_id IS NULL
            AND series_revision IS NULL
            AND blueprint_node_key IS NULL
            AND series_occurrence_at IS NULL)
        OR
        (series_id IS NOT NULL
            AND series_revision IS NOT NULL
            AND blueprint_node_key IS NOT NULL
            AND series_occurrence_at IS NOT NULL)
    )
);

CREATE INDEX idx_tasks_list_id ON tasks(list_id);
CREATE INDEX idx_tasks_list_sort_order ON tasks(list_id, sort_order, id);
CREATE INDEX idx_tasks_parent_task_id ON tasks(parent_task_id);
CREATE INDEX idx_tasks_deleted_at ON tasks(deleted_at);
CREATE INDEX idx_tasks_home_targets
    ON tasks(due_kind, due_on, due_at_ms, status, completed_at, list_id)
    WHERE due_kind IS NOT NULL;
CREATE INDEX idx_tasks_series_occurrence
    ON tasks(series_id, series_occurrence_at)
    WHERE series_id IS NOT NULL;

CREATE VIRTUAL TABLE tasks_fts USING fts5(
    task_id UNINDEXED,
    title,
    note,
    tokenize = 'unicode61'
);

CREATE TRIGGER tasks_fts_ai
AFTER INSERT ON tasks
WHEN NEW.deleted_at IS NULL
BEGIN
    INSERT INTO tasks_fts(task_id, title, note)
    VALUES (NEW.id, NEW.title, NEW.note);
END;

CREATE TRIGGER tasks_fts_au
AFTER UPDATE OF id, title, note, deleted_at ON tasks
BEGIN
    DELETE FROM tasks_fts WHERE task_id = OLD.id;
    INSERT INTO tasks_fts(task_id, title, note)
    SELECT NEW.id, NEW.title, NEW.note
    WHERE NEW.deleted_at IS NULL;
END;

CREATE TRIGGER tasks_fts_ad
AFTER DELETE ON tasks
BEGIN
    DELETE FROM tasks_fts WHERE task_id = OLD.id;
END;

CREATE TABLE task_undo_entries (
    id TEXT PRIMARY KEY NOT NULL,
    operation_type TEXT NOT NULL,
    task_id TEXT NOT NULL,
    list_id TEXT NOT NULL,
    before_snapshot TEXT NOT NULL,
    after_updated_at INTEGER NOT NULL,
    after_deleted_at INTEGER,
    after_completed_at INTEGER,
    created_at INTEGER NOT NULL,
    consumed_at INTEGER
);

CREATE INDEX idx_task_undo_entries_latest
    ON task_undo_entries(consumed_at, created_at);
CREATE INDEX idx_task_undo_entries_task_id
    ON task_undo_entries(task_id);

CREATE TABLE reminders (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    remind_at INTEGER NOT NULL,
    snoozed_until INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_reminders_task_id ON reminders(task_id);
CREATE INDEX idx_reminders_pending ON reminders(snoozed_until, remind_at);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE local_profile_binding (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    bound_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE local_tenant_root_key_cache (
    tenant_id TEXT PRIMARY KEY NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    wrapped_tenant_root_dek BLOB NOT NULL CHECK (length(wrapped_tenant_root_dek) > 0),
    updated_at INTEGER NOT NULL
);

CREATE TABLE active_timer_session (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    session_id TEXT NOT NULL UNIQUE,
    task_id TEXT,
    mode TEXT NOT NULL CHECK (mode IN ('pomodoro', 'stopwatch')),
    phase TEXT NOT NULL CHECK (phase IN ('work', 'short_break', 'long_break')),
    state TEXT NOT NULL CHECK (state IN ('running', 'paused')),
    started_at INTEGER NOT NULL,
    last_resumed_at INTEGER,
    accumulated_active_ms INTEGER NOT NULL
        CHECK (accumulated_active_ms >= 0 AND accumulated_active_ms <= 604800000),
    target_duration_ms INTEGER
        CHECK (target_duration_ms > 0 AND target_duration_ms <= 604800000),
    updated_at INTEGER NOT NULL,
    CHECK ((phase = 'work' AND task_id IS NOT NULL) OR (phase <> 'work' AND task_id IS NULL)),
    CHECK (mode = 'pomodoro' OR phase = 'work'),
    CHECK (
        (state = 'running' AND last_resumed_at IS NOT NULL)
        OR (state = 'paused' AND last_resumed_at IS NULL)
    )
);

CREATE TABLE timer_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('pomodoro', 'stopwatch')),
    finish_kind TEXT NOT NULL CHECK (finish_kind IN ('completed', 'interrupted')),
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    active_duration_ms INTEGER NOT NULL
        CHECK (active_duration_ms > 0 AND active_duration_ms <= 604800000),
    created_at INTEGER NOT NULL,
    CHECK (started_at <= ended_at),
    CHECK (created_at >= ended_at),
    CHECK (ended_at - started_at <= 604800000),
    CHECK (active_duration_ms <= ended_at - started_at)
);

CREATE INDEX idx_timer_sessions_task
    ON timer_sessions(task_id, started_at, id);

CREATE TABLE templates (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    default_list_id TEXT,
    blueprint_json TEXT NOT NULL,
    blueprint_revision TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_templates_updated ON templates(updated_at, id);

CREATE TABLE task_series (
    id TEXT PRIMARY KEY NOT NULL,
    blueprint_json TEXT NOT NULL,
    target_list_id TEXT,
    rrule TEXT NOT NULL,
    starts_at INTEGER NOT NULL,
    time_zone TEXT NOT NULL,
    next_run_at INTEGER,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    config_revision TEXT NOT NULL,
    config_parent_revision TEXT,
    config_effective_from INTEGER NOT NULL,
    lineage_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_task_series_due
    ON task_series(enabled, next_run_at, id)
    WHERE enabled = 1 AND next_run_at IS NOT NULL;

CREATE TABLE list_aliases (
    alias_list_id TEXT PRIMARY KEY NOT NULL,
    canonical_list_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (alias_list_id <> canonical_list_id)
);

CREATE INDEX idx_list_aliases_canonical
    ON list_aliases(canonical_list_id, alias_list_id);

CREATE TABLE sync_outbox (
    record_id TEXT PRIMARY KEY NOT NULL,
    collection TEXT NOT NULL
        CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
    op_id TEXT NOT NULL UNIQUE,
    base_revision_hlc TEXT,
    revision_hlc TEXT NOT NULL,
    state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
    semantic_hlc TEXT NOT NULL,
    blob BLOB,
    created_at INTEGER NOT NULL,
    CHECK (
        (state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
        OR (state_kind = 'tombstone' AND blob IS NULL)
    )
);

CREATE INDEX idx_sync_outbox_stable_order
    ON sync_outbox(created_at, record_id);

CREATE TABLE sync_record_states (
    record_id TEXT PRIMARY KEY NOT NULL,
    collection TEXT NOT NULL
        CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
    current_revision_hlc TEXT,
    state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
    semantic_hlc TEXT NOT NULL,
    plaintext_json TEXT,
    updated_at INTEGER NOT NULL,
    CHECK (
        (state_kind = 'live' AND plaintext_json IS NOT NULL)
        OR (state_kind = 'tombstone' AND plaintext_json IS NULL)
    )
);

CREATE TABLE sync_cursors (
    name TEXT PRIMARY KEY NOT NULL,
    seq INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE sync_quarantine (
    record_id TEXT PRIMARY KEY NOT NULL,
    collection TEXT NOT NULL
        CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
    seq INTEGER NOT NULL CHECK (seq > 0),
    revision_hlc TEXT NOT NULL,
    state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
    semantic_hlc TEXT NOT NULL,
    blob BLOB,
    reason TEXT NOT NULL CHECK (reason IN (
        'missing_dek',
        'no_matching_dek',
        'authentication_failed',
        'corrupt_envelope',
        'invalid_plaintext',
        'missing_dependency'
    )),
    required_list_id TEXT,
    first_failed_at INTEGER NOT NULL,
    last_failed_at INTEGER NOT NULL,
    attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
    CHECK (
        (state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
        OR (state_kind = 'tombstone' AND blob IS NULL)
    )
);

CREATE INDEX idx_sync_quarantine_seq
    ON sync_quarantine(seq, record_id);

CREATE TABLE sync_full_resync_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    generation_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('base', 'delta', 'sweep')),
    base_seq INTEGER NOT NULL CHECK (base_seq >= 0),
    base_cursor_collection TEXT CHECK (
        base_cursor_collection IS NULL
        OR base_cursor_collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')
    ),
    base_cursor_record_id TEXT,
    delta_cursor INTEGER NOT NULL CHECK (delta_cursor >= 0),
    closure_high_water INTEGER CHECK (closure_high_water >= 0),
    sweep_cursor_collection TEXT CHECK (
        sweep_cursor_collection IS NULL
        OR sweep_cursor_collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')
    ),
    sweep_cursor_record_id TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    continuity_generation INTEGER NOT NULL DEFAULT 0 CHECK (continuity_generation >= 0),
    CHECK (
        (base_cursor_collection IS NULL AND base_cursor_record_id IS NULL)
        OR (base_cursor_collection IS NOT NULL AND base_cursor_record_id IS NOT NULL)
    ),
    CHECK (
        (sweep_cursor_collection IS NULL AND sweep_cursor_record_id IS NULL)
        OR (sweep_cursor_collection IS NOT NULL AND sweep_cursor_record_id IS NOT NULL)
    ),
    CHECK (
        (phase = 'sweep' AND closure_high_water IS NOT NULL)
        OR (phase <> 'sweep' AND closure_high_water IS NULL)
    )
);

CREATE TABLE sync_full_resync_marks (
    generation_id TEXT NOT NULL,
    collection TEXT NOT NULL
        CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
    record_id TEXT NOT NULL,
    PRIMARY KEY (generation_id, collection, record_id)
);

CREATE INDEX idx_sync_full_resync_marks_record
    ON sync_full_resync_marks(generation_id, collection, record_id);

CREATE TABLE sync_record_origins (
    record_id TEXT PRIMARY KEY NOT NULL,
    collection TEXT NOT NULL
        CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
    origin_kind TEXT NOT NULL CHECK (origin_kind IN ('never_synced', 'server_seen')),
    updated_at INTEGER NOT NULL
);
