use super::*;

pub(super) async fn run_full_resync<S, N, R>(
    engine: &SyncEngine,
    context: &mut ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    key_refresher: &mut R,
    summary: &mut SyncRunSummary,
) -> Result<bool, String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    R: SyncKeyRefresher,
{
    let mut refreshed_keys = false;
    let mut resync_restart_performed = false;

    loop {
        if store
            .load_full_resync()
            .map_err(normalize_local_sync_error)?
            .is_none()
        {
            store.preflight_network_request()?;
            let start = engine
                .begin_full_resync()
                .await
                .map_err(sync_engine_error_to_string)?;
            let mut transaction = store
                .begin_write_transaction()
                .map_err(normalize_local_sync_error)?;
            transaction
                .start_full_resync(Uuid::now_v7(), start.generation, start.base_seq, now_ms()?)
                .map_err(normalize_local_sync_error)?;
            transaction.set_setting(
                SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY,
                &start.page_token,
                now_ms()?,
            )?;
            transaction.set_setting(
                SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY,
                "",
                now_ms()?,
            )?;
            transaction.commit().map_err(normalize_local_sync_error)?;
        }
        let progress = store
            .load_full_resync()
            .map_err(normalize_local_sync_error)?
            .ok_or_else(|| "sync failed".to_string())?;
        match progress.phase {
            LocalFullResyncPhase::Base => {
                let page_token = store
                    .get_setting(SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY)?
                    .filter(|token| !token.is_empty())
                    .ok_or_else(|| "sync failed".to_string())?;
                store.preflight_network_request()?;
                let page = match engine
                    .scan_base_page(
                        &page_token,
                        progress.base_cursor.as_ref(),
                        FULL_RESYNC_PAGE_LIMIT,
                    )
                    .await
                {
                    Ok(page) => page,
                    Err(SyncEngineError::ResyncRestartRequired) => {
                        restart_invalid_resync_once(store, now_ms, &mut resync_restart_performed)?;
                        continue;
                    }
                    Err(error) => return Err(sync_engine_error_to_string(error)),
                };
                if page.has_more && page.next_cursor.is_none() {
                    return Err("sync failed".to_string());
                }
                let base_complete = !page.has_more;
                let page_updated_at = now_ms()?;
                let apply = apply_full_resync_page(
                    &page.records,
                    context,
                    store,
                    now_ms,
                    false,
                    |transaction| {
                        transaction.advance_full_resync_base(
                            progress.generation_id,
                            page.next_cursor.as_ref(),
                            base_complete,
                            page_updated_at,
                        )?;
                        if let Some(next_page_token) = page.next_page_token.as_deref() {
                            transaction.set_setting(
                                SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY,
                                next_page_token,
                                page_updated_at,
                            )?;
                        } else {
                            transaction.set_setting(
                                SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY,
                                "",
                                page_updated_at,
                            )?;
                        }
                        if let Some(completion_token) = page.completion_token.as_deref() {
                            transaction.set_setting(
                                SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY,
                                completion_token,
                                page_updated_at,
                            )?;
                        }
                        Ok(())
                    },
                );
                let page_summary = match apply {
                    Ok(summary) => summary,
                    Err(PageApplyError::MissingKey) => {
                        store.preflight_network_request()?;
                        context.keys = key_refresher.refresh().await?;
                        refreshed_keys = true;
                        match apply_full_resync_page(
                            &page.records,
                            context,
                            store,
                            now_ms,
                            true,
                            |transaction| {
                                transaction.advance_full_resync_base(
                                    progress.generation_id,
                                    page.next_cursor.as_ref(),
                                    base_complete,
                                    page_updated_at,
                                )?;
                                if let Some(next_page_token) = page.next_page_token.as_deref() {
                                    transaction.set_setting(
                                        SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY,
                                        next_page_token,
                                        page_updated_at,
                                    )?;
                                } else {
                                    transaction.set_setting(
                                        SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY,
                                        "",
                                        page_updated_at,
                                    )?;
                                }
                                if let Some(completion_token) = page.completion_token.as_deref() {
                                    transaction.set_setting(
                                        SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY,
                                        completion_token,
                                        page_updated_at,
                                    )?;
                                }
                                Ok(())
                            },
                        ) {
                            Ok(summary) => summary,
                            Err(PageApplyError::UpgradeRequired(version)) => {
                                persist_full_resync_upgrade_block(store, now_ms, version)?;
                                return Err("upgrade required".to_string());
                            }
                            Err(error) => return Err(page_apply_error_to_string(error)),
                        }
                    }
                    Err(PageApplyError::UpgradeRequired(version)) => {
                        persist_full_resync_upgrade_block(store, now_ms, version)?;
                        return Err("upgrade required".to_string());
                    }
                    Err(error) => return Err(page_apply_error_to_string(error)),
                };
                merge_summary(summary, page_summary);
            }
            LocalFullResyncPhase::BaseAwaitingAck => {
                let completion_token = store
                    .get_setting(SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY)?
                    .filter(|token| !token.is_empty())
                    .ok_or_else(|| "sync failed".to_string())?;
                store.preflight_network_request()?;
                match engine.complete_resync_base(&completion_token).await {
                    Ok(()) => {}
                    Err(SyncEngineError::ResyncRestartRequired) => {
                        restart_invalid_resync_once(store, now_ms, &mut resync_restart_performed)?;
                        continue;
                    }
                    Err(error) => return Err(sync_engine_error_to_string(error)),
                }
                let mut transaction = store.begin_write_transaction()?;
                transaction.set_setting(
                    SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY,
                    "",
                    now_ms()?,
                )?;
                transaction.commit()?;
            }
            LocalFullResyncPhase::Delta => {
                store.preflight_network_request()?;
                let page = engine
                    .pull_page_for_generation(
                        progress.delta_cursor,
                        FULL_RESYNC_PAGE_LIMIT,
                        Some(progress.continuity_generation),
                    )
                    .await
                    .map_err(sync_engine_error_to_string)?;
                let reached_closure = page.reached_closure();
                let page_updated_at = now_ms()?;
                let apply = apply_full_resync_page(
                    &page.records,
                    context,
                    store,
                    now_ms,
                    false,
                    |transaction| {
                        transaction.advance_full_resync_delta(
                            progress.generation_id,
                            page.next_since,
                            page_updated_at,
                        )?;
                        if reached_closure {
                            transaction.enter_full_resync_sweep(
                                progress.generation_id,
                                page.high_water,
                                page_updated_at,
                            )?;
                        }
                        Ok(())
                    },
                );
                let page_summary = match apply {
                    Ok(summary) => summary,
                    Err(PageApplyError::MissingKey) => {
                        store.preflight_network_request()?;
                        context.keys = key_refresher.refresh().await?;
                        refreshed_keys = true;
                        match apply_full_resync_page(
                            &page.records,
                            context,
                            store,
                            now_ms,
                            true,
                            |transaction| {
                                transaction.advance_full_resync_delta(
                                    progress.generation_id,
                                    page.next_since,
                                    page_updated_at,
                                )?;
                                if reached_closure {
                                    transaction.enter_full_resync_sweep(
                                        progress.generation_id,
                                        page.high_water,
                                        page_updated_at,
                                    )?;
                                }
                                Ok(())
                            },
                        ) {
                            Ok(summary) => summary,
                            Err(PageApplyError::UpgradeRequired(version)) => {
                                persist_full_resync_upgrade_block(store, now_ms, version)?;
                                return Err("upgrade required".to_string());
                            }
                            Err(error) => return Err(page_apply_error_to_string(error)),
                        }
                    }
                    Err(PageApplyError::UpgradeRequired(version)) => {
                        persist_full_resync_upgrade_block(store, now_ms, version)?;
                        return Err("upgrade required".to_string());
                    }
                    Err(error) => return Err(page_apply_error_to_string(error)),
                };
                merge_summary(summary, page_summary);
            }
            LocalFullResyncPhase::Sweep => {
                let mut transaction = store
                    .begin_write_transaction()
                    .map_err(normalize_local_sync_error)?;
                let swept = transaction
                    .sweep_full_resync_batch(
                        progress.generation_id,
                        FULL_RESYNC_SWEEP_BATCH_LIMIT,
                        now_ms()?,
                    )
                    .map_err(normalize_local_sync_error)?;
                transaction.commit().map_err(normalize_local_sync_error)?;
                summary.deleted_count += swept.swept_lists
                    + swept.swept_tasks
                    + swept.swept_templates
                    + swept.swept_task_series
                    + swept.swept_timer_sessions;
                if swept.scanned_records == 0 {
                    let mut transaction = store
                        .begin_write_transaction()
                        .map_err(normalize_local_sync_error)?;
                    let high_water = transaction
                        .finalize_full_resync(progress.generation_id, SYNC_CURSOR_NAME, now_ms()?)
                        .map_err(normalize_local_sync_error)?;
                    transaction.commit().map_err(normalize_local_sync_error)?;
                    store.preflight_network_request()?;
                    let closure = engine
                        .pull_page_for_generation(
                            high_water,
                            FULL_RESYNC_PAGE_LIMIT,
                            Some(progress.continuity_generation),
                        )
                        .await
                        .map_err(sync_engine_error_to_string)?;
                    let proof = closure
                        .closure_proof
                        .clone()
                        .filter(|_| closure.reached_closure())
                        .ok_or_else(|| "sync failed".to_string())?;
                    store.preflight_network_request()?;
                    engine
                        .ack_continuity(proof)
                        .await
                        .map_err(sync_engine_error_to_string)?;
                    return Ok(refreshed_keys);
                }
            }
        }
    }
}

fn restart_invalid_resync_once<S, N>(
    store: &mut S,
    now_ms: &mut N,
    resync_restart_performed: &mut bool,
) -> Result<(), String>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
{
    if *resync_restart_performed {
        return Err("sync failed".to_string());
    }
    let reset_at = now_ms()?;
    let mut transaction = store.begin_write_transaction()?;
    transaction.reset_full_resync()?;
    transaction.set_setting(SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY, "", reset_at)?;
    transaction.set_setting(SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY, "", reset_at)?;
    transaction.commit()?;
    *resync_restart_performed = true;
    Ok(())
}

fn persist_full_resync_upgrade_block<S, N>(
    store: &mut S,
    now_ms: &mut N,
    envelope_version: u8,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    store.set_setting(
        SYNC_UPGRADE_REQUIRED_SETTING_KEY,
        &upgrade_block_value(crate::protocol::SYNC_PROTOCOL_VERSION, envelope_version),
        now_ms()?,
    )
}

fn apply_full_resync_page<S, N, F>(
    records: &[PullRecord],
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    quarantine_missing: bool,
    finish: F,
) -> Result<SyncRunSummary, PageApplyError>
where
    S: LocalSyncAtomicStore,
    N: FnMut() -> Result<i64, String>,
    F: FnOnce(&mut S::WriteTransaction) -> Result<(), String>,
{
    let progress = store
        .load_full_resync()
        .map_err(PageApplyError::Hard)?
        .ok_or_else(|| PageApplyError::Hard("sync failed".to_string()))?;
    let mut transaction = store
        .begin_write_transaction()
        .map_err(PageApplyError::Hard)?;
    let mut page_summary = SyncRunSummary {
        pulled_count: records.len(),
        ..SyncRunSummary::default()
    };
    for record in records {
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
        // Server presence is independent of decrypt/quarantine success.
        transaction
            .mark_full_resync_record(progress.generation_id, record.collection, record.record_id)
            .map_err(PageApplyError::Hard)?;
    }
    finish(&mut transaction).map_err(PageApplyError::Hard)?;
    transaction.commit().map_err(PageApplyError::Hard)?;
    Ok(page_summary)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PageApplyError {
    MissingKey,
    UpgradeRequired(u8),
    Hard(String),
}

pub(super) fn page_apply_error_to_string(error: PageApplyError) -> String {
    match error {
        PageApplyError::UpgradeRequired(_) => "upgrade required".to_string(),
        PageApplyError::MissingKey => "sync failed".to_string(),
        PageApplyError::Hard(error) => normalize_local_sync_error(error),
    }
}

fn normalize_local_sync_error(error: String) -> String {
    match error.as_str() {
        "sync lease busy" | "sync lease lost" | "database busy" => error,
        _ => "sync failed".to_string(),
    }
}

pub(super) fn merge_summary(target: &mut SyncRunSummary, page: SyncRunSummary) {
    target.pulled_count += page.pulled_count;
    target.applied_count += page.applied_count;
    target.deleted_count += page.deleted_count;
    target.decrypt_failed_count += page.decrypt_failed_count;
    target.repush_count += page.repush_count;
    target.missing_key_quarantined_count += page.missing_key_quarantined_count;
    target.corruption_quarantined_count += page.corruption_quarantined_count;
    target.resolved_quarantine_count += page.resolved_quarantine_count;
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn page_apply_preserves_coordination_error_categories() {
        for expected in ["sync lease busy", "sync lease lost", "database busy"] {
            assert_eq!(
                page_apply_error_to_string(PageApplyError::Hard(expected.to_string())),
                expected
            );
        }
        assert_eq!(
            page_apply_error_to_string(PageApplyError::Hard("sqlite details".to_string())),
            "sync failed"
        );
    }
}
