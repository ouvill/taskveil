use taskveil_domain::Uuid;
use taskveil_storage::{
    FullResyncPhase, FullResyncProgress, FullResyncStableCursor, FullResyncSweepSummary, ListAlias,
    NewSyncOutboxEntry, SyncOutboxEntry, SyncOutboxState, SyncQuarantineEntry,
    SyncRecordSemanticState, SyncRecordState,
};
use taskveil_sync::{
    enqueue::{LocalFullResyncPhase, LocalFullResyncProgress, LocalFullResyncSweepSummary},
    EncryptedSyncState, LocalListAlias, LocalSyncOutboxEntry, LocalSyncQuarantineEntry,
    LocalSyncRecordState, LocalSyncSemanticState, NewLocalSyncOutboxEntry, PullFailureReason,
    StableCursor, SyncCollection,
};

pub(super) fn storage_resync_to_local(progress: FullResyncProgress) -> LocalFullResyncProgress {
    LocalFullResyncProgress {
        generation_id: progress.generation_id,
        continuity_generation: progress.continuity_generation,
        phase: match progress.phase {
            FullResyncPhase::Base => LocalFullResyncPhase::Base,
            FullResyncPhase::Delta => LocalFullResyncPhase::Delta,
            FullResyncPhase::Sweep => LocalFullResyncPhase::Sweep,
        },
        base_seq: progress.base_seq,
        base_cursor: progress.base_cursor.map(storage_cursor_to_local),
        delta_cursor: progress.delta_cursor,
        closure_high_water: progress.closure_high_water,
        sweep_cursor: progress.sweep_cursor.map(storage_cursor_to_local),
    }
}

pub(super) fn storage_cursor_to_local(cursor: FullResyncStableCursor) -> StableCursor {
    StableCursor {
        collection: cursor
            .collection
            .parse()
            .expect("storage validates full resync cursor collection"),
        record_id: cursor.record_id,
    }
}

pub(super) fn local_cursor_to_storage(cursor: &StableCursor) -> FullResyncStableCursor {
    FullResyncStableCursor {
        collection: cursor.collection.to_string(),
        record_id: cursor.record_id,
    }
}

pub(super) fn storage_sweep_to_local(
    summary: FullResyncSweepSummary,
) -> LocalFullResyncSweepSummary {
    LocalFullResyncSweepSummary {
        scanned_records: summary.scanned_records,
        swept_lists: summary.swept_lists,
        swept_tasks: summary.swept_tasks,
        swept_templates: summary.swept_templates,
        swept_task_series: summary.swept_task_series,
        swept_timer_sessions: summary.swept_timer_sessions,
        swept_record_states: summary.swept_record_states,
    }
}

pub(super) fn local_outbox_to_storage(entry: NewLocalSyncOutboxEntry) -> NewSyncOutboxEntry {
    NewSyncOutboxEntry {
        op_id: entry.op_id,
        record_id: entry.record_id,
        collection: entry.collection.to_string(),
        base_revision_hlc: entry.base_revision_hlc,
        revision_hlc: entry.revision_hlc,
        state: match entry.state {
            EncryptedSyncState::Live { mutation_hlc, blob } => {
                SyncOutboxState::Live { mutation_hlc, blob }
            }
            EncryptedSyncState::Tombstone { delete_hlc } => {
                SyncOutboxState::Tombstone { delete_hlc }
            }
        },
        created_at: entry.created_at,
    }
}

pub(super) fn storage_outbox_to_local(
    entry: SyncOutboxEntry,
) -> Result<LocalSyncOutboxEntry, String> {
    Ok(LocalSyncOutboxEntry {
        op_id: entry.op_id,
        record_id: entry.record_id,
        collection: entry
            .collection
            .parse::<SyncCollection>()
            .map_err(|error| error.to_string())?,
        base_revision_hlc: entry.base_revision_hlc,
        revision_hlc: entry.revision_hlc,
        state: match entry.state {
            SyncOutboxState::Live { mutation_hlc, blob } => {
                EncryptedSyncState::Live { mutation_hlc, blob }
            }
            SyncOutboxState::Tombstone { delete_hlc } => {
                EncryptedSyncState::Tombstone { delete_hlc }
            }
        },
        created_at: entry.created_at,
    })
}

pub(super) fn local_quarantine_to_storage(entry: LocalSyncQuarantineEntry) -> SyncQuarantineEntry {
    SyncQuarantineEntry {
        record_id: entry.record_id,
        collection: entry.collection.to_string(),
        seq: entry.seq,
        revision_hlc: entry.revision_hlc,
        state: match entry.state {
            EncryptedSyncState::Live { mutation_hlc, blob } => {
                SyncOutboxState::Live { mutation_hlc, blob }
            }
            EncryptedSyncState::Tombstone { delete_hlc } => {
                SyncOutboxState::Tombstone { delete_hlc }
            }
        },
        reason: entry.reason.as_str().to_string(),
        required_list_id: entry.required_list_id,
        first_failed_at: entry.first_failed_at,
        last_failed_at: entry.last_failed_at,
        attempt_count: entry.attempt_count,
    }
}

pub(super) fn storage_quarantine_to_local(
    entry: SyncQuarantineEntry,
) -> Result<LocalSyncQuarantineEntry, String> {
    Ok(LocalSyncQuarantineEntry {
        record_id: entry.record_id,
        collection: entry
            .collection
            .parse::<SyncCollection>()
            .map_err(|error| error.to_string())?,
        seq: entry.seq,
        revision_hlc: entry.revision_hlc,
        state: match entry.state {
            SyncOutboxState::Live { mutation_hlc, blob } => {
                EncryptedSyncState::Live { mutation_hlc, blob }
            }
            SyncOutboxState::Tombstone { delete_hlc } => {
                EncryptedSyncState::Tombstone { delete_hlc }
            }
        },
        reason: entry.reason.parse::<PullFailureReason>()?,
        required_list_id: entry.required_list_id,
        first_failed_at: entry.first_failed_at,
        last_failed_at: entry.last_failed_at,
        attempt_count: entry.attempt_count,
    })
}

pub(super) fn storage_record_to_local(state: SyncRecordState) -> LocalSyncRecordState {
    LocalSyncRecordState {
        current_revision_hlc: state.current_revision_hlc,
        state: match state.state {
            SyncRecordSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            } => LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
            SyncRecordSemanticState::Tombstone { delete_hlc } => {
                LocalSyncSemanticState::Tombstone { delete_hlc }
            }
        },
    }
}

pub(super) fn local_record_to_storage(
    collection: SyncCollection,
    record_id: Uuid,
    state: LocalSyncRecordState,
    updated_at: i64,
) -> SyncRecordState {
    SyncRecordState {
        record_id,
        collection: collection.to_string(),
        current_revision_hlc: state.current_revision_hlc,
        state: match state.state {
            LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            } => SyncRecordSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
            LocalSyncSemanticState::Tombstone { delete_hlc } => {
                SyncRecordSemanticState::Tombstone { delete_hlc }
            }
        },
        updated_at,
    }
}

pub(super) fn storage_alias_to_local(alias: ListAlias) -> LocalListAlias {
    LocalListAlias {
        alias_list_id: alias.alias_list_id,
        canonical_list_id: alias.canonical_list_id,
    }
}
