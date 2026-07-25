use super::*;

pub(super) fn compare_encoded_hlc(left: &str, right: &str) -> Result<std::cmp::Ordering, String> {
    let left = Hlc::decode(left).map_err(|_| "sync failed".to_string())?;
    let right = Hlc::decode(right).map_err(|_| "sync failed".to_string())?;
    Ok(left.cmp(&right))
}

pub(super) fn decrypt_task_plaintext(
    record: &PullRecord,
    existing: Option<&Task>,
    keys: &LocalSyncKeys,
) -> Result<SyncPlaintext, ApplyDisposition> {
    let EncryptedSyncState::Live { blob, .. } = &record.state else {
        return Err(ApplyDisposition::Deferred(
            PullFailureReason::CorruptEnvelope,
            None,
        ));
    };
    let header = crate::parse_envelope_header(blob)
        .map_err(|error| classify_envelope_error(error, None, 0))?;
    let expected_list_id = existing.map(|task| task.list_id);
    let Some(tenant_key) = crate::tenant_root_dek_for_generation(keys, header.key_generation)
    else {
        return Err(ApplyDisposition::Deferred(
            PullFailureReason::MissingDek,
            expected_list_id,
        ));
    };
    decrypt_plaintext(
        tenant_key,
        keys.tenant_id,
        header.key_generation,
        TASKS_COLLECTION,
        record.record_id,
        blob,
    )
    .map_err(|error| {
        classify_envelope_error(error, expected_list_id, blob.first().copied().unwrap_or(0))
    })
}

pub(super) fn classify_envelope_error(
    error: EnvelopeError,
    required_list_id: Option<Uuid>,
    envelope_version: u8,
) -> ApplyDisposition {
    match error {
        EnvelopeError::UnsupportedVersion => ApplyDisposition::UpgradeRequired(envelope_version),
        EnvelopeError::Crypto(_) => {
            ApplyDisposition::Deferred(PullFailureReason::AuthenticationFailed, required_list_id)
        }
        EnvelopeError::Deserialization | EnvelopeError::Serialization => {
            ApplyDisposition::Deferred(PullFailureReason::InvalidPlaintext, required_list_id)
        }
        EnvelopeError::BlobTooShort
        | EnvelopeError::BlobTooLarge
        | EnvelopeError::UnsupportedSuite
        | EnvelopeError::InvalidGeneration
        | EnvelopeError::InvalidIdentity
        | EnvelopeError::CollectionTooLong
        | EnvelopeError::KeyDerivation => {
            ApplyDisposition::Deferred(PullFailureReason::CorruptEnvelope, required_list_id)
        }
    }
}

pub(super) fn upgrade_block_value(protocol_version: u16, envelope_version: u8) -> String {
    format!("{protocol_version}:{envelope_version}")
}

pub(super) fn upgrade_block_is_active(value: &str) -> bool {
    let Some((protocol, envelope)) = value.split_once(':') else {
        return true;
    };
    let (Ok(protocol), Ok(envelope)) = (protocol.parse::<u16>(), envelope.parse::<u8>()) else {
        return true;
    };
    crate::protocol::SYNC_PROTOCOL_VERSION < protocol || crate::ENVELOPE_VERSION < envelope
}

pub(super) fn replay_upgrade_version(error: &str) -> Option<u8> {
    error
        .strip_prefix("upgrade required:")
        .and_then(|value| value.parse().ok())
}

pub(super) fn record_hlc_or_initial(plaintext: &SyncPlaintext) -> Hlc {
    plaintext.record_hlc().clone()
}

pub(super) fn task_from_plaintext<N>(
    id: Uuid,
    _existing: Option<&Task>,
    plaintext: &SyncPlaintext,
    _now_ms: &mut N,
) -> Result<Task, String>
where
    N: FnMut() -> Result<i64, String>,
{
    plaintext
        .validate_for_collection(TASKS_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::Task(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(Task {
        id,
        list_id: fields.placement.value.list_id,
        parent_task_id: fields.placement.value.parent_task_id,
        content: TaskContent {
            title: fields.title.value.clone(),
            note: fields.note.value.clone(),
            priority: fields.priority.value,
            estimated_minutes: fields.estimated_minutes.value,
        },
        status: fields.completion.value.status,
        due: fields.due.value.clone(),
        scheduled_at: fields.scheduled_at.value,
        sort_order: fields.placement.value.rank.clone(),
        completed_at: fields.completion.value.completed_at,
        closed_reason: fields.completion.value.closed_reason.clone(),
        deleted_at: None,
        assignee: fields.assignee.value,
        series_occurrence: fields.series_occurrence.value.clone(),
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}

pub(super) fn template_from_plaintext(
    id: Uuid,
    plaintext: &SyncPlaintext,
) -> Result<TaskTemplate, String> {
    plaintext
        .validate_for_collection(TEMPLATES_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::Template(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(TaskTemplate {
        id,
        name: fields.name.value.clone(),
        default_list_id: fields.default_list_id.value,
        blueprint: fields.blueprint.value.blueprint.clone(),
        blueprint_revision: fields.blueprint.value.revision.clone(),
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}

pub(super) fn task_series_from_plaintext(
    id: Uuid,
    plaintext: &SyncPlaintext,
) -> Result<TaskSeries, String> {
    plaintext
        .validate_for_collection(TASK_SERIES_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::TaskSeries(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(TaskSeries {
        id,
        config: TaskSeriesConfig {
            blueprint: fields.config.value.blueprint.clone(),
            target_list_id: fields.config.value.target_list_id,
            rrule: fields.config.value.rrule.clone(),
            starts_at: fields.config.value.starts_at,
            time_zone: fields.config.value.time_zone.clone(),
            enabled: fields.config.value.enabled,
            config_revision: fields.config.value.revision.clone(),
            config_parent_revision: fields.config.value.parent_revision.clone(),
            config_effective_from: fields.config.value.effective_from,
            lineage: fields.config.value.lineage.clone(),
        },
        cursor: fields.cursor.value.cursor,
        created_at: fields.config.value.created_at,
        updated_at: fields.config.value.updated_at,
    })
}

pub(super) fn list_from_plaintext<N>(
    id: Uuid,
    _existing: Option<&List>,
    plaintext: &SyncPlaintext,
    _now_ms: &mut N,
) -> Result<List, String>
where
    N: FnMut() -> Result<i64, String>,
{
    plaintext
        .validate_for_collection(LISTS_COLLECTION, &id.to_string())
        .map_err(|_| "sync failed".to_string())?;
    let SyncPlaintext::List(fields) = plaintext else {
        return Err("sync failed".to_string());
    };
    Ok(List {
        id,
        name: fields.name.value.clone(),
        color: fields.color.value.clone(),
        icon: fields.icon.value.clone(),
        sort_order: fields.placement.value.rank.clone(),
        is_default: fields.is_default.value,
        archived_at: fields.archived_at.value,
        created_at: fields.created_at.value,
        updated_at: fields.updated_at.value,
    })
}
