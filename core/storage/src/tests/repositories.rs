use super::*;

#[test]
fn sqlite_task_repository_insert_get_roundtrips_task() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();

    repository.insert(task.clone()).unwrap();

    assert_eq!(repository.get(task.id).unwrap(), task);
}

#[test]
fn equal_task_ranks_use_record_id_as_stable_tie_break() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let list_id = Uuid::from_u128(10);
    let mut later_id = sample_task();
    later_id.id = Uuid::from_u128(12);
    later_id.list_id = list_id;
    later_id.parent_task_id = None;
    later_id.sort_order = "7fffffffffffffffffffffffffffffff".to_string();
    let mut earlier_id = later_id.clone();
    earlier_id.id = Uuid::from_u128(11);
    repository.insert(later_id.clone()).unwrap();
    repository.insert(earlier_id.clone()).unwrap();

    assert_eq!(
        repository
            .list_active_by_list(list_id)
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![earlier_id.id, later_id.id]
    );
}

#[test]
fn sqlite_list_repository_roundtrips_and_lists_by_sort_order() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);
    let mut first = sample_list("b0");
    let second = sample_list("a0");

    repository.insert(first.clone()).unwrap();
    repository.insert(second.clone()).unwrap();

    assert_eq!(repository.get(first.id).unwrap(), first);

    first.name = "Renamed".to_string();
    first.color = "#FFAA00".to_string();
    first.icon = "star".to_string();
    first.sort_order = "c0".to_string();
    first.archived_at = Some(1_799_000_001_000);
    first.updated_at += 1_000;
    repository.update(first.clone()).unwrap();

    assert_eq!(repository.get(first.id).unwrap(), first);
    assert_eq!(
        repository
            .list_all()
            .unwrap()
            .into_iter()
            .map(|list| list.id)
            .collect::<Vec<_>>(),
        vec![second.id]
    );
    assert_eq!(
        repository
            .list_archived()
            .unwrap()
            .into_iter()
            .map(|list| list.id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );
}

#[test]
fn archived_lists_use_rank_and_record_id_order() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);
    let mut later_rank = sample_list("b0000000000000000000000000000000");
    later_rank.id = Uuid::from_u128(12);
    later_rank.archived_at = Some(10);
    let mut later_id = sample_list("a0000000000000000000000000000000");
    later_id.id = Uuid::from_u128(11);
    later_id.archived_at = Some(30);
    let mut earlier_id = later_id.clone();
    earlier_id.id = Uuid::from_u128(10);
    earlier_id.archived_at = Some(20);
    repository.insert(later_rank.clone()).unwrap();
    repository.insert(later_id.clone()).unwrap();
    repository.insert(earlier_id.clone()).unwrap();

    assert_eq!(
        repository
            .list_archived()
            .unwrap()
            .into_iter()
            .map(|list| list.id)
            .collect::<Vec<_>>(),
        vec![earlier_id.id, later_id.id, later_rank.id]
    );
}

#[test]
fn delete_list_rehomes_tasks_and_preserves_task_undo_entries() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Project".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
    let mut default_list =
        new_list("Inbox".to_string(), "a1".to_string(), 1_700_000_000_000).unwrap();
    default_list.is_default = true;
    let task = new_task(
        list.id,
        None,
        "Task".to_string(),
        "a0".to_string(),
        1_700_000_001_000,
    )
    .unwrap();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
        list_repository.insert(default_list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(task.clone()).unwrap();
        let edited = update_title(task.clone(), "Edited".to_string(), task.updated_at + 1).unwrap();
        task_repository
            .update_with_undo(
                task.clone(),
                edited,
                TaskUndoOperation::Edit,
                task.updated_at + 1,
            )
            .unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut list_repository = SqliteListRepository::new(connection);
    assert_eq!(list_repository.count_tasks(list.id).unwrap(), 1);
    assert_eq!(list_repository.delete_and_rehome_tasks(list.id).unwrap(), 1);
    assert!(matches!(
        list_repository.get(list.id),
        Err(StorageError::NotFound(id)) if id == list.id
    ));

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let task_repository = SqliteTaskRepository::new(connection);
    assert_eq!(
        task_repository.get(task.id).unwrap().list_id,
        default_list.id
    );
    assert!(task_repository.latest_unconsumed_undo().unwrap().is_some());
}

#[test]
fn domain_usecases_persist_task_updates_after_reopen() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Inbox".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
    let task = new_task(
        list.id,
        None,
        "Draft title".to_string(),
        "a0".to_string(),
        1_700_000_001_000,
    )
    .unwrap();
    let renamed = update_title(task.clone(), "Final title".to_string(), 1_700_000_002_000).unwrap();
    let done = transition_task(renamed.clone(), TaskStatus::Done, None, 1_700_000_003_000).unwrap();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(task).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.update(renamed).unwrap();
        task_repository.update(done.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let task_repository = SqliteTaskRepository::new(connection);

    assert_eq!(task_repository.get(done.id).unwrap(), done);
}

#[test]
fn delete_subtree_removes_root_descendants_and_undo_entries() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Inbox".to_string(), "a0".to_string(), 1_700_000_000_000).unwrap();
    let active = new_task(
        list.id,
        None,
        "Keep".to_string(),
        "a0".to_string(),
        1_700_000_001_000,
    )
    .unwrap();
    let parent = new_task(
        list.id,
        None,
        "Delete parent".to_string(),
        "b0".to_string(),
        1_700_000_001_000,
    )
    .unwrap();
    let child = new_task(
        list.id,
        Some(parent.id),
        "Delete child".to_string(),
        "a0".to_string(),
        1_700_000_001_000,
    )
    .unwrap();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut task_repository = SqliteTaskRepository::new(connection);
    task_repository.insert(active.clone()).unwrap();
    task_repository.insert(parent.clone()).unwrap();
    task_repository.insert(child).unwrap();

    let updated = update_title(
        parent.clone(),
        "Before delete".to_string(),
        parent.updated_at + 1,
    )
    .unwrap();
    task_repository
        .update_with_undo(
            parent.clone(),
            updated,
            TaskUndoOperation::Edit,
            parent.updated_at + 1,
        )
        .unwrap();

    assert_eq!(task_repository.count_descendants(parent.id).unwrap(), 1);
    assert_eq!(task_repository.delete_subtree(parent.id).unwrap(), 2);
    assert!(matches!(
        task_repository.get(parent.id),
        Err(StorageError::NotFound(id)) if id == parent.id
    ));
    assert_eq!(
        task_repository.list_active_by_list(list.id).unwrap(),
        vec![active]
    );
    assert!(task_repository.latest_unconsumed_undo().unwrap().is_none());
}

#[test]
fn list_home_filters_due_active_and_closed_tasks_across_active_lists() {
    let file = NamedTempFile::new().unwrap();
    let today_start = 1_800_000_000_000;
    let tomorrow_start = today_start + 86_400_000;
    let overdue = today_start - 86_400_000;
    let tomorrow = tomorrow_start + 1_000;
    let upcoming = tomorrow_start + 86_400_000 + 1_000;

    let inbox = new_list("Inbox".to_string(), "a0".to_string(), today_start).unwrap();
    let work = new_list("Work".to_string(), "a1".to_string(), today_start).unwrap();
    let mut archived = new_list("Archive".to_string(), "a2".to_string(), today_start).unwrap();
    archived.archived_at = Some(today_start + 1);

    let mut due_today = new_task(
        inbox.id,
        None,
        "Due today".to_string(),
        "a0".to_string(),
        today_start,
    )
    .unwrap();
    due_today.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
    due_today.scheduled_at = Some(today_start + 500);
    let no_due_child = new_task(
        inbox.id,
        Some(due_today.id),
        "No due child".to_string(),
        "a0".to_string(),
        today_start,
    )
    .unwrap();
    let no_due_parent = new_task(
        inbox.id,
        None,
        "No due parent".to_string(),
        "a4".to_string(),
        today_start,
    )
    .unwrap();
    let mut due_child = new_task(
        inbox.id,
        Some(no_due_parent.id),
        "Due child".to_string(),
        "a0".to_string(),
        today_start,
    )
    .unwrap();
    due_child.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
    let mut overdue_task = new_task(
        work.id,
        None,
        "Overdue".to_string(),
        "a0".to_string(),
        today_start,
    )
    .unwrap();
    overdue_task.due = Some(TaskDue::date_time(overdue, "UTC").unwrap());
    let mut tomorrow_task = new_task(
        inbox.id,
        None,
        "Tomorrow".to_string(),
        "a1".to_string(),
        today_start,
    )
    .unwrap();
    tomorrow_task.due = Some(TaskDue::date_time(tomorrow, "UTC").unwrap());
    let mut upcoming_task = new_task(
        inbox.id,
        None,
        "Upcoming".to_string(),
        "a2".to_string(),
        today_start,
    )
    .unwrap();
    upcoming_task.due = Some(TaskDue::date_time(upcoming, "UTC").unwrap());
    let no_due = new_task(
        inbox.id,
        None,
        "No due".to_string(),
        "a3".to_string(),
        today_start,
    )
    .unwrap();
    let mut scheduled_today = new_task(
        work.id,
        None,
        "Scheduled today".to_string(),
        "a4".to_string(),
        today_start,
    )
    .unwrap();
    scheduled_today.scheduled_at = Some(today_start + 3_600_000);
    let mut scheduled_tomorrow = new_task(
        work.id,
        None,
        "Scheduled tomorrow".to_string(),
        "a5".to_string(),
        today_start,
    )
    .unwrap();
    scheduled_tomorrow.scheduled_at = Some(tomorrow_start + 3_600_000);
    let mut archived_task = new_task(
        archived.id,
        None,
        "Archived".to_string(),
        "a0".to_string(),
        today_start,
    )
    .unwrap();
    archived_task.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
    let mut deleted_task = new_task(
        work.id,
        None,
        "Deleted".to_string(),
        "a6".to_string(),
        today_start,
    )
    .unwrap();
    deleted_task.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
    deleted_task.deleted_at = Some(today_start + 1);
    let mut closed_today = new_task(
        work.id,
        None,
        "Closed today".to_string(),
        "a1".to_string(),
        today_start,
    )
    .unwrap();
    closed_today =
        transition_task(closed_today, TaskStatus::Done, None, today_start + 1_000).unwrap();
    let mut closed_yesterday = new_task(
        work.id,
        None,
        "Closed yesterday".to_string(),
        "a2".to_string(),
        today_start,
    )
    .unwrap();
    closed_yesterday.due = Some(TaskDue::date_time(today_start, "UTC").unwrap());
    closed_yesterday = transition_task(
        closed_yesterday,
        TaskStatus::Done,
        None,
        today_start - 1_000,
    )
    .unwrap();
    let mut wont_do_today = new_task(
        work.id,
        None,
        "Wont do today".to_string(),
        "a3".to_string(),
        today_start,
    )
    .unwrap();
    wont_do_today = transition_task(
        wont_do_today,
        TaskStatus::WontDo,
        Some("not needed".to_string()),
        today_start + 2_000,
    )
    .unwrap();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(inbox.clone()).unwrap();
        list_repository.insert(work.clone()).unwrap();
        list_repository.insert(archived).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut task_repository = SqliteTaskRepository::new(connection);
    for task in [
        due_today,
        overdue_task,
        tomorrow_task,
        upcoming_task,
        no_due,
        scheduled_today,
        scheduled_tomorrow,
        archived_task,
        deleted_task,
        closed_today,
        closed_yesterday,
        wont_do_today,
        no_due_child,
        no_due_parent,
        due_child,
    ] {
        task_repository.insert(task).unwrap();
    }

    let home_tasks = task_repository
        .list_home(today_start, tomorrow_start)
        .unwrap();
    let titles = home_tasks
        .iter()
        .map(|entry| entry.task.content.title.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        titles,
        vec![
            "Overdue",
            "Due today",
            "Due child",
            "Tomorrow",
            "Upcoming",
            "No due child",
            "Closed today",
            "Wont do today",
            "No due parent",
            "Scheduled today"
        ]
    );
    assert_eq!(
        titles.iter().filter(|title| **title == "Due today").count(),
        1,
        "a dual due/scheduled target remains one Home row"
    );
    assert!(
        !titles.contains(&"Deleted"),
        "soft-deleted tasks remain outside Home"
    );
    for title in ["Closed today", "Wont do today"] {
        assert!(
            home_tasks
                .iter()
                .find(|entry| entry.task.content.title == title)
                .unwrap()
                .is_home_target,
            "today's completed achievement is independent of planning fields"
        );
    }
    assert!(
        home_tasks
            .iter()
            .find(|entry| entry.task.content.title == "Due today")
            .unwrap()
            .is_home_target
    );
    assert!(
        !home_tasks
            .iter()
            .find(|entry| entry.task.content.title == "No due child")
            .unwrap()
            .is_home_target
    );
    assert!(
        !home_tasks
            .iter()
            .find(|entry| entry.task.content.title == "No due parent")
            .unwrap()
            .is_home_target
    );
    assert!(
        home_tasks
            .iter()
            .find(|entry| entry.task.content.title == "Due child")
            .unwrap()
            .is_home_target
    );
    assert_eq!(
        home_tasks
            .iter()
            .find(|entry| entry.task.content.title == "Overdue")
            .unwrap()
            .list_name,
        "Work"
    );
    assert!(!titles.contains(&"No due"));
    assert!(!titles.contains(&"Scheduled tomorrow"));
    assert!(!titles.contains(&"Archived"));
    assert!(!titles.contains(&"Closed yesterday"));
}

#[test]
fn calendar_range_rejects_empty_or_reversed_dimensions() {
    let day = CivilDate::parse("2026-03-08").unwrap();
    let next = CivilDate::parse("2026-03-09").unwrap();
    let start = UtcInstant::from_millis(1_773_000_000_000).unwrap();
    let end = UtcInstant::from_millis(start.as_millis() + 1).unwrap();

    assert_eq!(
        CalendarRange::new(day.clone(), day, start, end),
        Err(CalendarRangeError::InvalidCivilDateRange)
    );
    assert_eq!(
        CalendarRange::new(next.clone(), next, end, start),
        Err(CalendarRangeError::InvalidCivilDateRange)
    );
    assert_eq!(
        CalendarRange::new(
            CivilDate::parse("2026-03-08").unwrap(),
            CivilDate::parse("2026-03-09").unwrap(),
            start,
            start,
        ),
        Err(CalendarRangeError::InvalidInstantRange)
    );

    for hours in [23, 25] {
        let dst_end = UtcInstant::from_millis(start.as_millis() + hours * 3_600_000).unwrap();
        let range = CalendarRange::new(
            CivilDate::parse("2026-03-08").unwrap(),
            CivilDate::parse("2026-03-09").unwrap(),
            start,
            dst_end,
        )
        .unwrap();
        assert_eq!(
            range.end_at().as_millis() - range.start_at().as_millis(),
            hours * 3_600_000
        );
    }
}

#[test]
fn calendar_occurrences_preserve_semantics_boundaries_and_archived_context() {
    let file = NamedTempFile::new().unwrap();
    // America/New_York 2026 spring-forward day is 23 hours. The storage
    // contract consumes caller-provided boundaries and never adds 24h.
    let start_ms = 1_773_035_600_000; // 2026-03-08T05:00:00Z
    let end_ms = start_ms + 23 * 3_600_000;
    let range = CalendarRange::new(
        CivilDate::parse("2026-03-08").unwrap(),
        CivilDate::parse("2026-03-09").unwrap(),
        UtcInstant::from_millis(start_ms).unwrap(),
        UtcInstant::from_millis(end_ms).unwrap(),
    )
    .unwrap();
    let active = new_list("Active".into(), "a0".into(), start_ms).unwrap();
    let mut archived = new_list("Archive".into(), "a1".into(), start_ms).unwrap();
    archived.archived_at = Some(start_ms);
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(active.clone()).unwrap();
        lists.insert(archived.clone()).unwrap();
    }

    let mut dual = new_task(active.id, None, "Dual".into(), "a0".into(), start_ms).unwrap();
    dual.due = Some(TaskDue::date("2026-03-08").unwrap());
    dual.scheduled_at = Some(start_ms);

    let mut datetime = new_task(active.id, None, "Datetime".into(), "a1".into(), start_ms).unwrap();
    datetime.due = Some(TaskDue::date_time(end_ms - 1, "America/New_York").unwrap());
    datetime = transition_task(datetime, TaskStatus::InProgress, None, start_ms + 1).unwrap();

    let mut completed =
        new_task(archived.id, None, "Completed".into(), "a2".into(), start_ms).unwrap();
    completed = transition_task(completed, TaskStatus::Done, None, end_ms - 1).unwrap();

    let mut wont_do = new_task(active.id, None, "Wont do".into(), "a3".into(), start_ms).unwrap();
    wont_do.due = Some(TaskDue::date_time(start_ms, "UTC").unwrap());
    wont_do = transition_task(
        wont_do,
        TaskStatus::WontDo,
        Some("obsolete".into()),
        start_ms + 2,
    )
    .unwrap();

    let mut excluded_end =
        new_task(active.id, None, "At end".into(), "a4".into(), start_ms).unwrap();
    excluded_end.due = Some(TaskDue::date("2026-03-09").unwrap());
    excluded_end.scheduled_at = Some(end_ms);

    let mut deleted = new_task(active.id, None, "Deleted".into(), "a5".into(), start_ms).unwrap();
    deleted.due = Some(TaskDue::date("2026-03-08").unwrap());
    deleted.deleted_at = Some(start_ms + 3);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut tasks = SqliteTaskRepository::new(connection);
    for task in [dual, datetime, completed, wont_do, excluded_end, deleted] {
        tasks.insert(task).unwrap();
    }

    let occurrences = tasks.list_calendar_occurrences(&range).unwrap();
    assert_eq!(occurrences.len(), 5);
    assert_eq!(
        occurrences
            .iter()
            .filter(|occurrence| occurrence.task.content.title == "Dual")
            .count(),
        2
    );
    assert!(occurrences.iter().any(|occurrence| matches!(
        &occurrence.kind,
        CalendarOccurrenceKind::DateDue { due_on } if due_on.as_str() == "2026-03-08"
    )));
    assert!(occurrences.iter().any(|occurrence| matches!(
        occurrence.kind,
        CalendarOccurrenceKind::DateTimeDue { due_at, .. } if due_at.as_millis() == end_ms - 1
    )));
    assert!(occurrences.iter().any(|occurrence| matches!(
        occurrence.kind,
        CalendarOccurrenceKind::Scheduled { scheduled_at } if scheduled_at.as_millis() == start_ms
    )));
    assert!(occurrences.iter().any(|occurrence| {
        occurrence.list_archived
            && occurrence.list_name == "Archive"
            && matches!(occurrence.kind, CalendarOccurrenceKind::Completed { .. })
    }));
    assert!(occurrences.iter().any(|occurrence| {
        occurrence.task.status == TaskStatus::WontDo
            && matches!(occurrence.kind, CalendarOccurrenceKind::Completed { .. })
    }));
    assert!(!occurrences.iter().any(|occurrence| {
        occurrence.task.status == TaskStatus::WontDo
            && matches!(occurrence.kind, CalendarOccurrenceKind::DateTimeDue { .. })
    }));
    assert!(!occurrences
        .iter()
        .any(|occurrence| occurrence.task.content.title == "At end"
            || occurrence.task.content.title == "Deleted"));
}

#[test]
fn update_with_undo_records_edit_and_restores_previous_snapshot() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    repository.insert(task.clone()).unwrap();

    let updated = update_title(task.clone(), "Undo me".to_string(), task.updated_at + 1).unwrap();
    let undo = repository
        .update_with_undo(
            task.clone(),
            updated.clone(),
            TaskUndoOperation::Edit,
            updated.updated_at,
        )
        .unwrap();

    assert_eq!(repository.latest_unconsumed_undo().unwrap().unwrap(), undo);
    assert_eq!(repository.get(task.id).unwrap(), updated);

    let restored = repository
        .undo_task_operation(undo.id, updated.updated_at + 1)
        .unwrap();

    assert_eq!(restored, task);
    assert_eq!(repository.get(task.id).unwrap(), task);
    assert!(repository.latest_unconsumed_undo().unwrap().is_none());
    assert!(matches!(
        repository.undo_task_operation(undo.id, updated.updated_at + 2),
        Err(StorageError::UndoConsumed(id)) if id == undo.id
    ));
}

#[test]
fn delete_undo_entries_are_not_returned_as_latest_undo() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    repository.insert(task.clone()).unwrap();

    let mut deleted = task.clone();
    deleted.deleted_at = Some(task.updated_at + 1);
    deleted.updated_at = task.updated_at + 1;
    repository
        .update_with_undo(
            task.clone(),
            deleted.clone(),
            TaskUndoOperation::Delete,
            deleted.updated_at,
        )
        .unwrap();

    assert!(repository.latest_unconsumed_undo().unwrap().is_none());
}

#[test]
fn complete_undo_entry_restores_task_state() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    repository.insert(task.clone()).unwrap();

    let done = transition_task(task.clone(), TaskStatus::Done, None, task.updated_at + 1).unwrap();
    let complete_undo = repository
        .update_with_undo(
            task.clone(),
            done.clone(),
            TaskUndoOperation::Complete,
            done.updated_at,
        )
        .unwrap();

    assert_eq!(
        repository
            .undo_task_operation(complete_undo.id, done.updated_at + 1)
            .unwrap()
            .status,
        TaskStatus::Todo
    );
}

#[test]
fn undo_rejects_edit_conflict_after_later_update() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    repository.insert(task.clone()).unwrap();

    let edited = update_title(task.clone(), "First edit".to_string(), task.updated_at + 1).unwrap();
    let undo = repository
        .update_with_undo(
            task.clone(),
            edited.clone(),
            TaskUndoOperation::Edit,
            edited.updated_at,
        )
        .unwrap();
    let second_edit = update_title(
        edited.clone(),
        "Second edit".to_string(),
        edited.updated_at + 1,
    )
    .unwrap();
    repository.update(second_edit).unwrap();

    assert!(matches!(
        repository.undo_task_operation(undo.id, edited.updated_at + 2),
        Err(StorageError::UndoConflict(id)) if id == task.id
    ));
}

#[test]
fn complete_undo_rejects_physically_deleted_current_task() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    repository.insert(task.clone()).unwrap();

    let done = transition_task(task.clone(), TaskStatus::Done, None, task.updated_at + 1).unwrap();
    let undo = repository
        .update_with_undo(
            task.clone(),
            done.clone(),
            TaskUndoOperation::Complete,
            done.updated_at,
        )
        .unwrap();
    repository.delete_subtree(done.id).unwrap();

    assert!(matches!(
        repository.undo_task_operation(undo.id, task.updated_at + 3),
        Err(StorageError::NotFound(id)) if id == undo.id
    ));
}

#[test]
fn update_returns_not_found_for_missing_task_and_list() {
    let file = NamedTempFile::new().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut task_repository = SqliteTaskRepository::new(connection);
    let task = sample_task();
    assert!(matches!(
        task_repository.update(task.clone()),
        Err(StorageError::NotFound(id)) if id == task.id
    ));

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut list_repository = SqliteListRepository::new(connection);
    let list = sample_list("a0");
    assert!(matches!(
        list_repository.update(list.clone()),
        Err(StorageError::NotFound(id)) if id == list.id
    ));
}

#[test]
fn sqlite_write_tx_commits_domain_and_sync_state_together() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        repository.insert(task.clone()).unwrap();
    }

    let edited = update_title(
        task.clone(),
        "Transactional edit".to_string(),
        task.updated_at + 1,
    )
    .unwrap();
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
    assert_eq!(write_tx.get_task(task.id).unwrap(), task);
    write_tx
        .update_with_undo(
            task.clone(),
            edited.clone(),
            TaskUndoOperation::Edit,
            edited.updated_at,
        )
        .unwrap();
    write_tx
        .set_setting("sync_local_hlc", "encoded-hlc", edited.updated_at)
        .unwrap();
    let op_id = Uuid::now_v7();
    write_tx
        .put_outbox_head(new_live_outbox(
            task.id,
            "tasks",
            op_id,
            None,
            "encoded-hlc",
            "encoded-hlc",
            vec![1, 2, 3],
        ))
        .unwrap();
    write_tx
        .put_record_state(live_record_state(
            task.id,
            "tasks",
            Some("encoded-hlc"),
            "encoded-hlc",
            r#"{"title":"Transactional edit"}"#,
            edited.updated_at,
        ))
        .unwrap();
    assert_eq!(write_tx.list_outbox_heads(10).unwrap().len(), 1);
    write_tx.commit().unwrap();
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteTaskRepository::new(connection);
    assert_eq!(repository.get(task.id).unwrap(), edited);
    drop(repository);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let settings = SqliteSettingsRepository::new(connection);
    assert_eq!(
        settings.get_setting("sync_local_hlc").unwrap().as_deref(),
        Some("encoded-hlc")
    );
    drop(settings);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let sync = SqliteSyncStateRepository::new(connection);
    assert_eq!(sync.list_outbox_heads(10).unwrap().len(), 1);
    assert_eq!(
        sync.get_record_state("tasks", task.id).unwrap(),
        Some(live_record_state(
            task.id,
            "tasks",
            Some("encoded-hlc"),
            "encoded-hlc",
            r#"{"title":"Transactional edit"}"#,
            edited.updated_at,
        ))
    );
}

#[test]
fn sqlite_write_tx_drop_rolls_back_domain_and_sync_state_together() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        repository.insert(task.clone()).unwrap();
    }

    let edited = update_title(
        task.clone(),
        "Rolled back edit".to_string(),
        task.updated_at + 1,
    )
    .unwrap();
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    {
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        write_tx
            .update_with_undo(
                task.clone(),
                edited.clone(),
                TaskUndoOperation::Edit,
                edited.updated_at,
            )
            .unwrap();
        write_tx
            .set_setting("sync_local_hlc", "rolled-back-hlc", edited.updated_at)
            .unwrap();
        write_tx
            .put_outbox_head(new_live_outbox(
                task.id,
                "tasks",
                Uuid::now_v7(),
                None,
                "rolled-back-hlc",
                "rolled-back-hlc",
                vec![4, 5, 6],
            ))
            .unwrap();
        write_tx
            .put_record_state(live_record_state(
                task.id,
                "tasks",
                Some("rolled-back-hlc"),
                "rolled-back-hlc",
                r#"{"title":"Rolled back edit"}"#,
                edited.updated_at,
            ))
            .unwrap();
    }
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteTaskRepository::new(connection);
    assert_eq!(repository.get(task.id).unwrap(), task);
    assert!(repository.latest_unconsumed_undo().unwrap().is_none());
    drop(repository);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let settings = SqliteSettingsRepository::new(connection);
    assert_eq!(settings.get_setting("sync_local_hlc").unwrap(), None);
    drop(settings);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let sync = SqliteSyncStateRepository::new(connection);
    assert!(sync.list_outbox_heads(10).unwrap().is_empty());
    assert_eq!(sync.get_record_state("tasks", task.id).unwrap(), None);
}

#[test]
fn owned_sqlite_write_tx_commits_domain_hlc_record_state_and_outbox() {
    let file = NamedTempFile::new().unwrap();
    let mut list = sample_list("a0");
    list.is_default = true;
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let op_id = Uuid::now_v7();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .set_setting("sync_local_hlc", "owned-hlc", task.updated_at)
        .unwrap();
    transaction.upsert_list_for_sync(list.clone()).unwrap();
    transaction.upsert_task_for_sync(task.clone()).unwrap();
    transaction
        .put_record_state(live_record_state(
            task.id,
            "tasks",
            Some("owned-hlc"),
            "owned-hlc",
            r#"{"title":"Owned commit"}"#,
            task.updated_at,
        ))
        .unwrap();
    transaction
        .put_outbox_head(new_live_outbox(
            task.id,
            "tasks",
            op_id,
            Some("base-hlc"),
            "owned-hlc",
            "owned-hlc",
            vec![1, 2, 3],
        ))
        .unwrap();
    transaction
        .set_cursor("default", 7, task.updated_at)
        .unwrap();
    assert_eq!(transaction.default_list_id().unwrap(), Some(list.id));
    assert_eq!(transaction.get_list(list.id).unwrap(), Some(list.clone()));
    assert_eq!(transaction.get_task(task.id).unwrap(), Some(task.clone()));
    assert!(transaction.has_outbox_head("tasks", task.id).unwrap());
    assert_eq!(transaction.list_outbox_heads(10).unwrap().len(), 1);
    assert_eq!(transaction.get_cursor("default").unwrap().unwrap().seq, 7);
    let connection = transaction.commit().unwrap();

    assert_eq!(
        get_setting_on(&connection, "sync_local_hlc")
            .unwrap()
            .as_deref(),
        Some("owned-hlc")
    );
    assert_eq!(get_list_on(&connection, list.id).unwrap(), list);
    assert_eq!(get_task_on(&connection, task.id).unwrap(), task);
    assert!(get_record_state_on(&connection, "tasks", task.id)
        .unwrap()
        .is_some());
    assert_eq!(list_outbox_heads_on(&connection, 10).unwrap().len(), 1);
    assert_eq!(
        get_cursor_on(&connection, "default").unwrap().unwrap().seq,
        7
    );
}

#[test]
fn owned_sqlite_write_tx_drop_rolls_back_domain_hlc_record_state_and_outbox() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .set_setting("sync_local_hlc", "rolled-back-owned-hlc", task.updated_at)
            .unwrap();
        transaction.upsert_list_for_sync(list.clone()).unwrap();
        transaction.upsert_task_for_sync(task.clone()).unwrap();
        transaction
            .put_record_state(live_record_state(
                task.id,
                "tasks",
                None,
                "rolled-back-owned-hlc",
                r#"{"title":"Owned rollback"}"#,
                task.updated_at,
            ))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                task.id,
                "tasks",
                Uuid::now_v7(),
                None,
                "rolled-back-owned-hlc",
                "rolled-back-owned-hlc",
                vec![4, 5, 6],
            ))
            .unwrap();
        transaction
            .set_cursor("default", 9, task.updated_at)
            .unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(get_setting_on(&connection, "sync_local_hlc").unwrap(), None);
    assert!(matches!(
        get_list_on(&connection, list.id),
        Err(StorageError::NotFound(id)) if id == list.id
    ));
    assert!(matches!(
        get_task_on(&connection, task.id),
        Err(StorageError::NotFound(id)) if id == task.id
    ));
    assert_eq!(
        get_record_state_on(&connection, "tasks", task.id).unwrap(),
        None
    );
    assert!(list_outbox_heads_on(&connection, 10).unwrap().is_empty());
    assert_eq!(get_cursor_on(&connection, "default").unwrap(), None);
}

#[test]
fn sqlite_write_tx_commits_task_and_list_crud_without_nested_transactions() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        repository.insert(list.clone()).unwrap();
    }

    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let mut prepared = task.clone();
    prepared.content.note = "Updated before status transition".to_string();
    prepared.updated_at += 1;
    let done = transition_task(
        prepared.clone(),
        TaskStatus::Done,
        None,
        prepared.updated_at + 1,
    )
    .unwrap();
    let mut renamed_list = list.clone();
    renamed_list.name = "Renamed transactionally".to_string();
    renamed_list.updated_at += 1;

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
    assert_eq!(write_tx.get_list(list.id).unwrap(), list);
    write_tx.update_list(renamed_list.clone()).unwrap();
    write_tx.insert_task(task.clone()).unwrap();
    write_tx.update_task(prepared.clone()).unwrap();
    assert_eq!(
        write_tx.list_active_tasks_by_list(list.id).unwrap(),
        vec![prepared.clone()]
    );
    let undo = write_tx
        .update_task_with_undo(
            prepared,
            done.clone(),
            TaskUndoOperation::Complete,
            done.updated_at,
        )
        .unwrap();
    assert_eq!(write_tx.get_task(done.id).unwrap(), done);
    write_tx.commit().unwrap();
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let list_repository = SqliteListRepository::new(connection);
    assert_eq!(list_repository.get(list.id).unwrap(), renamed_list);
    drop(list_repository);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let task_repository = SqliteTaskRepository::new(connection);
    assert_eq!(task_repository.get(done.id).unwrap(), done);
    assert_eq!(
        task_repository.latest_unconsumed_undo().unwrap(),
        Some(undo)
    );
}

#[test]
fn sqlite_write_tx_snapshots_and_commits_task_subtree_delete() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut parent = sample_task();
    parent.list_id = list.id;
    parent.parent_task_id = None;
    parent.sort_order = "a0".to_string();
    let mut child = sample_task();
    child.list_id = list.id;
    child.parent_task_id = Some(parent.id);
    child.sort_order = "a1".to_string();
    let mut sibling = sample_task();
    sibling.list_id = list.id;
    sibling.parent_task_id = None;
    sibling.sort_order = "a2".to_string();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.insert(parent.clone()).unwrap();
        tasks.insert(child.clone()).unwrap();
        tasks.insert(sibling.clone()).unwrap();
    }

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
    assert_eq!(
        write_tx.list_task_subtree(parent.id).unwrap(),
        vec![parent.clone(), child.clone()]
    );
    assert_eq!(
        write_tx.list_tasks_by_list(list.id).unwrap(),
        vec![parent.clone(), child.clone(), sibling.clone()]
    );
    assert_eq!(write_tx.delete_task_subtree(parent.id).unwrap(), 2);
    assert!(matches!(
        write_tx.get_task(parent.id),
        Err(StorageError::NotFound(id)) if id == parent.id
    ));
    assert!(matches!(
        write_tx.get_task(child.id),
        Err(StorageError::NotFound(id)) if id == child.id
    ));
    assert_eq!(write_tx.get_task(sibling.id).unwrap(), sibling.clone());
    write_tx.commit().unwrap();
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let tasks = SqliteTaskRepository::new(connection);
    assert!(matches!(
        tasks.get(parent.id),
        Err(StorageError::NotFound(id)) if id == parent.id
    ));
    assert!(matches!(
        tasks.get(child.id),
        Err(StorageError::NotFound(id)) if id == child.id
    ));
    assert_eq!(tasks.get(sibling.id).unwrap(), sibling);
}

#[test]
fn sqlite_write_tx_drop_rolls_back_list_and_task_physical_delete() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut default_list = sample_list("a1");
    default_list.is_default = true;
    let mut active = sample_task();
    active.list_id = list.id;
    active.parent_task_id = None;
    active.sort_order = "a0".to_string();
    let mut logically_deleted = sample_task();
    logically_deleted.list_id = list.id;
    logically_deleted.parent_task_id = None;
    logically_deleted.sort_order = "a1".to_string();
    logically_deleted.deleted_at = Some(logically_deleted.updated_at + 1);

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(list.clone()).unwrap();
        lists.insert(default_list).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.insert(active.clone()).unwrap();
        tasks.insert(logically_deleted.clone()).unwrap();
    }

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    {
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert_eq!(
            write_tx.list_tasks_by_list(list.id).unwrap(),
            vec![active.clone(), logically_deleted.clone()]
        );
        assert_eq!(write_tx.delete_list_and_rehome_tasks(list.id).unwrap(), 2);
        assert!(matches!(
            write_tx.get_list(list.id),
            Err(StorageError::NotFound(id)) if id == list.id
        ));
    }
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let lists = SqliteListRepository::new(connection);
    assert_eq!(lists.get(list.id).unwrap(), list);
    drop(lists);
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let tasks = SqliteTaskRepository::new(connection);
    assert_eq!(tasks.get(active.id).unwrap(), active);
    assert_eq!(tasks.get(logically_deleted.id).unwrap(), logically_deleted);
}

#[test]
fn sqlite_write_tx_commits_list_delete_and_protects_default_list() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let mut default_list = sample_list("a1");
    default_list.is_default = true;

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(list.clone()).unwrap();
        lists.insert(default_list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut tasks = SqliteTaskRepository::new(connection);
        tasks.insert(task.clone()).unwrap();
    }

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
    assert!(matches!(
        write_tx.delete_list_and_rehome_tasks(default_list.id),
        Err(StorageError::DefaultListProtected { list_id, .. }) if list_id == default_list.id
    ));
    assert_eq!(write_tx.delete_list_and_rehome_tasks(list.id).unwrap(), 1);
    write_tx.commit().unwrap();
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let lists = SqliteListRepository::new(connection);
    assert!(matches!(
        lists.get(list.id),
        Err(StorageError::NotFound(id)) if id == list.id
    ));
    assert_eq!(lists.get(default_list.id).unwrap(), default_list);
    drop(lists);
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let tasks = SqliteTaskRepository::new(connection);
    assert_eq!(tasks.get(task.id).unwrap().list_id, default_list.id);
}

#[test]
fn sqlite_write_tx_drop_rolls_back_undo_restore_and_list_update() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let edited = update_title(
        task.clone(),
        "Awaiting undo".to_string(),
        task.updated_at + 1,
    )
    .unwrap();
    let undo = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
        drop(list_repository);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(task.clone()).unwrap();
        task_repository
            .update_with_undo(
                task.clone(),
                edited.clone(),
                TaskUndoOperation::Edit,
                edited.updated_at,
            )
            .unwrap()
    };
    let mut archived_list = list.clone();
    archived_list.archived_at = Some(edited.updated_at + 1);
    archived_list.updated_at = edited.updated_at + 1;

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    {
        let mut write_tx = SqliteWriteTx::begin(&mut connection).unwrap();
        assert_eq!(
            write_tx
                .undo_task_operation(undo.id, edited.updated_at + 1)
                .unwrap(),
            task
        );
        write_tx.update_list(archived_list).unwrap();
        assert_eq!(write_tx.get_task(task.id).unwrap(), task);
    }
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let list_repository = SqliteListRepository::new(connection);
    assert_eq!(list_repository.get(list.id).unwrap(), list);
    drop(list_repository);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut task_repository = SqliteTaskRepository::new(connection);
    assert_eq!(task_repository.get(task.id).unwrap(), edited);
    assert_eq!(
        task_repository
            .undo_task_operation(undo.id, edited.updated_at + 2)
            .unwrap(),
        task
    );
}
