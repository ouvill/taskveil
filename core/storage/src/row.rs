use crate::*;

pub(super) fn row_to_list(row: &rusqlite::Row<'_>) -> rusqlite::Result<List> {
    let id: String = row.get(0)?;

    Ok(List {
        id: parse_uuid(id, 0)?,
        name: row.get(1)?,
        color: row.get(2)?,
        icon: row.get(3)?,
        sort_order: row.get(4)?,
        archived_at: row.get(5)?,
        is_default: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub(super) fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let list_id: String = row.get(1)?;
    let parent_task_id: Option<String> = row.get(2)?;
    let status: String = row.get(5)?;
    let due_kind: Option<String> = row.get(7)?;
    let due_on: Option<String> = row.get(8)?;
    let due_at_ms: Option<i64> = row.get(9)?;
    let due_time_zone: Option<String> = row.get(10)?;
    let assignee: Option<String> = row.get(17)?;
    let series_id: Option<String> = row.get(18)?;
    let series_revision: Option<String> = row.get(19)?;
    let blueprint_node_key: Option<String> = row.get(20)?;
    let series_occurrence_at: Option<i64> = row.get(21)?;

    Ok(Task {
        id: parse_uuid(id, 0)?,
        list_id: parse_uuid(list_id, 1)?,
        parent_task_id: parse_optional_uuid(parent_task_id, 2)?,
        content: TaskContent {
            title: row.get(3)?,
            note: row.get(4)?,
            priority: row.get(6)?,
            estimated_minutes: row.get(12)?,
        },
        status: status_from_str(&status).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        due: task_due_from_columns(due_kind, due_on, due_at_ms, due_time_zone)?,
        scheduled_at: row.get(11)?,
        sort_order: row.get(13)?,
        completed_at: row.get(14)?,
        closed_reason: row.get(15)?,
        deleted_at: row.get(16)?,
        assignee: parse_optional_uuid(assignee, 17)?,
        series_occurrence: series_occurrence_from_columns(
            series_id,
            series_revision,
            blueprint_node_key,
            series_occurrence_at,
        )?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
    })
}

fn series_occurrence_from_columns(
    series_id: Option<String>,
    series_revision: Option<String>,
    blueprint_node_key: Option<String>,
    occurrence_at: Option<i64>,
) -> rusqlite::Result<Option<SeriesOccurrenceRef>> {
    match (
        series_id,
        series_revision,
        blueprint_node_key,
        occurrence_at,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(series_id), Some(series_revision), Some(blueprint_node_key), Some(occurrence_at)) => {
            Ok(Some(SeriesOccurrenceRef {
                series_id: parse_uuid(series_id, 18)?,
                series_revision,
                occurrence_at,
                blueprint_node_key,
            }))
        }
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            18,
            rusqlite::types::Type::Text,
            Box::new(StorageError::IncompatibleSchema(
                "partial task series occurrence provenance".to_string(),
            )),
        )),
    }
}

pub(super) fn row_to_home_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<HomeTask> {
    Ok(HomeTask {
        task: row_to_task(row)?,
        list_name: row.get(24)?,
        is_home_target: row.get(25)?,
    })
}

pub(super) fn task_due_parts(
    due: Option<&TaskDue>,
) -> (Option<&str>, Option<&str>, Option<i64>, Option<&str>) {
    match due {
        None => (None, None, None, None),
        Some(TaskDue::Date { due_on }) => (Some("date"), Some(due_on.as_str()), None, None),
        Some(TaskDue::DateTime { due_at, time_zone }) => (
            Some("datetime"),
            None,
            Some(due_at.as_millis()),
            Some(time_zone.as_str()),
        ),
    }
}

fn task_due_from_columns(
    kind: Option<String>,
    due_on: Option<String>,
    due_at_ms: Option<i64>,
    due_time_zone: Option<String>,
) -> rusqlite::Result<Option<TaskDue>> {
    let invalid_shape = || {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid task due column shape",
            )),
        )
    };
    match (kind.as_deref(), due_on, due_at_ms, due_time_zone) {
        (None, None, None, None) => Ok(None),
        (Some("date"), Some(value), None, None) => CivilDate::from_str(&value)
            .map(|due_on| Some(TaskDue::Date { due_on }))
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            }),
        (Some("datetime"), None, Some(value), Some(zone)) => {
            let due_at = UtcInstant::from_millis(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            let time_zone = IanaTimeZone::from_str(&zone).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(Some(TaskDue::DateTime { due_at, time_zone }))
        }
        _ => Err(invalid_shape()),
    }
}

pub(super) fn row_to_reminder(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reminder> {
    let id: String = row.get(0)?;
    let task_id: String = row.get(1)?;
    Ok(Reminder {
        id: parse_uuid(id, 0)?,
        task_id: parse_uuid(task_id, 1)?,
        remind_at: row.get(2)?,
        snoozed_until: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub(super) fn row_to_task_undo_entry(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<TaskUndoEntry, StorageError>> {
    let id: String = row.get(0)?;
    let operation_type: String = row.get(1)?;
    let task_id: String = row.get(2)?;
    let list_id: String = row.get(3)?;
    let before_snapshot: String = row.get(4)?;

    Ok((|| {
        Ok(TaskUndoEntry {
            id: Uuid::from_str(&id)?,
            operation_type: undo_operation_from_str(&operation_type)?,
            task_id: Uuid::from_str(&task_id)?,
            list_id: Uuid::from_str(&list_id)?,
            before_snapshot: serde_json::from_str(&before_snapshot)?,
            after_updated_at: row.get(5)?,
            after_deleted_at: row.get(6)?,
            after_completed_at: row.get(7)?,
            created_at: row.get(8)?,
            consumed_at: row.get(9)?,
        })
    })())
}

pub(super) fn parse_uuid(value: String, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

pub(super) fn parse_optional_uuid(
    value: Option<String>,
    column: usize,
) -> rusqlite::Result<Option<Uuid>> {
    value.map(|value| parse_uuid(value, column)).transpose()
}

pub(super) fn status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::WontDo => "wont_do",
    }
}

fn status_from_str(value: &str) -> Result<TaskStatus, StorageError> {
    match value {
        "todo" => Ok(TaskStatus::Todo),
        "in_progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        "wont_do" => Ok(TaskStatus::WontDo),
        other => Err(StorageError::InvalidStatus(other.to_string())),
    }
}

pub(super) fn undo_operation_to_str(operation_type: TaskUndoOperation) -> &'static str {
    match operation_type {
        TaskUndoOperation::Delete => "delete",
        TaskUndoOperation::Complete => "complete",
        TaskUndoOperation::Edit => "edit",
    }
}

fn undo_operation_from_str(value: &str) -> Result<TaskUndoOperation, StorageError> {
    match value {
        "delete" => Ok(TaskUndoOperation::Delete),
        "complete" => Ok(TaskUndoOperation::Complete),
        "edit" => Ok(TaskUndoOperation::Edit),
        other => Err(StorageError::InvalidUndoOperation(other.to_string())),
    }
}
