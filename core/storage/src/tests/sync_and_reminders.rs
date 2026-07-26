use super::*;

#[test]
fn sqlite_sync_state_repository_coalesces_record_head_and_old_ack_is_safe() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSyncStateRepository::new(connection);
    let record_id = Uuid::now_v7();
    let first_op_id = Uuid::now_v7();
    let second_op_id = Uuid::now_v7();

    repository
        .put_outbox_head(new_live_outbox(
            record_id,
            "tasks",
            first_op_id,
            Some("base-0"),
            "revision-1",
            "mutation-1",
            vec![1, 2, 3],
        ))
        .unwrap();
    let second = repository
        .put_outbox_head(NewSyncOutboxEntry {
            op_id: second_op_id,
            record_id,
            collection: "tasks".to_string(),
            base_revision_hlc: Some("base-0".to_string()),
            revision_hlc: "revision-2".to_string(),
            state: SyncOutboxState::Tombstone {
                delete_hlc: "delete-2".to_string(),
            },
            created_at: 1_799_000_000_001,
        })
        .unwrap();

    assert_eq!(repository.list_outbox_heads(10).unwrap(), vec![second]);
    assert!(!repository.ack_outbox_op(first_op_id).unwrap());
    assert_eq!(repository.list_outbox_heads(10).unwrap().len(), 1);
    assert!(repository.ack_outbox_op(second_op_id).unwrap());
    assert!(repository.list_outbox_heads(10).unwrap().is_empty());
    assert!(!repository.ack_outbox_op(second_op_id).unwrap());
}

#[test]
fn durable_quarantine_is_idempotent_and_blocks_only_its_record_outbox() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSyncStateRepository::new(connection);
    let blocked_id = Uuid::now_v7();
    let unrelated_id = Uuid::now_v7();
    for record_id in [blocked_id, unrelated_id] {
        repository
            .put_outbox_head(new_live_outbox(
                record_id,
                "tasks",
                Uuid::now_v7(),
                None,
                "revision-local",
                "mutation-local",
                vec![9],
            ))
            .unwrap();
    }
    let quarantined = SyncQuarantineEntry {
        record_id: blocked_id,
        collection: "tasks".to_string(),
        seq: 7,
        revision_hlc: "revision-remote".to_string(),
        state: SyncOutboxState::Live {
            mutation_hlc: "mutation-remote".to_string(),
            blob: vec![1, 2, 3],
        },
        reason: "no_matching_dek".to_string(),
        required_list_id: None,
        first_failed_at: 10,
        last_failed_at: 10,
        attempt_count: 1,
    };
    repository.put_quarantine(quarantined.clone()).unwrap();
    repository
        .put_quarantine(SyncQuarantineEntry {
            state: SyncOutboxState::Tombstone {
                delete_hlc: "must-not-replace".to_string(),
            },
            reason: "authentication_failed".to_string(),
            last_failed_at: 20,
            ..quarantined.clone()
        })
        .unwrap();

    let rows = repository.list_quarantine(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempt_count, 2);
    assert_eq!(rows[0].first_failed_at, 10);
    assert_eq!(rows[0].last_failed_at, 20);
    assert_eq!(rows[0].reason, "authentication_failed");
    assert_eq!(
        rows[0].state,
        SyncOutboxState::Live {
            mutation_hlc: "mutation-remote".to_string(),
            blob: vec![1, 2, 3]
        }
    );

    repository
        .put_quarantine(SyncQuarantineEntry {
            seq: 6,
            revision_hlc: "older-revision".to_string(),
            last_failed_at: 30,
            ..quarantined.clone()
        })
        .unwrap();
    let rows = repository.list_quarantine(10).unwrap();
    assert_eq!(rows[0].seq, 7);
    assert_eq!(rows[0].revision_hlc, "revision-remote");
    assert_eq!(rows[0].attempt_count, 2);
    assert!(matches!(
        repository.put_quarantine(SyncQuarantineEntry {
            revision_hlc: "different-at-same-seq".to_string(),
            ..quarantined.clone()
        }),
        Err(StorageError::IncompatibleSchema(_))
    ));
    assert!(matches!(
        repository.put_quarantine(SyncQuarantineEntry {
            collection: "lists".to_string(),
            ..quarantined.clone()
        }),
        Err(StorageError::SyncCollectionMismatch { .. })
    ));
    assert!(matches!(
        repository.put_record_state(SyncRecordState {
            record_id: blocked_id,
            collection: "lists".to_string(),
            current_revision_hlc: None,
            state: SyncRecordSemanticState::Tombstone {
                delete_hlc: "delete".to_string(),
            },
            updated_at: 30,
        }),
        Err(StorageError::SyncCollectionMismatch { .. })
    ));

    repository
        .put_quarantine(SyncQuarantineEntry {
            seq: 8,
            revision_hlc: "newer-revision".to_string(),
            state: SyncOutboxState::Tombstone {
                delete_hlc: "newer-delete".to_string(),
            },
            reason: "corrupt_envelope".to_string(),
            first_failed_at: 40,
            last_failed_at: 40,
            ..quarantined
        })
        .unwrap();
    let rows = repository.list_quarantine(10).unwrap();
    assert_eq!(rows[0].seq, 8);
    assert_eq!(rows[0].revision_hlc, "newer-revision");
    assert_eq!(rows[0].attempt_count, 1);
    assert_eq!(rows[0].first_failed_at, 40);
    assert!(matches!(rows[0].state, SyncOutboxState::Tombstone { .. }));
    let pushable = repository.list_outbox_heads(10).unwrap();
    assert_eq!(pushable.len(), 1);
    assert_eq!(pushable[0].record_id, unrelated_id);
    assert_eq!(repository.list_all_outbox_heads(10).unwrap().len(), 2);
    assert!(repository.has_outbox_head("tasks", blocked_id).unwrap());
    assert!(repository.delete_quarantine(blocked_id).unwrap());
    assert_eq!(repository.list_outbox_heads(10).unwrap().len(), 2);
}

#[test]
fn replayable_quarantine_query_skips_corruption_without_head_of_line_blocking() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSyncStateRepository::new(connection);
    for seq in 1..=100 {
        repository
            .put_quarantine(SyncQuarantineEntry {
                record_id: Uuid::now_v7(),
                collection: "lists".to_string(),
                seq,
                revision_hlc: format!("corrupt-{seq}"),
                state: SyncOutboxState::Live {
                    mutation_hlc: format!("mutation-{seq}"),
                    blob: vec![1],
                },
                reason: "corrupt_envelope".to_string(),
                required_list_id: None,
                first_failed_at: 10,
                last_failed_at: 10,
                attempt_count: 1,
            })
            .unwrap();
    }
    let waiting_id = Uuid::now_v7();
    repository
        .put_quarantine(SyncQuarantineEntry {
            record_id: waiting_id,
            collection: "lists".to_string(),
            seq: 101,
            revision_hlc: "waiting".to_string(),
            state: SyncOutboxState::Live {
                mutation_hlc: "waiting-mutation".to_string(),
                blob: vec![1],
            },
            reason: "missing_dek".to_string(),
            required_list_id: Some(waiting_id),
            first_failed_at: 10,
            last_failed_at: 10,
            attempt_count: 1,
        })
        .unwrap();

    let replayable = repository.list_replayable_quarantine(None, 100).unwrap();
    assert_eq!(replayable.len(), 1);
    assert_eq!(replayable[0].record_id, waiting_id);
    assert_eq!(repository.list_quarantine(100).unwrap().len(), 100);
}

#[test]
fn sqlite_sync_state_repository_preserves_tagged_heads_and_states_after_reopen() {
    let file = NamedTempFile::new().unwrap();
    let record_id = Uuid::now_v7();
    let op_id = Uuid::now_v7();
    let stored = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteSyncStateRepository::new(connection);
        let stored = repository
            .put_outbox_head(NewSyncOutboxEntry {
                op_id,
                record_id,
                collection: "tasks".to_string(),
                base_revision_hlc: Some("base-reopen".to_string()),
                revision_hlc: "revision-reopen".to_string(),
                state: SyncOutboxState::Live {
                    mutation_hlc: "mutation-reopen".to_string(),
                    blob: vec![7, 8, 9],
                },
                created_at: 1_799_000_000_000,
            })
            .unwrap();
        repository
            .put_record_state(SyncRecordState {
                record_id,
                collection: "tasks".to_string(),
                current_revision_hlc: Some("revision-reopen".to_string()),
                state: SyncRecordSemanticState::Tombstone {
                    delete_hlc: "delete-reopen".to_string(),
                },
                updated_at: 1_799_000_000_001,
            })
            .unwrap();
        stored
    };

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteSyncStateRepository::new(connection);

    assert_eq!(repository.list_outbox_heads(10).unwrap(), vec![stored]);
    assert_eq!(
        repository.get_record_state("tasks", record_id).unwrap(),
        Some(SyncRecordState {
            record_id,
            collection: "tasks".to_string(),
            current_revision_hlc: Some("revision-reopen".to_string()),
            state: SyncRecordSemanticState::Tombstone {
                delete_hlc: "delete-reopen".to_string(),
            },
            updated_at: 1_799_000_000_001,
        })
    );
}

#[test]
fn sqlite_sync_state_rejects_unknown_and_changed_collections() {
    let file = NamedTempFile::new().unwrap();
    let record_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSyncStateRepository::new(connection);

    assert!(matches!(
        repository.put_outbox_head(new_live_outbox(
            record_id,
            "unknown",
            Uuid::now_v7(),
            None,
            "revision-1",
            "mutation-1",
            vec![1],
        )),
        Err(StorageError::InvalidSyncCollection(collection)) if collection == "unknown"
    ));

    repository
        .put_record_state(live_record_state(
            record_id,
            "tasks",
            None,
            "mutation-1",
            "{}",
            1,
        ))
        .unwrap();
    assert!(matches!(
        repository.put_outbox_head(new_live_outbox(
            record_id,
            "lists",
            Uuid::now_v7(),
            None,
            "revision-2",
            "mutation-2",
            vec![2],
        )),
        Err(StorageError::SyncCollectionMismatch {
            record_id: mismatch_id,
            existing,
            requested,
        }) if mismatch_id == record_id && existing == "tasks" && requested == "lists"
    ));
    assert!(matches!(
        repository.get_record_state("lists", record_id),
        Err(StorageError::SyncCollectionMismatch { .. })
    ));
}

#[test]
fn sync_v2_schema_rejects_malformed_live_and_tombstone_rows() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let record_id = Uuid::now_v7();
    let op_id = Uuid::now_v7();

    let live_without_blob = connection.execute(
        "INSERT INTO sync_outbox (
                 record_id, collection, op_id, revision_hlc,
                 state_kind, semantic_hlc, blob, created_at
             ) VALUES (?1, 'tasks', ?2, 'revision', 'live', 'mutation', NULL, 1)",
        params![record_id.to_string(), op_id.to_string()],
    );
    assert!(live_without_blob.is_err());

    let tombstone_with_plaintext = connection.execute(
        "INSERT INTO sync_record_states (
                 record_id, collection, current_revision_hlc,
                 state_kind, semantic_hlc, plaintext_json, updated_at
             ) VALUES (?1, 'tasks', 'revision', 'tombstone', 'delete', '{}', 1)",
        [record_id.to_string()],
    );
    assert!(tombstone_with_plaintext.is_err());
}

#[test]
fn sqlite_sync_state_repository_roundtrips_pull_cursor() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSyncStateRepository::new(connection);

    assert_eq!(repository.get_cursor("default").unwrap(), None);

    repository
        .set_cursor("default", 41, 1_799_000_000_000)
        .unwrap();
    assert_eq!(
        repository.get_cursor("default").unwrap(),
        Some(SyncCursor {
            name: "default".to_string(),
            seq: 41,
            updated_at: 1_799_000_000_000,
        })
    );

    repository
        .set_cursor("default", 42, 1_799_000_001_000)
        .unwrap();
    assert_eq!(
        repository.get_cursor("default").unwrap(),
        Some(SyncCursor {
            name: "default".to_string(),
            seq: 42,
            updated_at: 1_799_000_001_000,
        })
    );
}

#[test]
fn sqlite_reminder_repository_creates_updates_deletes_and_snoozes_reminders() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(task.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteReminderRepository::new(connection);

    let first = repository
        .create_task_reminder(task.id, 1_800_000_000_000, 1_799_000_000_000)
        .unwrap();
    assert_eq!(first.task_id, task.id);
    assert_eq!(
        repository.list_task_reminders(task.id).unwrap(),
        vec![first.clone()]
    );

    let second = repository
        .create_task_reminder(task.id, 1_800_000_600_000, 1_799_000_001_000)
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(
        repository.list_task_reminders(task.id).unwrap(),
        vec![first.clone(), second.clone()]
    );

    let updated = repository
        .update_reminder(first.id, 1_800_001_200_000, 1_799_000_002_000)
        .unwrap();
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.created_at, first.created_at);
    assert_eq!(updated.remind_at, 1_800_001_200_000);

    let snoozed = repository
        .snooze_reminder(second.id, 1_800_004_200_000, 1_799_000_003_000)
        .unwrap();
    assert_eq!(snoozed.snoozed_until, Some(1_800_004_200_000));
    assert_eq!(repository.delete_reminder(updated.id).unwrap(), updated);
    assert_eq!(
        repository.clear_task_reminders(task.id).unwrap(),
        vec![snoozed]
    );
    assert!(repository.list_task_reminders(task.id).unwrap().is_empty());
}

#[test]
fn sqlite_reminder_repository_enforces_time_uniqueness_and_limit() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteReminderRepository::new(connection);
    assert!(matches!(
        repository.create_task_reminder(task.id, 100, 100),
        Err(StorageError::ReminderTimeNotFuture)
    ));

    let first = repository.create_task_reminder(task.id, 200, 100).unwrap();
    assert!(matches!(
        repository.create_task_reminder(task.id, 200, 100),
        Err(StorageError::DuplicateReminderTime)
    ));
    for remind_at in [300, 400, 500, 600] {
        repository
            .create_task_reminder(task.id, remind_at, 100)
            .unwrap();
    }
    assert!(matches!(
        repository.create_task_reminder(task.id, 700, 100),
        Err(StorageError::ReminderLimitReached {
            limit: MAX_REMINDERS_PER_TASK
        })
    ));
    assert!(matches!(
        repository.update_reminder(first.id, 300, 100),
        Err(StorageError::DuplicateReminderTime)
    ));
}

#[test]
fn sqlite_reminder_repository_lists_pending_open_tasks_only() {
    let file = NamedTempFile::new().unwrap();
    let mut pending_task = sample_task();
    pending_task.status = TaskStatus::Todo;
    pending_task.sort_order = "a0".to_string();
    let mut closed_task = sample_task();
    closed_task.status = TaskStatus::Todo;
    closed_task.sort_order = "a1".to_string();
    let mut expired_task = sample_task();
    expired_task.status = TaskStatus::Todo;
    expired_task.sort_order = "a2".to_string();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(pending_task.clone()).unwrap();
        task_repository.insert(closed_task.clone()).unwrap();
        task_repository.insert(expired_task.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteReminderRepository::new(connection);
    let pending = repository
        .create_task_reminder(pending_task.id, 1_800_000_000_000, 1_799_000_000_000)
        .unwrap();
    repository
        .create_task_reminder(closed_task.id, 1_800_000_000_000, 1_799_000_000_000)
        .unwrap();
    repository
        .connection()
        .execute(
            "UPDATE tasks SET status = 'done', completed_at = ?2 WHERE id = ?1",
            params![closed_task.id.to_string(), 1_799_000_010_000_i64],
        )
        .unwrap();
    repository
        .create_task_reminder(expired_task.id, 1_799_999_999_999, 1_799_000_000_000)
        .unwrap();

    assert_eq!(
        repository
            .list_pending_reminders(1_799_999_999_999)
            .unwrap(),
        vec![pending]
    );
}

#[test]
fn reminder_notification_commands_join_context_and_persist_unique_platform_ids() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
    }

    let occupied_reminder_id = Uuid::now_v7();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        connection
            .execute(
                "INSERT INTO reminder_notification_ids (
                     platform_id, reminder_id, command_revision, retired
                 ) VALUES (1, ?1, 0, 1)",
                [occupied_reminder_id.to_string()],
            )
            .unwrap();
    }
    let reminder = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteReminderRepository::new(connection)
            .create_task_reminder(task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap()
    };

    let first_command = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        let commands = repository.list_commands(1_799_000_000_000, 10).unwrap();
        assert_eq!(commands.len(), 1);
        let command = commands[0].clone();
        assert_eq!(command.reminder_id, reminder.id);
        assert_eq!(command.platform_id, 2);
        assert_eq!(command.action, ReminderNotificationAction::Schedule);
        assert_eq!(command.task_id, Some(task.id));
        assert_eq!(command.list_id, Some(list.id));
        assert_eq!(command.scheduled_at, Some(1_800_000_000_000));
        assert!(repository
            .ack_command(command.reminder_id, command.revision)
            .unwrap());
        command
    };

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderRepository::new(connection);
        repository
            .update_reminder(reminder.id, 1_800_000_600_000, 1_799_000_000_001)
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        let command = repository
            .list_commands(1_799_000_000_000, 10)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(command.platform_id, first_command.platform_id);
        assert!(command.revision > first_command.revision);
        assert!(!repository
            .ack_command(command.reminder_id, first_command.revision)
            .unwrap());
        assert!(repository
            .ack_command(command.reminder_id, command.revision)
            .unwrap());
    }

    let persisted_platform_id: i64 = open_encrypted(file.path(), &KEY)
        .unwrap()
        .query_row(
            "SELECT platform_id
             FROM reminder_notification_ids
             WHERE reminder_id = ?1",
            [reminder.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_platform_id, i64::from(first_command.platform_id));

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteReminderRepository::new(connection)
            .delete_reminder(reminder.id)
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        let cancel = repository
            .list_commands(1_799_000_000_000, 10)
            .unwrap()
            .remove(0);
        assert_eq!(cancel.action, ReminderNotificationAction::Cancel);
        assert_eq!(cancel.platform_id, first_command.platform_id);
        assert!(repository
            .ack_command(cancel.reminder_id, cancel.revision)
            .unwrap());
        assert!(repository
            .prepare_reconciliation(1_799_000_000_000)
            .unwrap()
            .is_empty());
        let (platform_id, retired): (i64, i64) = repository
            .connection()
            .query_row(
                "SELECT platform_id, retired
                 FROM reminder_notification_ids
                 WHERE reminder_id = ?1",
                [reminder.id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(platform_id, i64::from(first_command.platform_id));
        assert_eq!(retired, 1);
    }
}

#[test]
fn reminder_notification_commands_rebuild_after_ack_and_track_task_lifecycle() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
    }
    let reminder = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteReminderRepository::new(connection)
            .create_task_reminder(task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap()
    };
    let original = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        let command = repository
            .list_commands(1_799_000_000_000, 10)
            .unwrap()
            .remove(0);
        assert!(repository
            .ack_command(command.reminder_id, command.revision)
            .unwrap());
        command
    };

    let rebuilt = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        repository
            .prepare_reconciliation(1_799_000_000_000)
            .unwrap()
            .remove(0)
    };
    assert_eq!(rebuilt.reminder_id, reminder.id);
    assert_eq!(rebuilt.platform_id, original.platform_id);
    assert!(rebuilt.revision > original.revision);
    assert_eq!(rebuilt.action, ReminderNotificationAction::Schedule);

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        connection
            .execute(
                "UPDATE tasks
                 SET status = 'done', completed_at = ?2
                 WHERE id = ?1",
                params![task.id.to_string(), 1_799_000_000_100_i64],
            )
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteReminderNotificationRepository::new(connection);
        let command = repository
            .list_commands(1_799_000_000_000, 10)
            .unwrap()
            .remove(0);
        assert_eq!(command.action, ReminderNotificationAction::Cancel);
        assert_eq!(command.task_id, Some(task.id));
        assert_eq!(command.platform_id, original.platform_id);
        assert!(repository
            .ack_command(command.reminder_id, command.revision)
            .unwrap());
    }

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        connection
            .execute(
                "UPDATE tasks
                 SET status = 'todo', completed_at = NULL
                 WHERE id = ?1",
                [task.id.to_string()],
            )
            .unwrap();
    }
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteReminderNotificationRepository::new(connection);
    let command = repository
        .list_commands(1_799_000_000_000, 10)
        .unwrap()
        .remove(0);
    assert_eq!(command.action, ReminderNotificationAction::Schedule);
    assert_eq!(command.list_id, Some(list.id));
}

#[test]
fn sqlite_reminder_repository_lists_subtree_and_list_reminders_for_cancellation() {
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
    let mut other = sample_task();
    other.list_id = list.id;
    other.parent_task_id = None;
    other.sort_order = "a2".to_string();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(parent.clone()).unwrap();
        task_repository.insert(child.clone()).unwrap();
        task_repository.insert(other.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteReminderRepository::new(connection);
    let parent_reminder = repository
        .create_task_reminder(parent.id, 1_800_000_000_000, 1_799_000_000_000)
        .unwrap();
    let child_reminder = repository
        .create_task_reminder(child.id, 1_800_000_600_000, 1_799_000_000_000)
        .unwrap();
    let other_reminder = repository
        .create_task_reminder(other.id, 1_800_001_200_000, 1_799_000_000_000)
        .unwrap();

    assert_eq!(
        repository.list_task_subtree_reminders(parent.id).unwrap(),
        vec![parent_reminder.clone(), child_reminder.clone()]
    );
    assert_eq!(
        repository.list_list_reminders(list.id).unwrap(),
        vec![parent_reminder, child_reminder, other_reminder]
    );
}

#[test]
fn task_delete_removes_reminders_but_list_delete_rehomes_them() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut default_list = sample_list("a1");
    default_list.is_default = true;
    let mut subtree_task = sample_task();
    subtree_task.list_id = list.id;
    subtree_task.parent_task_id = None;
    subtree_task.sort_order = "a0".to_string();
    let mut list_task = sample_task();
    list_task.list_id = list.id;
    list_task.parent_task_id = None;
    list_task.sort_order = "a1".to_string();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
        list_repository.insert(default_list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(subtree_task.clone()).unwrap();
        task_repository.insert(list_task.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut reminder_repository = SqliteReminderRepository::new(connection);
        reminder_repository
            .create_task_reminder(subtree_task.id, 1_800_000_000_000, 1_799_000_000_000)
            .unwrap();
        reminder_repository
            .create_task_reminder(list_task.id, 1_800_000_600_000, 1_799_000_000_000)
            .unwrap();
    }

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.delete_subtree(subtree_task.id).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let reminder_repository = SqliteReminderRepository::new(connection);
        assert!(reminder_repository
            .list_task_reminders(subtree_task.id)
            .unwrap()
            .is_empty());
        assert_eq!(
            reminder_repository
                .list_task_reminders(list_task.id)
                .unwrap()
                .len(),
            1
        );
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut list_repository = SqliteListRepository::new(connection);
    list_repository.delete_and_rehome_tasks(list.id).unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let reminder_repository = SqliteReminderRepository::new(connection);
    assert!(reminder_repository
        .list_list_reminders(list.id)
        .unwrap()
        .is_empty());
    assert_eq!(
        reminder_repository
            .list_list_reminders(default_list.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn ensure_default_list_creates_default_when_missing_and_keeps_existing_name() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);

    let inbox = repository
        .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
        .unwrap();
    let again = repository
        .ensure_default_list("インボックス".to_string(), 1_799_000_001_000)
        .unwrap();

    assert_eq!(inbox.id, again.id);
    assert_eq!(again.name, "Inbox");
    assert!(again.is_default);
    assert_eq!(repository.list_all().unwrap().len(), 1);
}

#[test]
fn ensure_default_list_observes_ja_name_in_empty_database() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);

    let inbox = repository
        .ensure_default_list("インボックス".to_string(), 1_799_000_000_000)
        .unwrap();

    assert_eq!(inbox.name, "インボックス");
    assert!(inbox.is_default);
}

#[test]
fn unique_index_prevents_multiple_default_lists() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);
    let first = repository
        .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
        .unwrap();
    let mut second = sample_list("a1");
    second.is_default = true;

    let result = repository.insert(second);

    assert!(matches!(result, Err(StorageError::Sqlite(_))));
    assert_eq!(repository.get_default().unwrap().unwrap().id, first.id);
}

#[test]
fn default_list_cannot_be_archived_or_deleted_but_can_be_renamed() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteListRepository::new(connection);
    let mut list = repository
        .ensure_default_list("Inbox".to_string(), 1_799_000_000_000)
        .unwrap();

    list.name = "Renamed inbox".to_string();
    list.updated_at += 1;
    repository.update(list.clone()).unwrap();
    assert_eq!(repository.get(list.id).unwrap().name, "Renamed inbox");
    assert!(repository.get(list.id).unwrap().is_default);

    let mut archived = list.clone();
    archived.archived_at = Some(1_799_000_001_000);
    assert!(matches!(
        repository.update(archived),
        Err(StorageError::DefaultListProtected {
            operation: "archived",
            list_id,
        }) if list_id == list.id
    ));
    assert!(matches!(
        repository.delete_and_rehome_tasks(list.id),
        Err(StorageError::DefaultListProtected {
            operation: "deleted",
            list_id,
        }) if list_id == list.id
    ));
}
