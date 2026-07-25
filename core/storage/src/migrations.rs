use rusqlite::{Connection, Transaction};

use super::{StorageError, LATEST_SCHEMA_VERSION};

pub(super) const SCHEMA: &str = include_str!("schema.sql");
pub(super) const BASELINE_SCHEMA_VERSION: i32 = 1;

pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        target_version: 2,
        name: "add_lists_archived_at",
        apply: add_lists_archived_at,
    },
    Migration {
        target_version: 3,
        name: "add_lists_is_default",
        apply: add_lists_is_default,
    },
    Migration {
        target_version: 4,
        name: "rebuild_tasks_fts_triggers",
        apply: rebuild_tasks_fts_triggers,
    },
    Migration {
        target_version: 5,
        name: "add_settings",
        apply: add_settings,
    },
    Migration {
        target_version: 6,
        name: "add_reminders",
        apply: add_reminders,
    },
    Migration {
        target_version: 7,
        name: "add_performance_indexes",
        apply: add_performance_indexes,
    },
    Migration {
        target_version: 8,
        name: "add_sync_outbox_and_cursors",
        apply: add_sync_outbox_and_cursors,
    },
    Migration {
        target_version: 9,
        name: "add_sync_record_states",
        apply: add_sync_record_states,
    },
    Migration {
        target_version: 10,
        name: "add_local_crypto_cache",
        apply: add_local_crypto_cache,
    },
    Migration {
        target_version: 11,
        name: "replace_sync_metadata_v2",
        apply: replace_sync_metadata_v2,
    },
    Migration {
        target_version: 12,
        name: "normalize_fixed_width_ranks",
        apply: normalize_fixed_width_ranks,
    },
    Migration {
        target_version: 13,
        name: "add_sync_quarantine",
        apply: add_sync_quarantine,
    },
    Migration {
        target_version: 14,
        name: "reserved_schema_v14",
        apply: reserved_schema_v14,
    },
    Migration {
        target_version: 15,
        name: "add_full_resync_state",
        apply: add_full_resync_state,
    },
    Migration {
        target_version: 16,
        name: "add_archive_first_rebase_state",
        apply: add_archive_first_rebase_state,
    },
    Migration {
        target_version: 17,
        name: "replace_task_due_semantics",
        apply: replace_task_due_semantics,
    },
    Migration {
        target_version: 18,
        name: "add_timer_sync_foundation",
        apply: add_timer_sync_foundation,
    },
    Migration {
        target_version: 19,
        name: "add_list_aliases",
        apply: add_list_aliases,
    },
    Migration {
        target_version: 20,
        name: "add_template_recurrence_foundation",
        apply: add_template_recurrence_foundation,
    },
    Migration {
        target_version: 21,
        name: "finalize_tenant_record_boundary",
        apply: finalize_tenant_record_boundary,
    },
    Migration {
        target_version: 22,
        name: "redesign_task_templates_and_series",
        apply: redesign_task_templates_and_series,
    },
];

#[derive(Clone, Copy)]
pub(super) struct Migration {
    pub(super) target_version: i32,
    pub(super) name: &'static str,
    pub(super) apply: fn(&Transaction<'_>) -> rusqlite::Result<()>,
}

pub(super) fn ensure_schema(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<(), StorageError> {
    let mut user_version =
        read_user_version(connection).map_err(|_| StorageError::InvalidDatabaseKey)?;
    if user_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion {
            found: user_version,
            latest: LATEST_SCHEMA_VERSION,
        });
    }

    if user_version == 0 {
        user_version = ensure_baseline_schema(connection)?;
    }

    if user_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion {
            found: user_version,
            latest: LATEST_SCHEMA_VERSION,
        });
    }

    apply_pending_migrations(connection, user_version, migrations)?;
    Ok(())
}

pub(super) fn read_user_version(connection: &Connection) -> rusqlite::Result<i32> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn ensure_baseline_schema(connection: &mut Connection) -> Result<i32, StorageError> {
    if has_user_schema_objects(connection)? {
        validate_baseline_v1_schema(connection)?;
    }

    let transaction = connection.transaction()?;
    transaction.execute_batch(SCHEMA)?;
    set_user_version(&transaction, BASELINE_SCHEMA_VERSION)?;
    transaction.commit()?;

    Ok(BASELINE_SCHEMA_VERSION)
}

pub(super) fn apply_pending_migrations(
    connection: &mut Connection,
    current_version: i32,
    migrations: &[Migration],
) -> Result<(), StorageError> {
    if current_version == LATEST_SCHEMA_VERSION {
        return Ok(());
    }

    let pending = migrations
        .iter()
        .filter(|migration| migration.target_version > current_version)
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Err(StorageError::IncompatibleSchema(format!(
            "missing migration from version {current_version} to {LATEST_SCHEMA_VERSION}"
        )));
    }

    for (expected_version, migration) in (current_version + 1..).zip(pending.iter()) {
        if migration.target_version != expected_version {
            return Err(StorageError::IncompatibleSchema(format!(
                "missing migration to version {expected_version}"
            )));
        }
    }

    let transaction = connection.transaction()?;
    let mut final_migration = pending[0];
    for migration in pending {
        final_migration = migration;
        (migration.apply)(&transaction).map_err(|source| StorageError::MigrationFailed {
            target_version: migration.target_version,
            migration: migration.name,
            source,
        })?;
        set_user_version(&transaction, migration.target_version).map_err(|source| {
            StorageError::MigrationFailed {
                target_version: migration.target_version,
                migration: migration.name,
                source,
            }
        })?;
    }
    transaction
        .commit()
        .map_err(|source| StorageError::MigrationFailed {
            target_version: final_migration.target_version,
            migration: final_migration.name,
            source,
        })?;

    Ok(())
}

pub(super) fn set_user_version(connection: &Connection, version: i32) -> rusqlite::Result<()> {
    connection.execute_batch(&format!("PRAGMA user_version = {version};"))
}

pub(super) fn add_lists_archived_at(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch("ALTER TABLE lists ADD COLUMN archived_at INTEGER NULL;")
}

pub(super) fn add_lists_is_default(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "ALTER TABLE lists ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0;
         UPDATE lists
         SET is_default = 1
         WHERE id = (
             SELECT id
             FROM lists
             WHERE archived_at IS NULL
             ORDER BY sort_order ASC, id ASC
             LIMIT 1
         );
         CREATE UNIQUE INDEX idx_lists_single_default
             ON lists(is_default)
             WHERE is_default = 1;",
    )
}

pub(super) fn rebuild_tasks_fts_triggers(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "DROP TRIGGER IF EXISTS tasks_fts_ai;
         DROP TRIGGER IF EXISTS tasks_fts_au;
         DROP TRIGGER IF EXISTS tasks_fts_au_delete;
         DROP TRIGGER IF EXISTS tasks_fts_au_insert;
         DROP TRIGGER IF EXISTS tasks_fts_ad;
         DROP TABLE IF EXISTS tasks_fts;

         CREATE VIRTUAL TABLE tasks_fts USING fts5(
             task_id UNINDEXED,
             title,
             note,
             tokenize = 'unicode61'
         );

         INSERT INTO tasks_fts(task_id, title, note)
         SELECT id, title, note
         FROM tasks
         WHERE deleted_at IS NULL;

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
         END;",
    )
}

pub(super) fn add_settings(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL,
             updated_at INTEGER NOT NULL
         );",
    )
}

pub(super) fn add_reminders(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS reminders (
             id TEXT PRIMARY KEY NOT NULL,
             task_id TEXT NOT NULL,
             remind_at INTEGER NOT NULL,
             snoozed_until INTEGER,
             created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_reminders_task_id ON reminders(task_id);
         CREATE INDEX IF NOT EXISTS idx_reminders_pending
             ON reminders(snoozed_until, remind_at);",
    )
}

pub(super) fn add_performance_indexes(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tasks_list_sort_order
             ON tasks(list_id, sort_order, id);",
    )?;
    if table_columns_raw(transaction, "tasks")?
        .iter()
        .any(|column| column == "due_kind")
    {
        transaction.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tasks_home_targets
                 ON tasks(due_kind, due_on, due_at_ms, status, completed_at, list_id)
                 WHERE due_kind IS NOT NULL;",
        )
    } else {
        transaction.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tasks_home_targets
                 ON tasks(due_at, status, completed_at, list_id)
                 WHERE due_at IS NOT NULL;",
        )
    }
}

fn replace_task_due_semantics(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let columns = table_columns_raw(transaction, "tasks")?;
    if columns.iter().any(|column| column == "due_kind") {
        return Ok(());
    }
    let task_count: i64 =
        transaction.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
    if task_count != 0 {
        return Err(rusqlite::Error::InvalidParameterName(
            "task-101 requires recreating profiles that contain legacy due_at values".to_string(),
        ));
    }
    transaction.execute_batch(
        "DROP TRIGGER IF EXISTS tasks_fts_ai;
         DROP TRIGGER IF EXISTS tasks_fts_au;
         DROP TRIGGER IF EXISTS tasks_fts_ad;
         DROP TABLE IF EXISTS tasks_fts;
         DROP INDEX IF EXISTS idx_tasks_home_targets;
         DROP INDEX IF EXISTS idx_tasks_list_id;
         DROP INDEX IF EXISTS idx_tasks_list_sort_order;
         DROP INDEX IF EXISTS idx_tasks_parent_task_id;
         DROP INDEX IF EXISTS idx_tasks_deleted_at;
         ALTER TABLE tasks RENAME TO tasks_legacy_due;
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
             CHECK (
                 (due_kind IS NULL AND due_on IS NULL AND due_at_ms IS NULL AND due_time_zone IS NULL)
                 OR (due_kind = 'date' AND due_on IS NOT NULL AND due_at_ms IS NULL AND due_time_zone IS NULL)
                 OR (due_kind = 'datetime' AND due_on IS NULL AND due_at_ms IS NOT NULL AND due_time_zone IS NOT NULL)
             )
         );
         DROP TABLE tasks_legacy_due;
         CREATE INDEX idx_tasks_list_id ON tasks(list_id);
         CREATE INDEX idx_tasks_list_sort_order ON tasks(list_id, sort_order, id);
         CREATE INDEX idx_tasks_parent_task_id ON tasks(parent_task_id);
         CREATE INDEX idx_tasks_deleted_at ON tasks(deleted_at);
         CREATE INDEX idx_tasks_home_targets
             ON tasks(due_kind, due_on, due_at_ms, status, completed_at, list_id)
             WHERE due_kind IS NOT NULL;",
    )?;
    rebuild_tasks_fts_triggers(transaction)
}

/// Protocol v5 expands the strict collection enum without changing the
/// encrypted envelope. Existing transport heads, tombstones, cursors, and
/// origins are copied into tables with the expanded CHECK constraints.
fn add_timer_sync_foundation(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    // Some pre-release synthetic profiles declared a later user_version while
    // containing domain tables only. Materialize the empty v17 transport shape
    // before the preserving rebuild so those profiles remain migratable.
    if table_columns_raw(transaction, "sync_outbox")?.is_empty() {
        replace_sync_metadata_v2(transaction)?;
        add_sync_quarantine(transaction)?;
        add_full_resync_state(transaction)?;
        add_archive_first_rebase_state(transaction)?;
    }
    transaction.execute_batch(
        "CREATE TABLE local_tenant_root_key_cache (
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
             accumulated_active_ms INTEGER NOT NULL CHECK (accumulated_active_ms >= 0 AND accumulated_active_ms <= 604800000),
             target_duration_ms INTEGER CHECK (target_duration_ms > 0 AND target_duration_ms <= 604800000),
             updated_at INTEGER NOT NULL,
             CHECK ((phase = 'work' AND task_id IS NOT NULL) OR (phase <> 'work' AND task_id IS NULL)),
             CHECK (mode = 'pomodoro' OR phase = 'work'),
             CHECK ((state = 'running' AND last_resumed_at IS NOT NULL) OR (state = 'paused' AND last_resumed_at IS NULL))
         );
         CREATE TABLE timer_sessions (
             id TEXT PRIMARY KEY NOT NULL,
             task_id TEXT NOT NULL,
             mode TEXT NOT NULL CHECK (mode IN ('pomodoro', 'stopwatch')),
             finish_kind TEXT NOT NULL CHECK (finish_kind IN ('completed', 'interrupted')),
             started_at INTEGER NOT NULL,
             ended_at INTEGER NOT NULL,
             active_duration_ms INTEGER NOT NULL CHECK (active_duration_ms > 0 AND active_duration_ms <= 604800000),
             created_at INTEGER NOT NULL,
             CHECK (started_at <= ended_at),
             CHECK (created_at >= ended_at),
             CHECK (ended_at - started_at <= 604800000),
             CHECK (active_duration_ms <= ended_at - started_at)
         );
         CREATE INDEX idx_timer_sessions_task ON timer_sessions(task_id, started_at, id);

         DROP INDEX IF EXISTS idx_sync_outbox_stable_order;
         DROP INDEX IF EXISTS idx_sync_quarantine_seq;
         DROP INDEX IF EXISTS idx_sync_full_resync_marks_record;
         ALTER TABLE sync_outbox RENAME TO sync_outbox_v17;
         ALTER TABLE sync_record_states RENAME TO sync_record_states_v17;
         ALTER TABLE sync_cursors RENAME TO sync_cursors_v17;
         ALTER TABLE sync_quarantine RENAME TO sync_quarantine_v17;
         ALTER TABLE sync_full_resync_marks RENAME TO sync_full_resync_marks_v17;
         ALTER TABLE sync_full_resync_state RENAME TO sync_full_resync_state_v17;
         ALTER TABLE sync_record_origins RENAME TO sync_record_origins_v17;

         CREATE TABLE sync_outbox (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'timer_sessions')),
             op_id TEXT NOT NULL UNIQUE,
             base_revision_hlc TEXT,
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             created_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_outbox_stable_order ON sync_outbox(created_at, record_id);
         CREATE TABLE sync_record_states (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'timer_sessions')),
             current_revision_hlc TEXT,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             plaintext_json TEXT,
             updated_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND plaintext_json IS NOT NULL)
                    OR (state_kind = 'tombstone' AND plaintext_json IS NULL))
         );
         CREATE TABLE sync_cursors (
             name TEXT PRIMARY KEY NOT NULL,
             seq INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE sync_quarantine (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'timer_sessions')),
             seq INTEGER NOT NULL CHECK (seq > 0),
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             reason TEXT NOT NULL CHECK (reason IN (
                 'missing_dek', 'no_matching_dek', 'authentication_failed',
                 'corrupt_envelope', 'invalid_plaintext', 'missing_dependency'
             )),
             required_list_id TEXT,
             first_failed_at INTEGER NOT NULL,
             last_failed_at INTEGER NOT NULL,
             attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_quarantine_seq ON sync_quarantine(seq, record_id);
         CREATE TABLE sync_full_resync_state (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             generation_id TEXT NOT NULL,
             phase TEXT NOT NULL CHECK (phase IN ('base', 'delta', 'sweep')),
             base_seq INTEGER NOT NULL CHECK (base_seq >= 0),
             base_cursor_collection TEXT CHECK (base_cursor_collection IS NULL OR base_cursor_collection IN ('lists', 'tasks', 'timer_sessions')),
             base_cursor_record_id TEXT,
             delta_cursor INTEGER NOT NULL CHECK (delta_cursor >= 0),
             closure_high_water INTEGER CHECK (closure_high_water >= 0),
             sweep_cursor_collection TEXT CHECK (sweep_cursor_collection IS NULL OR sweep_cursor_collection IN ('lists', 'tasks', 'timer_sessions')),
             sweep_cursor_record_id TEXT,
             started_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             continuity_generation INTEGER NOT NULL DEFAULT 0 CHECK (continuity_generation >= 0),
             CHECK ((base_cursor_collection IS NULL AND base_cursor_record_id IS NULL)
                    OR (base_cursor_collection IS NOT NULL AND base_cursor_record_id IS NOT NULL)),
             CHECK ((sweep_cursor_collection IS NULL AND sweep_cursor_record_id IS NULL)
                    OR (sweep_cursor_collection IS NOT NULL AND sweep_cursor_record_id IS NOT NULL)),
             CHECK ((phase = 'sweep' AND closure_high_water IS NOT NULL)
                    OR (phase <> 'sweep' AND closure_high_water IS NULL))
         );
         CREATE TABLE sync_full_resync_marks (
             generation_id TEXT NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'timer_sessions')),
             record_id TEXT NOT NULL,
             PRIMARY KEY (generation_id, collection, record_id)
         );
         CREATE INDEX idx_sync_full_resync_marks_record ON sync_full_resync_marks(generation_id, collection, record_id);
         CREATE TABLE sync_record_origins (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'timer_sessions')),
             origin_kind TEXT NOT NULL CHECK (origin_kind IN ('never_synced', 'server_seen')),
             updated_at INTEGER NOT NULL
         );

         INSERT INTO sync_outbox SELECT * FROM sync_outbox_v17;
         INSERT INTO sync_record_states SELECT * FROM sync_record_states_v17;
         INSERT INTO sync_cursors SELECT * FROM sync_cursors_v17;
         INSERT INTO sync_quarantine SELECT * FROM sync_quarantine_v17;
         INSERT INTO sync_full_resync_state SELECT * FROM sync_full_resync_state_v17;
         INSERT INTO sync_full_resync_marks SELECT * FROM sync_full_resync_marks_v17;
         INSERT INTO sync_record_origins SELECT * FROM sync_record_origins_v17;
         DROP TABLE sync_outbox_v17;
         DROP TABLE sync_record_states_v17;
         DROP TABLE sync_cursors_v17;
         DROP TABLE sync_quarantine_v17;
         DROP TABLE sync_full_resync_marks_v17;
         DROP TABLE sync_full_resync_state_v17;
         DROP TABLE sync_record_origins_v17;",
    )
}

/// Durable device-local aliases used by canonical Inbox convergence.
///
/// Alias rows never replace the authenticated list sync record. They only hide
/// loser domain rows and resolve stale local references to the current
/// canonical list. The convergence transaction replaces the complete set.
fn add_list_aliases(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE list_aliases (
             alias_list_id TEXT PRIMARY KEY NOT NULL,
             canonical_list_id TEXT NOT NULL,
             updated_at INTEGER NOT NULL,
             CHECK (alias_list_id <> canonical_list_id)
         );
         CREATE INDEX idx_list_aliases_canonical
             ON list_aliases(canonical_list_id, alias_list_id);",
    )
}

fn finalize_tenant_record_boundary(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "DROP TABLE IF EXISTS pending_list_key_bundles;
         DROP TABLE IF EXISTS local_list_key_bundles;",
    )?;
    if table_columns_raw(transaction, "lists")?
        .iter()
        .any(|column| column == "org_id")
    {
        transaction.execute_batch("ALTER TABLE lists DROP COLUMN org_id;")?;
    }
    Ok(())
}

/// Pre-release breaking reset from template-bound schedules to independent Task Series.
///
/// Existing tasks are preserved as manual tasks. Template/schedule rows and sync metadata
/// are development data and are reset so protocol v8 can seed typed records from local state.
fn redesign_task_templates_and_series(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "UPDATE tasks
            SET recurrence_schedule_id = NULL,
                recurrence_schedule_revision = NULL,
                recurrence_template_revision = NULL,
                recurrence_occurrence_at = NULL;
         DROP INDEX IF EXISTS idx_tasks_recurrence_occurrence;
         ALTER TABLE tasks RENAME COLUMN recurrence_schedule_id TO series_id;
         ALTER TABLE tasks RENAME COLUMN recurrence_schedule_revision TO series_revision;
         ALTER TABLE tasks RENAME COLUMN recurrence_template_revision TO blueprint_node_key;
         ALTER TABLE tasks RENAME COLUMN recurrence_occurrence_at TO series_occurrence_at;
         CREATE INDEX idx_tasks_series_occurrence
             ON tasks(series_id, series_occurrence_at)
             WHERE series_id IS NOT NULL;

         DROP TABLE IF EXISTS schedules;
         DROP TABLE IF EXISTS templates;
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

         DROP INDEX IF EXISTS idx_sync_outbox_stable_order;
         DROP INDEX IF EXISTS idx_sync_quarantine_seq;
         DROP INDEX IF EXISTS idx_sync_full_resync_marks_record;
         DROP TABLE sync_outbox;
         DROP TABLE sync_record_states;
         DROP TABLE sync_quarantine;
         DROP TABLE sync_full_resync_marks;
         DROP TABLE sync_full_resync_state;
         DROP TABLE sync_record_origins;
         DELETE FROM sync_cursors;

         CREATE TABLE sync_outbox (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             op_id TEXT NOT NULL UNIQUE,
             base_revision_hlc TEXT,
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             created_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_outbox_stable_order ON sync_outbox(created_at, record_id);

         CREATE TABLE sync_record_states (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             current_revision_hlc TEXT,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             plaintext_json TEXT,
             updated_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND plaintext_json IS NOT NULL)
                    OR (state_kind = 'tombstone' AND plaintext_json IS NULL))
         );

         CREATE TABLE sync_quarantine (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             seq INTEGER NOT NULL CHECK (seq > 0),
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             reason TEXT NOT NULL CHECK (reason IN (
                 'missing_dek', 'no_matching_dek', 'authentication_failed',
                 'corrupt_envelope', 'invalid_plaintext', 'missing_dependency'
             )),
             required_list_id TEXT,
             first_failed_at INTEGER NOT NULL,
             last_failed_at INTEGER NOT NULL,
             attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_quarantine_seq ON sync_quarantine(seq, record_id);

         CREATE TABLE sync_full_resync_state (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             generation_id TEXT NOT NULL,
             phase TEXT NOT NULL CHECK (phase IN ('base', 'delta', 'sweep')),
             base_seq INTEGER NOT NULL CHECK (base_seq >= 0),
             base_cursor_collection TEXT CHECK (base_cursor_collection IS NULL OR base_cursor_collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             base_cursor_record_id TEXT,
             delta_cursor INTEGER NOT NULL CHECK (delta_cursor >= 0),
             closure_high_water INTEGER CHECK (closure_high_water >= 0),
             sweep_cursor_collection TEXT CHECK (sweep_cursor_collection IS NULL OR sweep_cursor_collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             sweep_cursor_record_id TEXT,
             started_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             continuity_generation INTEGER NOT NULL DEFAULT 0 CHECK (continuity_generation >= 0),
             CHECK ((base_cursor_collection IS NULL AND base_cursor_record_id IS NULL)
                    OR (base_cursor_collection IS NOT NULL AND base_cursor_record_id IS NOT NULL)),
             CHECK ((sweep_cursor_collection IS NULL AND sweep_cursor_record_id IS NULL)
                    OR (sweep_cursor_collection IS NOT NULL AND sweep_cursor_record_id IS NOT NULL)),
             CHECK ((phase = 'sweep' AND closure_high_water IS NOT NULL)
                    OR (phase <> 'sweep' AND closure_high_water IS NULL))
         );

         CREATE TABLE sync_full_resync_marks (
             generation_id TEXT NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             record_id TEXT NOT NULL,
             PRIMARY KEY (generation_id, collection, record_id)
         );
         CREATE INDEX idx_sync_full_resync_marks_record
             ON sync_full_resync_marks(generation_id, collection, record_id);

         CREATE TABLE sync_record_origins (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'task_series', 'timer_sessions')),
             origin_kind TEXT NOT NULL CHECK (origin_kind IN ('never_synced', 'server_seen')),
             updated_at INTEGER NOT NULL
         );",
    )
}

/// Protocol v6 adds tenant-scoped template and schedule records while keeping
/// every v5 transport head, tombstone, cursor, quarantine row, and resync mark.
fn add_template_recurrence_foundation(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let task_columns = table_columns_raw(transaction, "tasks")?;
    if !task_columns
        .iter()
        .any(|column| column == "recurrence_schedule_id")
    {
        transaction.execute_batch(
            "ALTER TABLE tasks ADD COLUMN recurrence_schedule_id TEXT;
             ALTER TABLE tasks ADD COLUMN recurrence_schedule_revision TEXT;
             ALTER TABLE tasks ADD COLUMN recurrence_template_revision TEXT;
             ALTER TABLE tasks ADD COLUMN recurrence_occurrence_at INTEGER
                 CHECK (
                     (recurrence_schedule_id IS NULL
                      AND recurrence_schedule_revision IS NULL
                      AND recurrence_template_revision IS NULL
                      AND recurrence_occurrence_at IS NULL)
                     OR
                     (recurrence_schedule_id IS NOT NULL
                      AND recurrence_schedule_revision IS NOT NULL
                      AND recurrence_template_revision IS NOT NULL
                      AND recurrence_occurrence_at IS NOT NULL)
                 );",
        )?;
    }
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tasks_recurrence_occurrence
             ON tasks(recurrence_schedule_id, recurrence_occurrence_at)
             WHERE recurrence_schedule_id IS NOT NULL;

         CREATE TABLE templates (
             id TEXT PRIMARY KEY NOT NULL,
             name TEXT NOT NULL,
             default_list_id TEXT,
             snapshot_json TEXT NOT NULL,
             snapshot_revision TEXT NOT NULL,
             snapshot_parent_revision TEXT,
             snapshot_effective_from INTEGER NOT NULL,
             lineage_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE INDEX idx_templates_updated ON templates(updated_at, id);

         CREATE TABLE schedules (
             id TEXT PRIMARY KEY NOT NULL,
             template_id TEXT NOT NULL,
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
         CREATE INDEX idx_schedules_template ON schedules(template_id, updated_at, id);
         CREATE INDEX idx_schedules_due
             ON schedules(enabled, next_run_at, id)
             WHERE enabled = 1 AND next_run_at IS NOT NULL;

         DROP INDEX IF EXISTS idx_sync_outbox_stable_order;
         DROP INDEX IF EXISTS idx_sync_quarantine_seq;
         DROP INDEX IF EXISTS idx_sync_full_resync_marks_record;
         ALTER TABLE sync_outbox RENAME TO sync_outbox_v19;
         ALTER TABLE sync_record_states RENAME TO sync_record_states_v19;
         ALTER TABLE sync_cursors RENAME TO sync_cursors_v19;
         ALTER TABLE sync_quarantine RENAME TO sync_quarantine_v19;
         ALTER TABLE sync_full_resync_marks RENAME TO sync_full_resync_marks_v19;
         ALTER TABLE sync_full_resync_state RENAME TO sync_full_resync_state_v19;
         ALTER TABLE sync_record_origins RENAME TO sync_record_origins_v19;

         CREATE TABLE sync_outbox (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             op_id TEXT NOT NULL UNIQUE,
             base_revision_hlc TEXT,
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             created_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_outbox_stable_order ON sync_outbox(created_at, record_id);
         CREATE TABLE sync_record_states (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             current_revision_hlc TEXT,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             plaintext_json TEXT,
             updated_at INTEGER NOT NULL,
             CHECK ((state_kind = 'live' AND plaintext_json IS NOT NULL)
                    OR (state_kind = 'tombstone' AND plaintext_json IS NULL))
         );
         CREATE TABLE sync_cursors (
             name TEXT PRIMARY KEY NOT NULL,
             seq INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE sync_quarantine (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             seq INTEGER NOT NULL CHECK (seq > 0),
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             reason TEXT NOT NULL CHECK (reason IN (
                 'missing_dek', 'no_matching_dek', 'authentication_failed',
                 'corrupt_envelope', 'invalid_plaintext', 'missing_dependency'
             )),
             required_list_id TEXT,
             first_failed_at INTEGER NOT NULL,
             last_failed_at INTEGER NOT NULL,
             attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
             CHECK ((state_kind = 'live' AND blob IS NOT NULL AND length(blob) > 0)
                    OR (state_kind = 'tombstone' AND blob IS NULL))
         );
         CREATE INDEX idx_sync_quarantine_seq ON sync_quarantine(seq, record_id);
         CREATE TABLE sync_full_resync_state (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             generation_id TEXT NOT NULL,
             phase TEXT NOT NULL CHECK (phase IN ('base', 'delta', 'sweep')),
             base_seq INTEGER NOT NULL CHECK (base_seq >= 0),
             base_cursor_collection TEXT CHECK (base_cursor_collection IS NULL OR base_cursor_collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             base_cursor_record_id TEXT,
             delta_cursor INTEGER NOT NULL CHECK (delta_cursor >= 0),
             closure_high_water INTEGER CHECK (closure_high_water >= 0),
             sweep_cursor_collection TEXT CHECK (sweep_cursor_collection IS NULL OR sweep_cursor_collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             sweep_cursor_record_id TEXT,
             started_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             continuity_generation INTEGER NOT NULL DEFAULT 0 CHECK (continuity_generation >= 0),
             CHECK ((base_cursor_collection IS NULL AND base_cursor_record_id IS NULL)
                    OR (base_cursor_collection IS NOT NULL AND base_cursor_record_id IS NOT NULL)),
             CHECK ((sweep_cursor_collection IS NULL AND sweep_cursor_record_id IS NULL)
                    OR (sweep_cursor_collection IS NOT NULL AND sweep_cursor_record_id IS NOT NULL)),
             CHECK ((phase = 'sweep' AND closure_high_water IS NOT NULL)
                    OR (phase <> 'sweep' AND closure_high_water IS NULL))
         );
         CREATE TABLE sync_full_resync_marks (
             generation_id TEXT NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             record_id TEXT NOT NULL,
             PRIMARY KEY (generation_id, collection, record_id)
         );
         CREATE INDEX idx_sync_full_resync_marks_record ON sync_full_resync_marks(generation_id, collection, record_id);
         CREATE TABLE sync_record_origins (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks', 'templates', 'schedules', 'timer_sessions')),
             origin_kind TEXT NOT NULL CHECK (origin_kind IN ('never_synced', 'server_seen')),
             updated_at INTEGER NOT NULL
         );

         INSERT INTO sync_outbox SELECT * FROM sync_outbox_v19;
         INSERT INTO sync_record_states SELECT * FROM sync_record_states_v19;
         INSERT INTO sync_cursors SELECT * FROM sync_cursors_v19;
         INSERT INTO sync_quarantine SELECT * FROM sync_quarantine_v19;
         INSERT INTO sync_full_resync_state SELECT * FROM sync_full_resync_state_v19;
         INSERT INTO sync_full_resync_marks SELECT * FROM sync_full_resync_marks_v19;
         INSERT INTO sync_record_origins SELECT * FROM sync_record_origins_v19;
         DROP TABLE sync_outbox_v19;
         DROP TABLE sync_record_states_v19;
         DROP TABLE sync_cursors_v19;
         DROP TABLE sync_quarantine_v19;
         DROP TABLE sync_full_resync_marks_v19;
         DROP TABLE sync_full_resync_state_v19;
         DROP TABLE sync_record_origins_v19;",
    )
}

pub(super) fn table_columns_raw(
    connection: &Connection,
    table: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns)
}

pub(super) fn add_sync_outbox_and_cursors(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_outbox (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             record_id TEXT NOT NULL,
             collection TEXT NOT NULL,
             hlc TEXT NOT NULL,
             deleted INTEGER NOT NULL DEFAULT 0,
             blob BLOB NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_sync_outbox_stable_order
             ON sync_outbox(created_at, id);
         CREATE TABLE IF NOT EXISTS sync_cursors (
             name TEXT PRIMARY KEY NOT NULL,
             seq INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );",
    )
}

fn has_user_schema_objects(connection: &Connection) -> Result<bool, StorageError> {
    let count: i64 = connection.query_row(
        "SELECT count(*)
         FROM sqlite_master
         WHERE type IN ('table', 'view')
           AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn validate_baseline_v1_schema(connection: &Connection) -> Result<(), StorageError> {
    for (table, required_columns) in BASELINE_V1_COLUMNS {
        let columns = table_columns(connection, table)?;
        if columns.is_empty() {
            return Err(StorageError::IncompatibleSchema(format!(
                "missing baseline v1 table {table}"
            )));
        }

        for required_column in *required_columns {
            if !columns.iter().any(|column| column == required_column) {
                return Err(StorageError::IncompatibleSchema(format!(
                    "missing baseline v1 column {table}.{required_column}"
                )));
            }
        }
    }

    let list_columns = table_columns(connection, "lists")?;
    if list_columns.iter().any(|column| column == "archived_at") {
        return Err(StorageError::IncompatibleSchema(
            "lists.archived_at exists while user_version is 0".to_string(),
        ));
    }
    if list_columns.iter().any(|column| column == "is_default") {
        return Err(StorageError::IncompatibleSchema(
            "lists.is_default exists while user_version is 0".to_string(),
        ));
    }

    Ok(())
}

pub(super) fn add_sync_record_states(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_record_states (
             record_id TEXT NOT NULL,
             collection TEXT NOT NULL,
             plaintext_json TEXT NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (collection, record_id)
         );",
    )
}

pub(super) fn add_local_crypto_cache(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE local_profile_binding (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             tenant_id TEXT NOT NULL,
             user_id TEXT NOT NULL,
             device_id TEXT NOT NULL,
             bound_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         INSERT INTO local_profile_binding (
             singleton, tenant_id, user_id, device_id, bound_at, updated_at
         )
         SELECT 1,
                tenant.value,
                account_user.value,
                device.value,
                MIN(tenant.updated_at, account_user.updated_at, device.updated_at),
                MAX(tenant.updated_at, account_user.updated_at, device.updated_at)
         FROM settings AS tenant,
              settings AS account_user,
              settings AS device
         WHERE tenant.key = 'account_tenant_id'
           AND account_user.key = 'account_user_id'
           AND device.key = 'account_device_id'
           AND trim(tenant.value) <> ''
           AND trim(account_user.value) <> ''
           AND trim(device.value) <> ''
         LIMIT 1;",
    )
}

/// Protocol v2 is intentionally destructive for local sync metadata.
/// Domain rows and the account-bound crypto cache are left untouched; callers
/// regenerate v2 seed heads after opening the migrated profile.
fn replace_sync_metadata_v2(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "DROP TABLE IF EXISTS sync_outbox;
         DROP TABLE IF EXISTS sync_record_states;
         DROP TABLE IF EXISTS sync_cursors;

         CREATE TABLE sync_outbox (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
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
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
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
         );",
    )
}

/// Pre-release destructive rank migration. Domain order is preserved while all
/// sync metadata is discarded so the caller can seed strict typed payloads.
fn normalize_fixed_width_ranks(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "WITH ranked AS (
             SELECT id,
                    printf('%016x%016x', row_number() OVER (ORDER BY sort_order, id), 0) AS rank
             FROM lists
         )
         UPDATE lists
         SET sort_order = (SELECT rank FROM ranked WHERE ranked.id = lists.id);

         WITH ranked AS (
             SELECT id,
                    printf('%016x%016x',
                           row_number() OVER (
                               PARTITION BY list_id, parent_task_id
                               ORDER BY sort_order, id
                           ),
                           0) AS rank
             FROM tasks
         )
         UPDATE tasks
         SET sort_order = (SELECT rank FROM ranked WHERE ranked.id = tasks.id);",
    )?;
    replace_sync_metadata_v2(transaction)
}

fn add_sync_quarantine(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE sync_quarantine (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
             seq INTEGER NOT NULL CHECK (seq > 0),
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             reason TEXT NOT NULL CHECK (reason IN (
                 'missing_dek', 'no_matching_dek', 'authentication_failed',
                 'corrupt_envelope', 'invalid_plaintext'
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
             ON sync_quarantine(seq, record_id);",
    )
}

fn reserved_schema_v14(_transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    Ok(())
}

fn add_full_resync_state(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE sync_full_resync_state (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             generation_id TEXT NOT NULL,
             phase TEXT NOT NULL CHECK (phase IN ('base', 'delta', 'sweep')),
             base_seq INTEGER NOT NULL CHECK (base_seq >= 0),
             base_cursor_collection TEXT CHECK (
                 base_cursor_collection IS NULL
                 OR base_cursor_collection IN ('lists', 'tasks')
             ),
             base_cursor_record_id TEXT,
             delta_cursor INTEGER NOT NULL CHECK (delta_cursor >= 0),
             closure_high_water INTEGER CHECK (closure_high_water >= 0),
             sweep_cursor_collection TEXT CHECK (
                 sweep_cursor_collection IS NULL
                 OR sweep_cursor_collection IN ('lists', 'tasks')
             ),
             sweep_cursor_record_id TEXT,
             started_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
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
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
             record_id TEXT NOT NULL,
             PRIMARY KEY (generation_id, collection, record_id)
         );
         CREATE INDEX idx_sync_full_resync_marks_record
             ON sync_full_resync_marks(generation_id, collection, record_id);",
    )
}

fn add_archive_first_rebase_state(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE sync_record_origins (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
             origin_kind TEXT NOT NULL CHECK (origin_kind IN ('never_synced', 'server_seen')),
             updated_at INTEGER NOT NULL
         );
         INSERT INTO sync_record_origins (record_id, collection, origin_kind, updated_at)
         SELECT record_id, collection,
                CASE WHEN current_revision_hlc IS NULL THEN 'never_synced' ELSE 'server_seen' END,
                updated_at
         FROM sync_record_states;
         ALTER TABLE sync_full_resync_state
             ADD COLUMN continuity_generation INTEGER NOT NULL DEFAULT 0
             CHECK (continuity_generation >= 0);

         ALTER TABLE sync_quarantine RENAME TO sync_quarantine_v15;
         CREATE TABLE sync_quarantine (
             record_id TEXT PRIMARY KEY NOT NULL,
             collection TEXT NOT NULL CHECK (collection IN ('lists', 'tasks')),
             seq INTEGER NOT NULL CHECK (seq > 0),
             revision_hlc TEXT NOT NULL,
             state_kind TEXT NOT NULL CHECK (state_kind IN ('live', 'tombstone')),
             semantic_hlc TEXT NOT NULL,
             blob BLOB,
             reason TEXT NOT NULL CHECK (reason IN (
                 'missing_dek', 'no_matching_dek', 'authentication_failed',
                 'corrupt_envelope', 'invalid_plaintext', 'missing_dependency'
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
         INSERT INTO sync_quarantine SELECT * FROM sync_quarantine_v15;
         DROP TABLE sync_quarantine_v15;
         CREATE INDEX idx_sync_quarantine_seq ON sync_quarantine(seq, record_id);",
    )
}

const BASELINE_V1_COLUMNS: &[(&str, &[&str])] = &[
    (
        "tasks",
        &[
            "id",
            "list_id",
            "parent_task_id",
            "title",
            "note",
            "status",
            "priority",
            "due_kind",
            "due_on",
            "due_at_ms",
            "due_time_zone",
            "scheduled_at",
            "estimated_minutes",
            "sort_order",
            "completed_at",
            "closed_reason",
            "deleted_at",
            "assignee",
            "created_at",
            "updated_at",
        ],
    ),
    (
        "lists",
        &[
            "id",
            "name",
            "color",
            "icon",
            "org_id",
            "sort_order",
            "created_at",
            "updated_at",
        ],
    ),
    (
        "task_undo_entries",
        &[
            "id",
            "operation_type",
            "task_id",
            "list_id",
            "before_snapshot",
            "after_updated_at",
            "after_deleted_at",
            "after_completed_at",
            "created_at",
            "consumed_at",
        ],
    ),
    ("tasks_fts", &["title", "note"]),
];

pub(super) fn table_columns(
    connection: &Connection,
    table: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns)
}
