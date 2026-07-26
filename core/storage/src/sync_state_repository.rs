use crate::*;

/// SQLite-backed implementation of [`SyncStateRepository`].
pub struct SqliteSyncStateRepository {
    connection: Connection,
}

impl SqliteSyncStateRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.connection, collection, record_id)
    }

    pub fn list_record_states(
        &self,
        collection: &str,
    ) -> Result<Vec<SyncRecordState>, StorageError> {
        list_record_states_on(&self.connection, collection)
    }

    pub fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.connection, state)
    }

    pub fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(&self.connection, entry)
    }

    pub fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(&self.connection, limit)
    }

    pub fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(&self.connection, after, limit)
    }

    pub fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(&self.connection, record_id)
    }

    pub fn has_live_quarantine(&self, collection: &str) -> Result<bool, StorageError> {
        has_live_quarantine_on(&self.connection, collection)
    }

    pub fn list_list_aliases(&self) -> Result<Vec<ListAlias>, StorageError> {
        list_list_aliases_on(&self.connection)
    }

    pub fn resolve_list_alias(&self, list_id: Uuid) -> Result<Uuid, StorageError> {
        resolve_list_alias_on(&self.connection, list_id)
    }

    pub fn load_full_resync(&self) -> Result<Option<FullResyncProgress>, StorageError> {
        load_full_resync_on(&self.connection)
    }

    pub fn start_full_resync(
        &mut self,
        generation_id: Uuid,
        continuity_generation: i64,
        base_seq: i64,
        now_ms: i64,
    ) -> Result<FullResyncProgress, StorageError> {
        start_full_resync_on(
            &self.connection,
            generation_id,
            continuity_generation,
            base_seq,
            now_ms,
        )
    }
}

impl SyncStateRepository for SqliteSyncStateRepository {
    fn put_outbox_head(
        &mut self,
        entry: NewSyncOutboxEntry,
    ) -> Result<SyncOutboxEntry, StorageError> {
        put_outbox_head_on(&self.connection, entry)
    }

    fn list_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_outbox_heads_on(&self.connection, limit)
    }

    fn list_all_outbox_heads(&self, limit: usize) -> Result<Vec<SyncOutboxEntry>, StorageError> {
        list_all_outbox_heads_on(&self.connection, limit)
    }

    fn has_outbox_head(&self, collection: &str, record_id: Uuid) -> Result<bool, StorageError> {
        has_outbox_head_on(&self.connection, collection, record_id)
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, StorageError> {
        ack_outbox_op_on(&self.connection, op_id)
    }

    fn delete_outbox_head(
        &mut self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<bool, StorageError> {
        delete_outbox_head_on(&self.connection, collection, record_id)
    }

    fn get_record_state(
        &self,
        collection: &str,
        record_id: Uuid,
    ) -> Result<Option<SyncRecordState>, StorageError> {
        get_record_state_on(&self.connection, collection, record_id)
    }

    fn put_record_state(&mut self, state: SyncRecordState) -> Result<(), StorageError> {
        put_record_state_on(&self.connection, state)
    }

    fn get_cursor(&self, name: &str) -> Result<Option<SyncCursor>, StorageError> {
        get_cursor_on(&self.connection, name)
    }

    fn set_cursor(&mut self, name: &str, seq: i64, updated_at: i64) -> Result<(), StorageError> {
        set_cursor_on(&self.connection, name, seq, updated_at)
    }

    fn delete_cursor(&mut self, name: &str) -> Result<(), StorageError> {
        delete_cursor_on(&self.connection, name)
    }

    fn put_quarantine(&mut self, entry: SyncQuarantineEntry) -> Result<(), StorageError> {
        put_quarantine_on(&self.connection, entry)
    }

    fn list_quarantine(&self, limit: usize) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_quarantine_on(&self.connection, limit)
    }

    fn list_replayable_quarantine(
        &self,
        after: Option<(i64, Uuid)>,
        limit: usize,
    ) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
        list_replayable_quarantine_on(&self.connection, after, limit)
    }

    fn delete_quarantine(&mut self, record_id: Uuid) -> Result<bool, StorageError> {
        delete_quarantine_on(&self.connection, record_id)
    }
}

pub(super) fn get_cursor_on(
    connection: &Connection,
    name: &str,
) -> Result<Option<SyncCursor>, StorageError> {
    connection
        .query_row(
            "SELECT name, seq, updated_at
             FROM sync_cursors
             WHERE name = ?1",
            [name],
            row_to_sync_cursor,
        )
        .optional()
        .map_err(StorageError::from)
}

pub(super) fn set_cursor_on(
    connection: &Connection,
    name: &str,
    seq: i64,
    updated_at: i64,
) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO sync_cursors (name, seq, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
             seq = excluded.seq,
             updated_at = excluded.updated_at",
        params![name, seq, updated_at],
    )?;
    Ok(())
}

pub(super) fn delete_cursor_on(connection: &Connection, name: &str) -> Result<(), StorageError> {
    connection.execute("DELETE FROM sync_cursors WHERE name = ?1", [name])?;
    Ok(())
}

pub(super) fn list_record_states_on(
    connection: &Connection,
    collection: &str,
) -> Result<Vec<SyncRecordState>, StorageError> {
    validate_sync_collection(collection)?;
    let mut statement = connection.prepare(
        "SELECT record_id, collection, current_revision_hlc, state_kind,
                semantic_hlc, plaintext_json, updated_at
         FROM sync_record_states
         WHERE collection = ?1
         ORDER BY record_id ASC",
    )?;
    let nested_states = statement
        .query_map([collection], row_to_sync_record_state)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let states = nested_states
        .into_iter()
        .collect::<Result<Vec<_>, StorageError>>()?;
    Ok(states)
}

pub(super) fn has_live_quarantine_on(
    connection: &Connection,
    collection: &str,
) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM sync_quarantine
                 WHERE collection = ?1 AND state_kind = 'live'
             )",
            [collection],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

pub(super) fn list_list_aliases_on(
    connection: &Connection,
) -> Result<Vec<ListAlias>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT alias_list_id, canonical_list_id, updated_at
         FROM list_aliases ORDER BY alias_list_id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(alias_list_id, canonical_list_id, updated_at)| {
            Ok(ListAlias {
                alias_list_id: Uuid::parse_str(&alias_list_id)?,
                canonical_list_id: Uuid::parse_str(&canonical_list_id)?,
                updated_at,
            })
        })
        .collect()
}

pub(super) fn resolve_list_alias_on(
    connection: &Connection,
    list_id: Uuid,
) -> Result<Uuid, StorageError> {
    let canonical = connection
        .query_row(
            "SELECT canonical_list_id FROM list_aliases WHERE alias_list_id = ?1",
            [list_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(canonical) = canonical else {
        return Ok(list_id);
    };
    let canonical = Uuid::parse_str(&canonical)?;
    let canonical_is_alias: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM list_aliases WHERE alias_list_id = ?1)",
        [canonical.to_string()],
        |row| row.get(0),
    )?;
    if canonical_is_alias {
        return Err(StorageError::IncompatibleSchema(
            "list alias chain is not allowed".to_string(),
        ));
    }
    let canonical_exists: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM lists WHERE id = ?1)",
        [canonical.to_string()],
        |row| row.get(0),
    )?;
    if !canonical_exists {
        return Err(StorageError::IncompatibleSchema(
            "list alias points to a missing canonical list".to_string(),
        ));
    }
    Ok(canonical)
}

pub(super) fn replace_list_aliases_on(
    connection: &Connection,
    canonical_list_id: Uuid,
    alias_list_ids: &[Uuid],
    updated_at: i64,
) -> Result<(), StorageError> {
    let canonical = get_list_on(connection, canonical_list_id)?;
    if !canonical.is_default {
        return Err(StorageError::IncompatibleSchema(
            "canonical list must be materialized before aliases are replaced".to_string(),
        ));
    }
    let mut unique = HashSet::with_capacity(alias_list_ids.len());
    for alias_list_id in alias_list_ids {
        if *alias_list_id == canonical_list_id || !unique.insert(*alias_list_id) {
            return Err(StorageError::IncompatibleSchema(
                "invalid canonical Inbox alias set".to_string(),
            ));
        }
        get_list_on(connection, *alias_list_id)?;
    }

    connection.execute("DELETE FROM list_aliases", [])?;
    for alias_list_id in alias_list_ids {
        connection.execute(
            "INSERT INTO list_aliases (alias_list_id, canonical_list_id, updated_at)
             VALUES (?1, ?2, ?3)",
            params![
                alias_list_id.to_string(),
                canonical_list_id.to_string(),
                updated_at,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn materialize_canonical_list_on(
    connection: &Connection,
    canonical_list_id: Uuid,
) -> Result<(), StorageError> {
    let canonical = get_list_on(connection, canonical_list_id)?;
    if canonical.archived_at.is_some() {
        return Err(StorageError::DefaultListProtected {
            operation: "archived",
            list_id: canonical_list_id,
        });
    }
    // Demote first so the existing partial unique index never observes two
    // default rows during a canonical switch.
    connection.execute("UPDATE lists SET is_default = 0 WHERE is_default = 1", [])?;
    let changed = connection.execute(
        "UPDATE lists SET is_default = 1 WHERE id = ?1",
        [canonical_list_id.to_string()],
    )?;
    if changed != 1 {
        return Err(StorageError::NotFound(canonical_list_id));
    }
    Ok(())
}

pub(super) fn get_record_state_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<Option<SyncRecordState>, StorageError> {
    validate_sync_collection(collection)?;
    let state = connection
        .query_row(
            "SELECT record_id, collection, current_revision_hlc, state_kind,
                    semantic_hlc, plaintext_json, updated_at
             FROM sync_record_states
             WHERE record_id = ?1",
            [record_id.to_string()],
            row_to_sync_record_state,
        )
        .optional()?
        .transpose()?;
    if let Some(state) = &state {
        ensure_requested_collection(record_id, &state.collection, collection)?;
    }
    Ok(state)
}

pub(super) fn put_record_state_on(
    connection: &Connection,
    state: SyncRecordState,
) -> Result<(), StorageError> {
    validate_sync_collection(&state.collection)?;
    ensure_sync_collection_matches(connection, state.record_id, &state.collection)?;
    let record_id = state.record_id.to_string();
    let collection = state.collection.clone();
    let current_revision_hlc = state.current_revision_hlc.clone();
    let updated_at = state.updated_at;
    let (state_kind, semantic_hlc, plaintext_json) = match state.state {
        SyncRecordSemanticState::Live {
            mutation_hlc,
            plaintext_json,
        } => ("live", mutation_hlc, Some(plaintext_json)),
        SyncRecordSemanticState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_record_states (
             record_id, collection, current_revision_hlc, state_kind,
             semantic_hlc, plaintext_json, updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7
         )
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             current_revision_hlc = excluded.current_revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             plaintext_json = excluded.plaintext_json,
             updated_at = excluded.updated_at",
        params![
            record_id,
            collection,
            current_revision_hlc,
            state_kind,
            semantic_hlc,
            plaintext_json,
            updated_at,
        ],
    )?;
    let origin_kind = if current_revision_hlc.is_some() {
        "server_seen"
    } else {
        "never_synced"
    };
    connection.execute(
        "INSERT INTO sync_record_origins (record_id, collection, origin_kind, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             origin_kind = CASE
                 WHEN sync_record_origins.origin_kind = 'server_seen' THEN 'server_seen'
                 ELSE excluded.origin_kind
             END,
             updated_at = excluded.updated_at",
        params![record_id, collection, origin_kind, updated_at],
    )?;
    Ok(())
}

pub(super) fn put_outbox_head_on(
    connection: &Connection,
    entry: NewSyncOutboxEntry,
) -> Result<SyncOutboxEntry, StorageError> {
    validate_sync_collection(&entry.collection)?;
    ensure_sync_collection_matches(connection, entry.record_id, &entry.collection)?;
    let origin_record_id = entry.record_id.to_string();
    let origin_collection = entry.collection.clone();
    let origin_updated_at = entry.created_at;
    let (state_kind, semantic_hlc, blob) = match entry.state {
        SyncOutboxState::Live { mutation_hlc, blob } => ("live", mutation_hlc, Some(blob)),
        SyncOutboxState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_outbox (
             record_id, collection, op_id, base_revision_hlc, revision_hlc,
             state_kind, semantic_hlc, blob, created_at
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
         )
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             op_id = excluded.op_id,
             base_revision_hlc = excluded.base_revision_hlc,
             revision_hlc = excluded.revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             blob = excluded.blob,
             created_at = excluded.created_at",
        params![
            entry.record_id.to_string(),
            entry.collection,
            entry.op_id.to_string(),
            entry.base_revision_hlc,
            entry.revision_hlc,
            state_kind,
            semantic_hlc,
            blob,
            entry.created_at,
        ],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO sync_record_origins
             (record_id, collection, origin_kind, updated_at)
         VALUES (?1, ?2, 'never_synced', ?3)",
        params![origin_record_id, origin_collection, origin_updated_at],
    )?;
    connection
        .query_row(
            "SELECT op_id, record_id, collection, base_revision_hlc,
                    revision_hlc, state_kind, semantic_hlc, blob, created_at
             FROM sync_outbox
             WHERE record_id = ?1",
            [entry.record_id.to_string()],
            row_to_sync_outbox_entry,
        )
        .map_err(StorageError::from)?
}

pub(super) fn list_outbox_heads_on(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<SyncOutboxEntry>, StorageError> {
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("outbox limit exceeded i64".to_string()))?;
    let mut statement = connection.prepare(
        "SELECT op_id, record_id, collection, base_revision_hlc,
                revision_hlc, state_kind, semantic_hlc, blob, created_at
         FROM sync_outbox AS outbox
         WHERE NOT EXISTS (
             SELECT 1 FROM sync_quarantine AS quarantine
             WHERE quarantine.record_id = outbox.record_id
         )
         ORDER BY CASE collection WHEN 'lists' THEN 0 ELSE 1 END ASC,
                  created_at ASC, record_id ASC
         LIMIT ?1",
    )?;
    let entries = statement
        .query_map([limit], row_to_sync_outbox_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    entries.into_iter().collect()
}

pub(super) fn list_all_outbox_heads_on(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<SyncOutboxEntry>, StorageError> {
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("outbox limit exceeded i64".to_string()))?;
    let mut statement = connection.prepare(
        "SELECT op_id, record_id, collection, base_revision_hlc,
                revision_hlc, state_kind, semantic_hlc, blob, created_at
         FROM sync_outbox
         ORDER BY CASE collection WHEN 'lists' THEN 0 ELSE 1 END ASC,
                  created_at ASC, record_id ASC
         LIMIT ?1",
    )?;
    let entries = statement
        .query_map([limit], row_to_sync_outbox_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    entries.into_iter().collect()
}

pub(super) fn put_quarantine_on(
    connection: &Connection,
    entry: SyncQuarantineEntry,
) -> Result<(), StorageError> {
    validate_sync_collection(&entry.collection)?;
    ensure_sync_collection_matches(connection, entry.record_id, &entry.collection)?;
    let existing = connection
        .query_row(
            "SELECT seq, revision_hlc FROM sync_quarantine WHERE record_id = ?1",
            [entry.record_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((existing_seq, existing_revision_hlc)) = existing {
        if entry.seq < existing_seq {
            return Ok(());
        }
        if entry.seq == existing_seq {
            if entry.revision_hlc != existing_revision_hlc {
                return Err(StorageError::IncompatibleSchema(
                    "quarantine revision changed at the same server sequence".to_string(),
                ));
            }
            connection.execute(
                "UPDATE sync_quarantine
                 SET reason = ?2,
                     required_list_id = ?3,
                     last_failed_at = ?4,
                     attempt_count = attempt_count + 1
                 WHERE record_id = ?1",
                params![
                    entry.record_id.to_string(),
                    entry.reason,
                    entry.required_list_id.map(|id| id.to_string()),
                    entry.last_failed_at,
                ],
            )?;
            return Ok(());
        }
    }
    let (state_kind, semantic_hlc, blob) = match entry.state {
        SyncOutboxState::Live { mutation_hlc, blob } => ("live", mutation_hlc, Some(blob)),
        SyncOutboxState::Tombstone { delete_hlc } => ("tombstone", delete_hlc, None),
    };
    connection.execute(
        "INSERT INTO sync_quarantine (
             record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
             blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(record_id) DO UPDATE SET
             collection = excluded.collection,
             seq = excluded.seq,
             revision_hlc = excluded.revision_hlc,
             state_kind = excluded.state_kind,
             semantic_hlc = excluded.semantic_hlc,
             blob = excluded.blob,
             reason = excluded.reason,
             required_list_id = excluded.required_list_id,
             first_failed_at = excluded.first_failed_at,
             last_failed_at = excluded.last_failed_at,
             attempt_count = excluded.attempt_count",
        params![
            entry.record_id.to_string(),
            entry.collection,
            entry.seq,
            entry.revision_hlc,
            state_kind,
            semantic_hlc,
            blob,
            entry.reason,
            entry.required_list_id.map(|id| id.to_string()),
            entry.first_failed_at,
            entry.last_failed_at,
            entry.attempt_count,
        ],
    )?;
    Ok(())
}

pub(super) fn list_quarantine_on(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
    let limit = i64::try_from(limit)
        .map_err(|_| StorageError::IncompatibleSchema("quarantine limit exceeded i64".into()))?;
    let mut statement = connection.prepare(
        "SELECT record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
                blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         FROM sync_quarantine ORDER BY seq ASC, record_id ASC LIMIT ?1",
    )?;
    let entries = statement
        .query_map([limit], row_to_sync_quarantine)?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();
    entries
}

pub(super) fn list_replayable_quarantine_on(
    connection: &Connection,
    after: Option<(i64, Uuid)>,
    limit: usize,
) -> Result<Vec<SyncQuarantineEntry>, StorageError> {
    let limit = i64::try_from(limit).map_err(|_| {
        StorageError::IncompatibleSchema("quarantine replay limit exceeded i64".into())
    })?;
    let (after_seq, after_record_id) = after
        .map(|(seq, record_id)| (Some(seq), Some(record_id.to_string())))
        .unwrap_or((None, None));
    let mut statement = connection.prepare(
        "SELECT record_id, collection, seq, revision_hlc, state_kind, semantic_hlc,
                blob, reason, required_list_id, first_failed_at, last_failed_at, attempt_count
         FROM sync_quarantine
         WHERE reason IN ('missing_dek', 'no_matching_dek', 'missing_dependency')
           AND (?1 IS NULL OR seq > ?1 OR (seq = ?1 AND record_id > ?2))
         ORDER BY seq ASC, record_id ASC
         LIMIT ?3",
    )?;
    let entries = statement
        .query_map(
            params![after_seq, after_record_id, limit],
            row_to_sync_quarantine,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();
    entries
}

pub(super) fn delete_quarantine_on(
    connection: &Connection,
    record_id: Uuid,
) -> Result<bool, StorageError> {
    Ok(connection.execute(
        "DELETE FROM sync_quarantine WHERE record_id = ?1",
        [record_id.to_string()],
    )? == 1)
}

fn row_to_sync_quarantine(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncQuarantineEntry, StorageError>> {
    let record_id = match Uuid::parse_str(&row.get::<_, String>(0)?) {
        Ok(value) => value,
        Err(error) => return Ok(Err(StorageError::InvalidUuid(error))),
    };
    let collection: String = row.get(1)?;
    let state_kind: String = row.get(4)?;
    let semantic_hlc: String = row.get(5)?;
    let blob: Option<Vec<u8>> = row.get(6)?;
    let state = match (state_kind.as_str(), blob) {
        ("live", Some(blob)) => SyncOutboxState::Live {
            mutation_hlc: semantic_hlc,
            blob,
        },
        ("tombstone", None) => SyncOutboxState::Tombstone {
            delete_hlc: semantic_hlc,
        },
        _ => return Ok(Err(StorageError::InvalidSyncState(state_kind))),
    };
    let required_list_id = match row.get::<_, Option<String>>(8)? {
        Some(value) => match Uuid::parse_str(&value) {
            Ok(value) => Some(value),
            Err(error) => return Ok(Err(StorageError::InvalidUuid(error))),
        },
        None => None,
    };
    Ok(Ok(SyncQuarantineEntry {
        record_id,
        collection,
        seq: row.get(2)?,
        revision_hlc: row.get(3)?,
        state,
        reason: row.get(7)?,
        required_list_id,
        first_failed_at: row.get(9)?,
        last_failed_at: row.get(10)?,
        attempt_count: row.get(11)?,
    }))
}

pub(super) fn has_outbox_head_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    let existing = connection
        .query_row(
            "SELECT collection
             FROM sync_outbox
             WHERE record_id = ?1",
            [record_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        ensure_requested_collection(record_id, &existing, collection)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn ack_outbox_op_on(connection: &Connection, op_id: Uuid) -> Result<bool, StorageError> {
    let acked: Option<(String, String)> = connection
        .query_row(
            "SELECT record_id, collection FROM sync_outbox WHERE op_id = ?1",
            [op_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let changed = connection.execute(
        "DELETE FROM sync_outbox WHERE op_id = ?1",
        [op_id.to_string()],
    )?;
    if let Some((record_id, collection)) = acked {
        connection.execute(
            "INSERT INTO sync_record_origins (record_id, collection, origin_kind, updated_at)
             VALUES (?1, ?2, 'server_seen', unixepoch('subsec') * 1000)
             ON CONFLICT(record_id) DO UPDATE SET
                 origin_kind = 'server_seen', collection = excluded.collection,
                 updated_at = excluded.updated_at",
            params![record_id, collection],
        )?;
    }
    Ok(changed == 1)
}

pub(super) fn delete_outbox_head_on(
    connection: &Connection,
    collection: &str,
    record_id: Uuid,
) -> Result<bool, StorageError> {
    validate_sync_collection(collection)?;
    ensure_sync_collection_matches(connection, record_id, collection)?;
    let changed = connection.execute(
        "DELETE FROM sync_outbox WHERE collection = ?1 AND record_id = ?2",
        params![collection, record_id.to_string()],
    )?;
    Ok(changed == 1)
}

pub(super) fn validate_sync_collection(collection: &str) -> Result<(), StorageError> {
    match collection {
        "lists" | "tasks" | "templates" | "task_series" | "timer_sessions" => Ok(()),
        other => Err(StorageError::InvalidSyncCollection(other.to_string())),
    }
}

fn ensure_sync_collection_matches(
    connection: &Connection,
    record_id: Uuid,
    requested: &str,
) -> Result<(), StorageError> {
    let existing = connection
        .query_row(
            "SELECT collection FROM sync_record_states WHERE record_id = ?1
             UNION ALL
             SELECT collection FROM sync_outbox WHERE record_id = ?1
             UNION ALL
             SELECT collection FROM sync_quarantine WHERE record_id = ?1
             LIMIT 1",
            [record_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        ensure_requested_collection(record_id, &existing, requested)?;
    }
    Ok(())
}

fn ensure_requested_collection(
    record_id: Uuid,
    existing: &str,
    requested: &str,
) -> Result<(), StorageError> {
    if existing == requested {
        Ok(())
    } else {
        Err(StorageError::SyncCollectionMismatch {
            record_id,
            existing: existing.to_string(),
            requested: requested.to_string(),
        })
    }
}

fn row_to_sync_outbox_entry(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncOutboxEntry, StorageError>> {
    let op_id: String = row.get(0)?;
    let record_id: String = row.get(1)?;
    let state_kind: String = row.get(5)?;
    let semantic_hlc: String = row.get(6)?;
    let blob: Option<Vec<u8>> = row.get(7)?;
    Ok((|| {
        let state = match (state_kind.as_str(), blob) {
            ("live", Some(blob)) => SyncOutboxState::Live {
                mutation_hlc: semantic_hlc,
                blob,
            },
            ("tombstone", None) => SyncOutboxState::Tombstone {
                delete_hlc: semantic_hlc,
            },
            (kind, _) => return Err(StorageError::InvalidSyncState(kind.to_string())),
        };
        Ok(SyncOutboxEntry {
            op_id: Uuid::from_str(&op_id)?,
            record_id: Uuid::from_str(&record_id)?,
            collection: row.get(2)?,
            base_revision_hlc: row.get(3)?,
            revision_hlc: row.get(4)?,
            state,
            created_at: row.get(8)?,
        })
    })())
}

fn row_to_sync_record_state(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<SyncRecordState, StorageError>> {
    let record_id: String = row.get(0)?;
    let state_kind: String = row.get(3)?;
    let semantic_hlc: String = row.get(4)?;
    let plaintext_json: Option<String> = row.get(5)?;
    Ok((|| {
        let state = match (state_kind.as_str(), plaintext_json) {
            ("live", Some(plaintext_json)) => SyncRecordSemanticState::Live {
                mutation_hlc: semantic_hlc,
                plaintext_json,
            },
            ("tombstone", None) => SyncRecordSemanticState::Tombstone {
                delete_hlc: semantic_hlc,
            },
            (kind, _) => return Err(StorageError::InvalidSyncState(kind.to_string())),
        };
        Ok(SyncRecordState {
            record_id: Uuid::from_str(&record_id)?,
            collection: row.get(1)?,
            current_revision_hlc: row.get(2)?,
            state,
            updated_at: row.get(6)?,
        })
    })())
}

fn row_to_sync_cursor(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncCursor> {
    Ok(SyncCursor {
        name: row.get(0)?,
        seq: row.get(1)?,
        updated_at: row.get(2)?,
    })
}
