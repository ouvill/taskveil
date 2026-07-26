use super::*;

pub(super) fn apply_pull_page<S, N>(
    page: &crate::DeltaPage,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    quarantine_missing: bool,
) -> Result<SyncRunSummary, PageApplyError>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut transaction = store
        .begin_write_transaction()
        .map_err(PageApplyError::Hard)?;
    let mut page_summary = SyncRunSummary {
        pulled_count: page.records.len(),
        ..SyncRunSummary::default()
    };
    for record in &page.records {
        let disposition =
            apply_pull_record(record, context, &mut transaction, now_ms, &mut page_summary)
                .map_err(PageApplyError::Hard)?;
        match disposition {
            ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => {
                if transaction
                    .delete_quarantine(record.record_id)
                    .map_err(PageApplyError::Hard)?
                {
                    page_summary.resolved_quarantine_count += 1;
                }
            }
            ApplyDisposition::Deferred(reason, required_list_id) => {
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) && !quarantine_missing
                {
                    return Err(PageApplyError::MissingKey);
                }
                let failed_at = now_ms().map_err(PageApplyError::Hard)?;
                transaction
                    .put_quarantine(LocalSyncQuarantineEntry {
                        record_id: record.record_id,
                        collection: record.collection,
                        seq: record.seq,
                        revision_hlc: record.revision_hlc.clone(),
                        state: record.state.clone(),
                        reason,
                        required_list_id,
                        first_failed_at: failed_at,
                        last_failed_at: failed_at,
                        attempt_count: 1,
                    })
                    .map_err(PageApplyError::Hard)?;
                page_summary.decrypt_failed_count += 1;
                if matches!(
                    reason,
                    PullFailureReason::MissingDek | PullFailureReason::NoMatchingDek
                ) {
                    page_summary.missing_key_quarantined_count += 1;
                } else {
                    page_summary.corruption_quarantined_count += 1;
                }
            }
            ApplyDisposition::UpgradeRequired(version) => {
                return Err(PageApplyError::UpgradeRequired(version));
            }
        }
    }
    transaction
        .set_cursor(
            SYNC_CURSOR_NAME,
            page.next_since,
            now_ms().map_err(PageApplyError::Hard)?,
        )
        .map_err(PageApplyError::Hard)?;
    transaction.commit().map_err(PageApplyError::Hard)?;
    Ok(page_summary)
}

pub(super) fn replay_quarantine<S, N>(
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<(), String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut after = None;
    loop {
        let entries = store.list_replayable_quarantine(after, QUARANTINE_REPLAY_BATCH_LIMIT)?;
        if entries.is_empty() {
            break;
        }
        let page_len = entries.len();
        for entry in entries {
            after = Some((entry.seq, entry.record_id));
            let record = PullRecord {
                record_id: entry.record_id,
                collection: entry.collection,
                seq: entry.seq,
                revision_hlc: entry.revision_hlc,
                state: entry.state,
            };
            let mut transaction = store.begin_write_transaction()?;
            let mut replay_summary = SyncRunSummary::default();
            match apply_pull_record(
                &record,
                context,
                &mut transaction,
                now_ms,
                &mut replay_summary,
            )? {
                ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => {
                    transaction.delete_quarantine(record.record_id)?;
                    transaction.commit()?;
                    replay_summary.resolved_quarantine_count += 1;
                    merge_summary(summary, replay_summary);
                }
                ApplyDisposition::Deferred(reason, required_list_id) => {
                    let failed_at = now_ms()?;
                    transaction.put_quarantine(LocalSyncQuarantineEntry {
                        record_id: record.record_id,
                        collection: record.collection,
                        seq: record.seq,
                        revision_hlc: record.revision_hlc,
                        state: record.state,
                        reason,
                        required_list_id,
                        first_failed_at: failed_at,
                        last_failed_at: failed_at,
                        attempt_count: 1,
                    })?;
                    transaction.commit()?;
                }
                ApplyDisposition::UpgradeRequired(version) => {
                    return Err(format!("upgrade required:{version}"));
                }
            }
        }
        if page_len < QUARANTINE_REPLAY_BATCH_LIMIT {
            break;
        }
    }
    Ok(())
}

pub(super) fn update_current_revision<S, N>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
    revision_hlc: &str,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let Some(mut state) = store.get_record_state(collection, record_id)? else {
        return Err("sync failed".to_string());
    };
    state.current_revision_hlc = Some(revision_hlc.to_string());
    store.put_record_state(collection, record_id, state, now_ms()?)
}

pub(super) fn reconcile_nonaccepted_push_in_transaction<S, N>(
    current: &PullRecord,
    stale_op_id: Uuid,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    if !store.ack_outbox_op(stale_op_id)? {
        return Ok(());
    }
    match apply_pull_record(current, context, store, now_ms, summary)? {
        ApplyDisposition::AppliedCurrent | ApplyDisposition::Rebased => Ok(()),
        ApplyDisposition::Deferred(_, _) | ApplyDisposition::UpgradeRequired(_) => {
            Err("sync failed".to_string())
        }
    }
}
