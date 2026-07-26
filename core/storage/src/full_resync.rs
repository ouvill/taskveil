use crate::*;

pub(super) fn load_full_resync_on(
    connection: &Connection,
) -> Result<Option<FullResyncProgress>, StorageError> {
    let row = connection
        .query_row(
            "SELECT generation_id, continuity_generation, phase, base_seq,
                    base_cursor_collection, base_cursor_record_id,
                    delta_cursor, closure_high_water,
                    sweep_cursor_collection, sweep_cursor_record_id,
                    started_at, updated_at
             FROM sync_full_resync_state
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        generation_id,
        continuity_generation,
        phase,
        base_seq,
        base_cursor_collection,
        base_cursor_record_id,
        delta_cursor,
        closure_high_water,
        sweep_cursor_collection,
        sweep_cursor_record_id,
        started_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    let phase = match phase.as_str() {
        "base" => FullResyncPhase::Base,
        "delta" => FullResyncPhase::Delta,
        "sweep" => FullResyncPhase::Sweep,
        _ => return Err(StorageError::InvalidSyncState(phase)),
    };
    Ok(Some(FullResyncProgress {
        generation_id: Uuid::parse_str(&generation_id)?,
        continuity_generation,
        phase,
        base_seq,
        base_cursor: parse_full_resync_cursor(base_cursor_collection, base_cursor_record_id)?,
        delta_cursor,
        closure_high_water,
        sweep_cursor: parse_full_resync_cursor(sweep_cursor_collection, sweep_cursor_record_id)?,
        started_at,
        updated_at,
    }))
}

fn parse_full_resync_cursor(
    collection: Option<String>,
    record_id: Option<String>,
) -> Result<Option<FullResyncStableCursor>, StorageError> {
    match (collection, record_id) {
        (None, None) => Ok(None),
        (Some(collection), Some(record_id)) => {
            validate_sync_collection(&collection)?;
            Ok(Some(FullResyncStableCursor {
                collection,
                record_id: Uuid::parse_str(&record_id)?,
            }))
        }
        _ => Err(StorageError::IncompatibleSchema(
            "full resync cursor is incomplete".to_string(),
        )),
    }
}

pub(super) fn start_full_resync_on(
    connection: &Connection,
    generation_id: Uuid,
    continuity_generation: i64,
    base_seq: i64,
    now_ms: i64,
) -> Result<FullResyncProgress, StorageError> {
    if base_seq < 0 || continuity_generation <= 0 {
        return Err(StorageError::IncompatibleSchema(
            "full resync base sequence is negative".to_string(),
        ));
    }
    if let Some(progress) = load_full_resync_on(connection)? {
        return Ok(progress);
    }
    connection.execute(
        "INSERT INTO sync_full_resync_state (
             singleton, generation_id, continuity_generation, phase, base_seq,
             base_cursor_collection, base_cursor_record_id,
             delta_cursor, closure_high_water,
             sweep_cursor_collection, sweep_cursor_record_id,
             started_at, updated_at
         ) VALUES (1, ?1, ?2, 'base', ?3, NULL, NULL, ?3, NULL, NULL, NULL, ?4, ?4)",
        params![
            generation_id.to_string(),
            continuity_generation,
            base_seq,
            now_ms
        ],
    )?;
    load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state was not persisted".to_string())
    })
}

fn require_full_resync(
    connection: &Connection,
    generation_id: Uuid,
    expected_phase: FullResyncPhase,
) -> Result<FullResyncProgress, StorageError> {
    let progress = load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state is missing".to_string())
    })?;
    if progress.generation_id != generation_id || progress.phase != expected_phase {
        return Err(StorageError::IncompatibleSchema(
            "full resync generation or phase mismatch".to_string(),
        ));
    }
    Ok(progress)
}

pub(super) fn mark_full_resync_record_on(
    connection: &Connection,
    generation_id: Uuid,
    collection: &str,
    record_id: Uuid,
) -> Result<(), StorageError> {
    validate_sync_collection(collection)?;
    let progress = load_full_resync_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync state is missing".to_string())
    })?;
    if progress.generation_id != generation_id || progress.phase == FullResyncPhase::Sweep {
        return Err(StorageError::IncompatibleSchema(
            "full resync mark does not belong to an active scan".to_string(),
        ));
    }
    connection.execute(
        "INSERT OR IGNORE INTO sync_full_resync_marks (
             generation_id, collection, record_id
         ) VALUES (?1, ?2, ?3)",
        params![generation_id.to_string(), collection, record_id.to_string()],
    )?;
    Ok(())
}

pub(super) fn advance_full_resync_base_on(
    connection: &Connection,
    generation_id: Uuid,
    next_cursor: Option<&FullResyncStableCursor>,
    base_complete: bool,
    now_ms: i64,
) -> Result<(), StorageError> {
    require_full_resync(connection, generation_id, FullResyncPhase::Base)?;
    if let Some(cursor) = next_cursor {
        validate_sync_collection(&cursor.collection)?;
    }
    if !base_complete && next_cursor.is_none() {
        return Err(StorageError::IncompatibleSchema(
            "incomplete base page has no continuation cursor".to_string(),
        ));
    }
    let (collection, record_id) = next_cursor
        .map(|cursor| {
            (
                Some(cursor.collection.as_str()),
                Some(cursor.record_id.to_string()),
            )
        })
        .unwrap_or((None, None));
    let phase = if base_complete { "delta" } else { "base" };
    connection.execute(
        "UPDATE sync_full_resync_state
         SET phase = ?2,
             base_cursor_collection = ?3,
             base_cursor_record_id = ?4,
             updated_at = ?5
         WHERE singleton = 1 AND generation_id = ?1",
        params![
            generation_id.to_string(),
            phase,
            collection,
            record_id,
            now_ms
        ],
    )?;
    Ok(())
}

pub(super) fn advance_full_resync_delta_on(
    connection: &Connection,
    generation_id: Uuid,
    delta_cursor: i64,
    now_ms: i64,
) -> Result<(), StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Delta)?;
    if delta_cursor < progress.delta_cursor {
        return Err(StorageError::IncompatibleSchema(
            "full resync delta cursor moved backwards".to_string(),
        ));
    }
    connection.execute(
        "UPDATE sync_full_resync_state
         SET delta_cursor = ?2, updated_at = ?3
         WHERE singleton = 1 AND generation_id = ?1",
        params![generation_id.to_string(), delta_cursor, now_ms],
    )?;
    Ok(())
}

pub(super) fn enter_full_resync_sweep_on(
    connection: &Connection,
    generation_id: Uuid,
    closure_high_water: i64,
    now_ms: i64,
) -> Result<(), StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Delta)?;
    if closure_high_water < 0 || progress.delta_cursor != closure_high_water {
        return Err(StorageError::IncompatibleSchema(
            "full resync delta has not reached closure high-water".to_string(),
        ));
    }
    connection.execute(
        "UPDATE sync_full_resync_state
         SET phase = 'sweep', closure_high_water = ?2,
             sweep_cursor_collection = NULL, sweep_cursor_record_id = NULL,
             updated_at = ?3
         WHERE singleton = 1 AND generation_id = ?1",
        params![generation_id.to_string(), closure_high_water, now_ms],
    )?;
    Ok(())
}

pub(super) fn sweep_full_resync_batch_on(
    connection: &Connection,
    generation_id: Uuid,
    limit: usize,
    now_ms: i64,
) -> Result<FullResyncSweepSummary, StorageError> {
    if limit == 0 {
        return Err(StorageError::IncompatibleSchema(
            "full resync sweep batch limit is zero".to_string(),
        ));
    }
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Sweep)?;
    let (after_order, after_record_id) = progress
        .sweep_cursor
        .as_ref()
        .map(|cursor| {
            Ok::<_, StorageError>((
                Some(local_sweep_collection_order(&cursor.collection)?),
                Some(cursor.record_id.to_string()),
            ))
        })
        .transpose()?
        .unwrap_or((None, None));
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("sweep limit exceeded i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT collection, record_id
         FROM sync_record_states
         WHERE ?1 IS NULL
            OR CASE collection
                   WHEN 'timer_sessions' THEN 0
                   WHEN 'tasks' THEN 1
                   WHEN 'task_series' THEN 2
                   WHEN 'templates' THEN 3
                   WHEN 'lists' THEN 4
               END > ?1
            OR (
                CASE collection
                    WHEN 'timer_sessions' THEN 0
                    WHEN 'tasks' THEN 1
                    WHEN 'task_series' THEN 2
                    WHEN 'templates' THEN 3
                    WHEN 'lists' THEN 4
                END = ?1
                AND record_id > ?2
            )
         ORDER BY CASE collection
                      WHEN 'timer_sessions' THEN 0
                      WHEN 'tasks' THEN 1
                      WHEN 'task_series' THEN 2
                      WHEN 'templates' THEN 3
                      WHEN 'lists' THEN 4
                  END ASC,
                  record_id ASC
         LIMIT ?3",
    )?;
    let records = statement
        .query_map(params![after_order, after_record_id, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut summary = FullResyncSweepSummary {
        scanned_records: records.len(),
        ..FullResyncSweepSummary::default()
    };
    for (collection, record_id) in &records {
        let marked: bool = connection.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_full_resync_marks
                 WHERE generation_id = ?1 AND collection = ?2 AND record_id = ?3
             )",
            params![generation_id.to_string(), collection, record_id],
            |row| row.get(0),
        )?;
        if marked || never_synced_record_is_valid(connection, generation_id, collection, record_id)?
        {
            continue;
        }
        let record_uuid = Uuid::parse_str(record_id)?;
        match collection.as_str() {
            "tasks" => {
                connection.execute(
                    "DELETE FROM task_undo_entries WHERE task_id = ?1",
                    [record_id],
                )?;
                connection.execute("DELETE FROM reminders WHERE task_id = ?1", [record_id])?;
                summary.swept_tasks +=
                    connection.execute("DELETE FROM tasks WHERE id = ?1", [record_id])?;
            }
            "lists" => {
                connection.execute(
                    "DELETE FROM sync_quarantine
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                connection.execute(
                    "DELETE FROM sync_outbox
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                connection.execute(
                    "DELETE FROM sync_record_origins
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                summary.swept_record_states += connection.execute(
                    "DELETE FROM sync_record_states
                     WHERE record_id IN (SELECT id FROM tasks WHERE list_id = ?1)",
                    [record_id],
                )?;
                let list_existed: bool = connection.query_row(
                    "SELECT EXISTS (SELECT 1 FROM lists WHERE id = ?1)",
                    [record_id],
                    |row| row.get(0),
                )?;
                summary.swept_tasks +=
                    delete_list_and_rehome_tasks_for_sync_on(connection, record_uuid)?;
                summary.swept_lists += usize::from(list_existed);
            }
            "templates" => {
                summary.swept_templates +=
                    connection.execute("DELETE FROM templates WHERE id = ?1", [record_id])?;
            }
            "task_series" => {
                summary.swept_task_series +=
                    connection.execute("DELETE FROM task_series WHERE id = ?1", [record_id])?;
            }
            "timer_sessions" => {
                summary.swept_timer_sessions +=
                    connection.execute("DELETE FROM timer_sessions WHERE id = ?1", [record_id])?;
            }
            other => return Err(StorageError::InvalidSyncCollection(other.to_string())),
        }
        connection.execute(
            "DELETE FROM sync_quarantine WHERE record_id = ?1",
            [record_uuid.to_string()],
        )?;
        connection.execute(
            "DELETE FROM sync_outbox WHERE collection = ?1 AND record_id = ?2",
            params![collection, record_id],
        )?;
        connection.execute(
            "DELETE FROM sync_record_origins WHERE record_id = ?1",
            [record_id],
        )?;
        summary.swept_record_states += connection.execute(
            "DELETE FROM sync_record_states WHERE collection = ?1 AND record_id = ?2",
            params![collection, record_id],
        )?;
    }
    if let Some((collection, record_id)) = records.last() {
        connection.execute(
            "UPDATE sync_full_resync_state
             SET sweep_cursor_collection = ?2, sweep_cursor_record_id = ?3, updated_at = ?4
             WHERE singleton = 1 AND generation_id = ?1",
            params![generation_id.to_string(), collection, record_id, now_ms],
        )?;
    }
    Ok(summary)
}

fn never_synced_record_is_valid(
    connection: &Connection,
    generation_id: Uuid,
    collection: &str,
    record_id: &str,
) -> Result<bool, StorageError> {
    match collection {
        "lists" => connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM sync_record_origins origin
                     JOIN sync_outbox outbox ON outbox.record_id = origin.record_id
                     WHERE origin.record_id = ?1 AND origin.collection = 'lists'
                       AND origin.origin_kind = 'never_synced'
                 )",
                [record_id],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        "tasks" => connection
            .query_row(
                "WITH RECURSIVE ancestors(id, parent_task_id, valid) AS (
                     SELECT task.id, task.parent_task_id, 1
                     FROM tasks task WHERE task.id = ?1
                     UNION ALL
                     SELECT parent.id, parent.parent_task_id,
                            EXISTS (
                                SELECT 1 FROM sync_record_origins parent_origin
                                JOIN sync_outbox parent_outbox
                                  ON parent_outbox.record_id = parent_origin.record_id
                                WHERE parent_origin.record_id = parent.id
                                  AND parent_origin.collection = 'tasks'
                                  AND parent_origin.origin_kind = 'never_synced'
                                UNION ALL
                                SELECT 1 FROM sync_full_resync_marks parent_mark
                                WHERE parent_mark.generation_id = ?2
                                  AND parent_mark.collection = 'tasks'
                                  AND parent_mark.record_id = parent.id
                            )
                     FROM tasks parent
                     JOIN ancestors child ON child.parent_task_id = parent.id
                 )
                 SELECT EXISTS (
                     SELECT 1 FROM tasks task
                     JOIN sync_record_origins task_origin
                       ON task_origin.record_id = task.id
                      AND task_origin.collection = 'tasks'
                      AND task_origin.origin_kind = 'never_synced'
                     JOIN sync_outbox task_outbox ON task_outbox.record_id = task.id
                     WHERE task.id = ?1
                       AND EXISTS (
                           SELECT 1 FROM sync_record_origins list_origin
                           JOIN sync_outbox list_outbox
                             ON list_outbox.record_id = list_origin.record_id
                           WHERE list_origin.record_id = task.list_id
                             AND list_origin.collection = 'lists'
                             AND list_origin.origin_kind = 'never_synced'
                           UNION ALL
                           SELECT 1 FROM sync_full_resync_marks list_mark
                           WHERE list_mark.generation_id = ?2
                             AND list_mark.collection = 'lists'
                             AND list_mark.record_id = task.list_id
                       )
                       AND NOT EXISTS (SELECT 1 FROM ancestors WHERE valid = 0)
                       AND NOT EXISTS (
                           SELECT 1 FROM ancestors child
                           WHERE child.parent_task_id IS NOT NULL
                             AND NOT EXISTS (
                                 SELECT 1 FROM ancestors parent
                                 WHERE parent.id = child.parent_task_id
                             )
                       )
                 )",
                params![record_id, generation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        "templates" => never_synced_record_has_outbox(connection, collection, record_id),
        "task_series" => never_synced_record_has_outbox(connection, collection, record_id),
        "timer_sessions" => connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM timer_sessions timer
                     JOIN sync_record_origins origin
                       ON origin.record_id = timer.id
                      AND origin.collection = 'timer_sessions'
                      AND origin.origin_kind = 'never_synced'
                     JOIN sync_outbox outbox
                       ON outbox.record_id = timer.id
                      AND outbox.collection = 'timer_sessions'
                     WHERE timer.id = ?1
                       AND EXISTS (
                           SELECT 1 FROM sync_record_origins task_origin
                           JOIN sync_outbox task_outbox
                             ON task_outbox.record_id = task_origin.record_id
                            AND task_outbox.collection = 'tasks'
                           WHERE task_origin.record_id = timer.task_id
                             AND task_origin.collection = 'tasks'
                             AND task_origin.origin_kind = 'never_synced'
                           UNION ALL
                           SELECT 1 FROM sync_full_resync_marks task_mark
                           WHERE task_mark.generation_id = ?2
                             AND task_mark.collection = 'tasks'
                             AND task_mark.record_id = timer.task_id
                       )
                 )",
                params![record_id, generation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StorageError::from),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

fn never_synced_record_has_outbox(
    connection: &Connection,
    collection: &str,
    record_id: &str,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_record_origins origin
                 JOIN sync_outbox outbox
                   ON outbox.record_id = origin.record_id
                  AND outbox.collection = origin.collection
                 WHERE origin.record_id = ?1
                   AND origin.collection = ?2
                   AND origin.origin_kind = 'never_synced'
             )",
            params![record_id, collection],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

fn local_sweep_collection_order(collection: &str) -> Result<i64, StorageError> {
    match collection {
        "timer_sessions" => Ok(0),
        "tasks" => Ok(1),
        "task_series" => Ok(2),
        "templates" => Ok(3),
        "lists" => Ok(4),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

pub(super) fn finalize_full_resync_on(
    connection: &Connection,
    generation_id: Uuid,
    cursor_name: &str,
    now_ms: i64,
) -> Result<i64, StorageError> {
    let progress = require_full_resync(connection, generation_id, FullResyncPhase::Sweep)?;
    let high_water = progress.closure_high_water.ok_or_else(|| {
        StorageError::IncompatibleSchema("full resync closure high-water is missing".to_string())
    })?;
    let (after_order, after_record_id) = progress
        .sweep_cursor
        .as_ref()
        .map(|cursor| {
            Ok::<_, StorageError>((
                local_sweep_collection_order(&cursor.collection)?,
                cursor.record_id.to_string(),
            ))
        })
        .transpose()?
        .unwrap_or((-1, String::new()));
    let has_more: bool = connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM sync_record_states
             WHERE CASE collection
                       WHEN 'timer_sessions' THEN 0
                       WHEN 'tasks' THEN 1
                       WHEN 'task_series' THEN 2
                       WHEN 'templates' THEN 3
                       WHEN 'lists' THEN 4
                   END > ?1
                OR (
                    CASE collection
                        WHEN 'timer_sessions' THEN 0
                        WHEN 'tasks' THEN 1
                        WHEN 'task_series' THEN 2
                        WHEN 'templates' THEN 3
                        WHEN 'lists' THEN 4
                    END = ?1
                    AND record_id > ?2
                )
         )",
        params![after_order, after_record_id],
        |row| row.get(0),
    )?;
    if has_more {
        return Err(StorageError::IncompatibleSchema(
            "full resync sweep is incomplete".to_string(),
        ));
    }
    set_cursor_on(connection, cursor_name, high_water, now_ms)?;
    connection.execute(
        "DELETE FROM sync_full_resync_marks WHERE generation_id = ?1",
        [generation_id.to_string()],
    )?;
    connection.execute(
        "DELETE FROM sync_full_resync_state WHERE singleton = 1 AND generation_id = ?1",
        [generation_id.to_string()],
    )?;
    Ok(high_water)
}

pub(super) fn reset_full_resync_on(connection: &Connection) -> Result<(), StorageError> {
    connection.execute("DELETE FROM sync_full_resync_marks", [])?;
    connection.execute("DELETE FROM sync_full_resync_state", [])?;
    Ok(())
}
