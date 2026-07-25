use super::*;

pub(super) fn apply_pull_record<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    match record.collection {
        SyncCollection::Lists => apply_pull_list(record, context, store, now_ms, summary),
        SyncCollection::Tasks => apply_pull_task(record, context, store, now_ms, summary),
        SyncCollection::Templates => apply_pull_template(record, context, store, now_ms, summary),
        SyncCollection::TaskSeries => {
            apply_pull_task_series(record, context, store, now_ms, summary)
        }
        SyncCollection::TimerSessions => {
            apply_pull_timer_session(record, context, store, now_ms, summary)
        }
    }
}

pub(super) fn apply_pull_timer_session<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::TimerSessions, record.record_id)?;
    if let Some(LocalSyncRecordState {
        state: LocalSyncSemanticState::Tombstone { delete_hlc },
        ..
    }) = local_state.as_ref()
    {
        enqueue_rebased_tombstone(
            store,
            RebaseTombstoneRequest {
                record_id: record.record_id,
                collection: SyncCollection::TimerSessions,
                delete_hlc,
                device_id: &context.device_id,
                base_revision_hlc: Some(&record.revision_hlc),
            },
            now_ms,
        )?;
        return Ok(ApplyDisposition::Rebased);
    }

    match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::TimerSessions, record.record_id)?;
            store.delete_timer_session_for_sync(record.record_id)?;
            store.put_record_state(
                SyncCollection::TimerSessions,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            summary.deleted_count += 1;
            Ok(ApplyDisposition::AppliedCurrent)
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            let header = match crate::parse_envelope_header(blob) {
                Ok(header) => header,
                Err(error) => {
                    return Ok(classify_envelope_error(
                        error,
                        None,
                        blob.first().copied().unwrap_or(0),
                    ))
                }
            };
            let Some(dek) =
                crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
            else {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::MissingDek,
                    None,
                ));
            };
            let plaintext = decrypt_plaintext(
                dek,
                context.tenant_id,
                header.key_generation,
                crate::TIMER_SESSIONS_COLLECTION,
                record.record_id,
                blob,
            )
            .map_err(|error| {
                classify_envelope_error(error, None, blob.first().copied().unwrap_or(0))
            });
            let plaintext = match plaintext {
                Ok(value) => value,
                Err(disposition) => return Ok(disposition),
            };
            let SyncPlaintext::TimerSession(incoming) = &plaintext else {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::InvalidPlaintext,
                    None,
                ));
            };
            let task_state =
                store.get_record_state(SyncCollection::Tasks, incoming.value.task_id)?;
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = task_state
            {
                enqueue_rebased_tombstone(
                    store,
                    RebaseTombstoneRequest {
                        record_id: record.record_id,
                        collection: SyncCollection::TimerSessions,
                        delete_hlc: &delete_hlc,
                        device_id: &context.device_id,
                        base_revision_hlc: Some(&record.revision_hlc),
                    },
                    now_ms,
                )?;
                return Ok(ApplyDisposition::Rebased);
            }
            if store.get_task(incoming.value.task_id)?.is_none() {
                return Ok(ApplyDisposition::Deferred(
                    PullFailureReason::MissingDependency,
                    Some(incoming.value.task_id),
                ));
            }
            if let Some(existing) = store.get_timer_session(record.record_id)? {
                if existing != incoming.value {
                    return Ok(ApplyDisposition::Deferred(
                        PullFailureReason::InvalidPlaintext,
                        None,
                    ));
                }
            } else {
                store.upsert_timer_session_for_sync(incoming.value.clone())?;
            }
            store.delete_outbox_head(SyncCollection::TimerSessions, record.record_id)?;
            store.put_record_state(
                SyncCollection::TimerSessions,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Live {
                        mutation_hlc: mutation_hlc.clone(),
                        plaintext_json: serde_json::to_string(&plaintext)
                            .map_err(|_| "sync failed".to_string())?,
                    },
                },
                now_ms()?,
            )?;
            summary.applied_count += 1;
            if header.key_generation < context.keys.tenant_generation {
                enqueue_timer_session_sync(
                    store,
                    &context.keys,
                    &context.device_id,
                    &incoming.value,
                    false,
                    now_ms,
                )?;
                summary.repush_count += 1;
                Ok(ApplyDisposition::Rebased)
            } else {
                Ok(ApplyDisposition::AppliedCurrent)
            }
        }
    }
}

pub(super) fn apply_pull_template<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::Templates, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Templates, record.record_id)?;
            if store.delete_template_for_sync(record.record_id)? {
                summary.deleted_count += 1;
            }
            store.put_record_state(
                SyncCollection::Templates,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Templates,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            None,
        ));
    };
    let incoming = match decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        TEMPLATES_COLLECTION,
        record.record_id,
        blob,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let existing = store.get_template(record.record_id)?;
    let stored_plaintext =
        stored_sync_plaintext(store, SyncCollection::Templates, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local), _) => {
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local = template_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let template = template_from_plaintext(record.record_id, &merged)?;
    store.upsert_template_for_sync(template)?;
    store_sync_plaintext(
        store,
        SyncCollection::Templates,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Templates,
                plaintext: &merged,
                dek: tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

pub(super) fn apply_pull_task_series<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::TaskSeries, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::TaskSeries, record.record_id)?;
            if store.delete_series_for_sync(record.record_id)? {
                summary.deleted_count += 1;
            }
            store.put_record_state(
                SyncCollection::TaskSeries,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::TaskSeries,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            None,
        ));
    };
    let incoming = match decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        TASK_SERIES_COLLECTION,
        record.record_id,
        blob,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                None,
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let existing = store.get_series(record.record_id)?;
    let stored_plaintext =
        stored_sync_plaintext(store, SyncCollection::TaskSeries, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local), _) => {
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local = task_series_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let series = task_series_from_plaintext(record.record_id, &merged)?;
    store.upsert_series_for_sync(series)?;
    store_sync_plaintext(
        store,
        SyncCollection::TaskSeries,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::TaskSeries,
                plaintext: &merged,
                dek: tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

pub(super) fn apply_pull_list<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let local_state = store.get_record_state(SyncCollection::Lists, record.record_id)?;
    let (incoming_mutation_hlc, blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Lists, record.record_id)?;
            let default_list_id = store
                .default_list_id()?
                .filter(|default_id| *default_id != record.record_id)
                .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
            let mut known_tasks = store.list_tasks_by_list_for_sync(record.record_id)?;
            for task in &mut known_tasks {
                store.delete_outbox_head(SyncCollection::Tasks, task.id)?;
                task.list_id = default_list_id;
                task.updated_at = now_ms()?;
                store.upsert_task_for_sync(task.clone())?;
                enqueue_task_sync(
                    store,
                    &context.keys,
                    &context.device_id,
                    task,
                    false,
                    now_ms,
                )?;
                summary.repush_count += 1;
            }
            store.delete_list_and_rehome_tasks_for_sync(record.record_id)?;
            summary.deleted_count += 1;
            store.put_record_state(
                SyncCollection::Lists,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Lists,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let header = match crate::parse_envelope_header(blob) {
        Ok(header) => header,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                Some(record.record_id),
                blob.first().copied().unwrap_or(0),
            ))
        }
    };
    let Some(dek) = crate::tenant_root_dek_for_generation(&context.keys, header.key_generation)
    else {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            Some(record.record_id),
        ));
    };
    let incoming = decrypt_plaintext(
        dek,
        context.tenant_id,
        header.key_generation,
        LISTS_COLLECTION,
        record.record_id,
        blob,
    );
    let incoming = match incoming {
        Ok(incoming) => incoming,
        Err(error) => {
            return Ok(classify_envelope_error(
                error,
                Some(record.record_id),
                blob.first().copied().unwrap_or(0),
            ));
        }
    };
    let existing = store.get_list(record.record_id)?;
    let stored_plaintext = stored_sync_plaintext(store, SyncCollection::Lists, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local_plaintext), _) => {
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local_plaintext = list_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || header.key_generation < context.keys.tenant_generation;
    let mut list = list_from_plaintext(record.record_id, existing.as_ref(), &merged, now_ms)?;
    if list.is_default {
        if let Some(default_list_id) = store.default_list_id()? {
            if default_list_id != list.id {
                if default_list_id.as_bytes() < list.id.as_bytes() {
                    // Preserve the authenticated candidate identity in record
                    // state, while keeping the domain UNIQUE index valid until
                    // the closure-level canonical transaction runs.
                    list.is_default = false;
                } else {
                    let mut previous = store
                        .get_list(default_list_id)?
                        .ok_or_else(|| "sync failed".to_string())?;
                    previous.is_default = false;
                    store.upsert_list_for_sync(previous)?;
                }
            }
        }
    }
    store.upsert_list_for_sync(list)?;
    store_sync_plaintext(
        store,
        SyncCollection::Lists,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if needs_repush {
        let active_dek = tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?;
        let active_generation = context.keys.tenant_generation;
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Lists,
                plaintext: &merged,
                dek: active_dek,
                tenant_id: context.tenant_id,
                generation: active_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

pub(super) fn apply_pull_task<S, N>(
    record: &PullRecord,
    context: &ActiveSyncContext,
    store: &mut S,
    now_ms: &mut N,
    summary: &mut SyncRunSummary,
) -> Result<ApplyDisposition, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    observe_remote_hlc(store, &context.device_id, &record.revision_hlc, now_ms)?;
    let existing = store.get_task(record.record_id)?;
    let local_state = store.get_record_state(SyncCollection::Tasks, record.record_id)?;
    let (incoming_mutation_hlc, _blob) = match &record.state {
        EncryptedSyncState::Tombstone { delete_hlc } => {
            store.delete_outbox_head(SyncCollection::Tasks, record.record_id)?;
            let known_tasks = store.list_task_subtree_for_sync(record.record_id)?;
            summary.deleted_count += cascade_timer_sessions_for_tasks(
                store,
                &known_tasks,
                delete_hlc,
                &context.device_id,
                now_ms,
            )?;
            let deleted = store.delete_task_subtree_for_sync(record.record_id)?;
            summary.deleted_count += deleted;
            store.put_record_state(
                SyncCollection::Tasks,
                record.record_id,
                LocalSyncRecordState {
                    current_revision_hlc: Some(record.revision_hlc.clone()),
                    state: LocalSyncSemanticState::Tombstone {
                        delete_hlc: delete_hlc.clone(),
                    },
                },
                now_ms()?,
            )?;
            return Ok(ApplyDisposition::AppliedCurrent);
        }
        EncryptedSyncState::Live { mutation_hlc, blob } => {
            if let Some(LocalSyncRecordState {
                state: LocalSyncSemanticState::Tombstone { delete_hlc },
                ..
            }) = local_state.as_ref()
            {
                if compare_encoded_hlc(delete_hlc, mutation_hlc)? != std::cmp::Ordering::Less {
                    enqueue_rebased_tombstone(
                        store,
                        RebaseTombstoneRequest {
                            record_id: record.record_id,
                            collection: SyncCollection::Tasks,
                            delete_hlc,
                            device_id: &context.device_id,
                            base_revision_hlc: Some(&record.revision_hlc),
                        },
                        now_ms,
                    )?;
                    summary.repush_count += 1;
                    return Ok(ApplyDisposition::Rebased);
                }
            }
            (mutation_hlc, blob)
        }
    };
    let incoming_generation = match &record.state {
        EncryptedSyncState::Live { blob, .. } => {
            crate::parse_envelope_header(blob)
                .map_err(|_| "sync failed".to_string())?
                .key_generation
        }
        EncryptedSyncState::Tombstone { .. } => return Err("sync failed".to_string()),
    };
    let incoming = match decrypt_task_plaintext(record, existing.as_ref(), &context.keys) {
        Ok(incoming) => incoming,
        Err(disposition) => return Ok(disposition),
    };
    match &incoming {
        SyncPlaintext::Task(_) => {}
        SyncPlaintext::List(_)
        | SyncPlaintext::Template(_)
        | SyncPlaintext::TaskSeries(_)
        | SyncPlaintext::TimerSession(_) => return Err("sync failed".to_string()),
    }
    let dek = tenant_root_dek(&context.keys).ok_or_else(|| "sync failed".to_string())?;
    let stored_plaintext = stored_sync_plaintext(store, SyncCollection::Tasks, record.record_id)?;
    let (merged, needs_repush) = match (stored_plaintext, existing.as_ref()) {
        (Some(local_plaintext), _) => {
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, Some(local)) => {
            let local_plaintext = task_plaintext(local, record_hlc_or_initial(&incoming));
            let merge = merge_lww(&local_plaintext, &incoming).map_err(|_| "sync failed")?;
            let needs_repush = merge.needs_repush();
            (merge.plaintext, needs_repush)
        }
        (None, None) => (incoming, false),
    };
    let needs_repush = needs_repush || incoming_generation < context.keys.tenant_generation;
    let mut task = task_from_plaintext(record.record_id, existing.as_ref(), &merged, now_ms)?;
    let authenticated_list_id = task.list_id;
    let resolved_list_id = store.resolve_list_alias(authenticated_list_id)?;
    let resolved_alias = resolved_list_id != authenticated_list_id;
    task.list_id = resolved_list_id;
    let dependency = task_dependency_disposition(store, &task)?;
    if matches!(dependency, TaskDependencyDisposition::Missing)
        && store.load_full_resync()?.is_some()
    {
        return Ok(ApplyDisposition::Deferred(
            PullFailureReason::MissingDependency,
            Some(task.list_id),
        ));
    }
    if let TaskDependencyDisposition::Deleted = dependency {
        task.list_id = store
            .default_list_id()?
            .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
        task.updated_at = now_ms()?;
        store.upsert_task_for_sync(task.clone())?;
        store_sync_plaintext(
            store,
            SyncCollection::Tasks,
            record.record_id,
            &record.revision_hlc,
            incoming_mutation_hlc,
            &merged,
            now_ms,
        )?;
        enqueue_task_sync(
            store,
            &context.keys,
            &context.device_id,
            &task,
            false,
            now_ms,
        )?;
        summary.repush_count += 1;
        return Ok(ApplyDisposition::Rebased);
    }
    if matches!(dependency, TaskDependencyDisposition::Missing) {
        let delete_hlc = record.revision_hlc.clone();
        store.delete_outbox_head(SyncCollection::Tasks, record.record_id)?;
        let known_tasks = store.list_task_subtree_for_sync(record.record_id)?;
        summary.deleted_count += cascade_timer_sessions_for_tasks(
            store,
            &known_tasks,
            &delete_hlc,
            &context.device_id,
            now_ms,
        )?;
        let deleted = store.delete_task_subtree_for_sync(record.record_id)?;
        summary.deleted_count += deleted;
        enqueue_rebased_tombstone(
            store,
            RebaseTombstoneRequest {
                record_id: record.record_id,
                collection: SyncCollection::Tasks,
                delete_hlc: &delete_hlc,
                device_id: &context.device_id,
                base_revision_hlc: Some(&record.revision_hlc),
            },
            now_ms,
        )?;
        summary.repush_count += 1;
        return Ok(ApplyDisposition::Rebased);
    }
    store.upsert_task_for_sync(task.clone())?;
    store_sync_plaintext(
        store,
        SyncCollection::Tasks,
        record.record_id,
        &record.revision_hlc,
        incoming_mutation_hlc,
        &merged,
        now_ms,
    )?;
    summary.applied_count += 1;
    if resolved_alias {
        // Persist the authenticated remote merge first, then reuse the normal
        // mutation enqueue path to stamp only placement, reuse the Tenant
        // generation, and replace any stale outbox head transactionally.
        enqueue_task_sync(
            store,
            &context.keys,
            &context.device_id,
            &task,
            false,
            now_ms,
        )?;
        summary.repush_count += 1;
    } else if needs_repush {
        enqueue_merged_plaintext(
            store,
            RebasePlaintextRequest {
                record_id: record.record_id,
                collection: SyncCollection::Tasks,
                plaintext: &merged,
                dek,
                tenant_id: context.tenant_id,
                generation: context.keys.tenant_generation,
                device_id: &context.device_id,
                base_revision_hlc: &record.revision_hlc,
            },
            now_ms,
        )?;
        summary.repush_count += 1;
    }
    Ok(if needs_repush || resolved_alias {
        ApplyDisposition::Rebased
    } else {
        ApplyDisposition::AppliedCurrent
    })
}

fn cascade_timer_sessions_for_tasks<S, N>(
    store: &mut S,
    tasks: &[Task],
    delete_hlc: &str,
    device_id: &str,
    now_ms: &mut N,
) -> Result<usize, String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let mut deleted = 0;
    for task in tasks {
        for session in store.list_timer_sessions_by_task(task.id)? {
            store.delete_outbox_head(SyncCollection::TimerSessions, session.id)?;
            let base_revision = store
                .get_record_state(SyncCollection::TimerSessions, session.id)?
                .and_then(|state| state.current_revision_hlc);
            enqueue_rebased_tombstone(
                store,
                RebaseTombstoneRequest {
                    record_id: session.id,
                    collection: SyncCollection::TimerSessions,
                    delete_hlc,
                    device_id,
                    base_revision_hlc: base_revision.as_deref(),
                },
                now_ms,
            )?;
            deleted += usize::from(store.delete_timer_session_for_sync(session.id)?);
        }
        store.clear_active_timer_for_task(task.id)?;
    }
    Ok(deleted)
}

fn task_dependency_disposition<S>(
    store: &mut S,
    task: &Task,
) -> Result<TaskDependencyDisposition, String>
where
    S: LocalSyncStore,
{
    if let Some(LocalSyncRecordState {
        state: LocalSyncSemanticState::Tombstone { delete_hlc },
        ..
    }) = store.get_record_state(SyncCollection::Lists, task.list_id)?
    {
        let _ = delete_hlc;
        return Ok(TaskDependencyDisposition::Deleted);
    }
    if store.get_list(task.list_id)?.is_none() {
        return Ok(TaskDependencyDisposition::Missing);
    }

    let mut parent_id = task.parent_task_id;
    let mut visited = HashSet::new();
    while let Some(id) = parent_id {
        if !visited.insert(id) {
            return Ok(TaskDependencyDisposition::Missing);
        }
        if let Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Tombstone { delete_hlc },
            ..
        }) = store.get_record_state(SyncCollection::Tasks, id)?
        {
            let _ = delete_hlc;
            return Ok(TaskDependencyDisposition::Deleted);
        }
        let Some(parent) = store.get_task(id)? else {
            return Ok(TaskDependencyDisposition::Missing);
        };
        if parent.list_id != task.list_id {
            return Ok(TaskDependencyDisposition::Missing);
        }
        parent_id = parent.parent_task_id;
    }
    Ok(TaskDependencyDisposition::Valid)
}

pub(super) fn stored_sync_plaintext<S>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
) -> Result<Option<SyncPlaintext>, String>
where
    S: LocalSyncStore,
{
    match store.get_record_state(collection, record_id)? {
        Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Live { plaintext_json, .. },
            ..
        }) => {
            let plaintext: SyncPlaintext =
                serde_json::from_str(&plaintext_json).map_err(|_| "sync failed".to_string())?;
            plaintext
                .validate_for_collection(collection.as_str(), &record_id.to_string())
                .map_err(|_| "sync failed".to_string())?;
            Ok(Some(plaintext))
        }
        Some(LocalSyncRecordState {
            state: LocalSyncSemanticState::Tombstone { .. },
            ..
        })
        | None => Ok(None),
    }
}

fn store_sync_plaintext<S, N>(
    store: &mut S,
    collection: SyncCollection,
    record_id: Uuid,
    current_revision_hlc: &str,
    incoming_mutation_hlc: &str,
    plaintext: &SyncPlaintext,
    now_ms: &mut N,
) -> Result<(), String>
where
    S: LocalSyncStore,
    N: FnMut() -> Result<i64, String>,
{
    let plaintext_json = serde_json::to_string(plaintext).map_err(|_| "sync failed".to_string())?;
    let merged_mutation_hlc = plaintext
        .record_hlc()
        .encode()
        .map_err(|_| "sync failed".to_string())?;
    let mutation_hlc = if compare_encoded_hlc(&merged_mutation_hlc, incoming_mutation_hlc)?
        == std::cmp::Ordering::Less
    {
        incoming_mutation_hlc.to_string()
    } else {
        merged_mutation_hlc
    };
    store.put_record_state(
        collection,
        record_id,
        LocalSyncRecordState {
            current_revision_hlc: Some(current_revision_hlc.to_string()),
            state: LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json,
            },
        },
        now_ms()?,
    )
}
