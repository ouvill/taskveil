use super::*;

#[test]
fn encrypted_connections_have_finite_busy_timeout_and_write_tx_locks_immediately() {
    let file = NamedTempFile::new().unwrap();
    let mut first = open_encrypted(file.path(), &KEY).unwrap();
    let second = open_encrypted(file.path(), &KEY).unwrap();
    let busy_timeout_ms: i64 = second
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    assert_eq!(busy_timeout_ms, 5_000);
    second.busy_timeout(Duration::ZERO).unwrap();

    let _write_tx = SqliteWriteTx::begin(&mut first).unwrap();
    let result = second.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
        params!["other_writer", "blocked", 1],
    );
    assert!(matches!(
        result,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy
    ));
}

#[test]
fn full_resync_progress_and_marks_roll_back_together() {
    let file = NamedTempFile::new().unwrap();
    let generation_id = Uuid::now_v7();
    let record_id = Uuid::now_v7();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 17, 100)
        .unwrap();
    transaction.rollback().unwrap();
    let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(repository.load_full_resync().unwrap(), None);
    drop(repository);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 17, 100)
        .unwrap();
    transaction.commit().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .mark_full_resync_record(generation_id, "tasks", record_id)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 110)
        .unwrap();
    transaction.rollback().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let progress = load_full_resync_on(&connection).unwrap().unwrap();
    assert_eq!(progress.phase, FullResyncPhase::Base);
    let mark_count: i64 = connection
        .query_row("SELECT count(*) FROM sync_full_resync_marks", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(mark_count, 0);
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 120)
        .unwrap();
    transaction.commit().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .advance_full_resync_delta(generation_id, 18, 130)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 18, 131)
        .unwrap();
    transaction.rollback().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let progress = load_full_resync_on(&connection).unwrap().unwrap();
    assert_eq!(progress.phase, FullResyncPhase::Delta);
    assert_eq!(progress.delta_cursor, 17);
    assert_eq!(progress.closure_high_water, None);
}

#[test]
fn full_resync_sweep_preserves_marks_but_purges_server_seen_absent_outbox() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("00000000000000010000000000000000");
    let mut remote_task = sample_task();
    remote_task.id = Uuid::now_v7();
    remote_task.list_id = list.id;
    remote_task.parent_task_id = None;
    let mut pending_task = remote_task.clone();
    pending_task.id = Uuid::now_v7();
    let mut absent_task = remote_task.clone();
    absent_task.id = Uuid::now_v7();

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    {
        let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
        transaction.insert_list(list.clone()).unwrap();
        for task in [&remote_task, &pending_task, &absent_task] {
            transaction.insert_task(task.clone()).unwrap();
            transaction
                .put_record_state(live_record_state(
                    task.id,
                    "tasks",
                    Some("r1"),
                    "m1",
                    "{}",
                    1,
                ))
                .unwrap();
        }
        transaction
            .put_record_state(live_record_state(
                list.id,
                "lists",
                Some("r1"),
                "m1",
                "{}",
                1,
            ))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                pending_task.id,
                "tasks",
                Uuid::now_v7(),
                Some("r1"),
                "r2",
                "m2",
                vec![1],
            ))
            .unwrap();
        transaction.commit().unwrap();
    }

    let generation_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 5, 10)
        .unwrap();
    transaction
        .mark_full_resync_record(generation_id, "lists", list.id)
        .unwrap();
    transaction
        .mark_full_resync_record(generation_id, "tasks", remote_task.id)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 11)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 5, 12)
        .unwrap();
    transaction.commit().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    let rolled_back = transaction
        .sweep_full_resync_batch(generation_id, 1, 19)
        .unwrap();
    assert_eq!(rolled_back.scanned_records, 1);
    transaction.rollback().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let progress = load_full_resync_on(&connection).unwrap().unwrap();
    assert_eq!(progress.sweep_cursor, None);
    let state_count: i64 = connection
        .query_row("SELECT count(*) FROM sync_record_states", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state_count, 4);

    let mut total = FullResyncSweepSummary::default();
    for now in 20..30 {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let batch = transaction
            .sweep_full_resync_batch(generation_id, 1, now)
            .unwrap();
        transaction.commit().unwrap();
        total.scanned_records += batch.scanned_records;
        total.swept_lists += batch.swept_lists;
        total.swept_tasks += batch.swept_tasks;
        total.swept_record_states += batch.swept_record_states;
        if batch.scanned_records == 0 {
            break;
        }
    }
    assert_eq!(total.scanned_records, 4);
    assert_eq!(total.swept_tasks, 2);
    assert_eq!(total.swept_record_states, 2);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    assert_eq!(
        transaction
            .finalize_full_resync(generation_id, "default", 30)
            .unwrap(),
        5
    );
    transaction.rollback().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(get_cursor_on(&connection, "default").unwrap(), None);
    assert!(load_full_resync_on(&connection).unwrap().is_some());

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    assert_eq!(
        transaction
            .finalize_full_resync(generation_id, "default", 31)
            .unwrap(),
        5
    );
    transaction.commit().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert!(get_task_on(&connection, remote_task.id).is_ok());
    assert!(matches!(
        get_task_on(&connection, pending_task.id),
        Err(StorageError::NotFound(_))
    ));
    assert!(matches!(
        get_task_on(&connection, absent_task.id),
        Err(StorageError::NotFound(_))
    ));
    assert!(!has_outbox_head_on(&connection, "tasks", pending_task.id).unwrap());
    assert_eq!(
        get_cursor_on(&connection, "default").unwrap().unwrap().seq,
        5
    );
    assert_eq!(load_full_resync_on(&connection).unwrap(), None);
}

#[test]
fn full_resync_sweeps_absent_series_then_template_across_stable_batches() {
    let file = NamedTempFile::new().unwrap();
    let template = sample_template();
    let schedule = sample_schedule(template.id);
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    transaction.upsert_template(template.clone()).unwrap();
    transaction.upsert_series(schedule.clone()).unwrap();
    for (id, collection) in [(template.id, "templates"), (schedule.id, "task_series")] {
        transaction
            .put_record_state(live_record_state(
                id,
                collection,
                Some("server-r1"),
                "m1",
                "{}",
                1,
            ))
            .unwrap();
    }
    transaction.commit().unwrap();

    let generation_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 0, 10)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 11)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 0, 12)
        .unwrap();
    transaction.commit().unwrap();

    let mut total = FullResyncSweepSummary::default();
    for now in 20..24 {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let batch = transaction
            .sweep_full_resync_batch(generation_id, 1, now)
            .unwrap();
        transaction.commit().unwrap();
        total.scanned_records += batch.scanned_records;
        total.swept_templates += batch.swept_templates;
        total.swept_task_series += batch.swept_task_series;
        total.swept_record_states += batch.swept_record_states;
        if batch.scanned_records == 0 {
            break;
        }
    }

    assert_eq!(total.scanned_records, 2);
    assert_eq!(total.swept_task_series, 1);
    assert_eq!(total.swept_templates, 1);
    assert_eq!(total.swept_record_states, 2);
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert!(matches!(
        get_series_on(&connection, schedule.id),
        Err(StorageError::NotFound(_))
    ));
    assert!(matches!(
        get_template_on(&connection, template.id),
        Err(StorageError::NotFound(_))
    ));
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    assert_eq!(
        transaction
            .finalize_full_resync(generation_id, "default", 30)
            .unwrap(),
        0
    );
    transaction.commit().unwrap();
}

#[test]
fn full_resync_preserves_never_synced_series_independently_of_template() {
    let file = NamedTempFile::new().unwrap();
    let template = sample_template();
    let schedule = sample_schedule(template.id);
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    transaction.upsert_template(template.clone()).unwrap();
    transaction.upsert_series(schedule.clone()).unwrap();
    for (id, collection) in [(template.id, "templates"), (schedule.id, "task_series")] {
        transaction
            .put_record_state(live_record_state(id, collection, None, "m1", "{}", 1))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                id,
                collection,
                Uuid::now_v7(),
                None,
                "r1",
                "m1",
                vec![1],
            ))
            .unwrap();
    }
    transaction.commit().unwrap();

    let generation_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 0, 10)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 11)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 0, 12)
        .unwrap();
    transaction.commit().unwrap();

    loop {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let batch = transaction
            .sweep_full_resync_batch(generation_id, 1, 20)
            .unwrap();
        transaction.commit().unwrap();
        if batch.scanned_records == 0 {
            break;
        }
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(get_series_on(&connection, schedule.id).unwrap(), schedule);
    assert_eq!(get_template_on(&connection, template.id).unwrap(), template);
    assert!(has_outbox_head_on(&connection, "task_series", schedule.id).unwrap());
    assert!(has_outbox_head_on(&connection, "templates", template.id).unwrap());
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .finalize_full_resync(generation_id, "default", 30)
        .unwrap();
    transaction.commit().unwrap();
}

#[test]
fn full_resync_preserves_valid_never_synced_list_and_task_in_dependency_order() {
    let file = NamedTempFile::new().unwrap();
    let tenant_id = Uuid::now_v7();
    let list = sample_list("00000000000000010000000000000000");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let mut local_crypto =
        SqliteLocalCryptoRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    local_crypto
        .bind_tenant_root(
            LocalProfileBinding {
                tenant_id,
                user_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
                bound_at: 1,
                updated_at: 1,
            },
            &local_tenant_root_bundle(tenant_id, 1),
        )
        .unwrap();
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    transaction.insert_list(list.clone()).unwrap();
    transaction.insert_task(task.clone()).unwrap();
    for (id, collection) in [(list.id, "lists"), (task.id, "tasks")] {
        transaction
            .put_record_state(live_record_state(id, collection, None, "m1", "{}", 1))
            .unwrap();
        transaction
            .put_outbox_head(new_live_outbox(
                id,
                collection,
                Uuid::now_v7(),
                None,
                "r1",
                "m1",
                vec![1],
            ))
            .unwrap();
    }
    transaction.commit().unwrap();

    let generation_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 0, 10)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 11)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 0, 12)
        .unwrap();
    transaction.commit().unwrap();

    loop {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let batch = transaction
            .sweep_full_resync_batch(generation_id, 10, 20)
            .unwrap();
        transaction.commit().unwrap();
        if batch.scanned_records == 0 {
            break;
        }
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert!(get_list_on(&connection, list.id).is_ok());
    assert!(get_task_on(&connection, task.id).is_ok());
    let outbox = list_outbox_heads_on(&connection, 10).unwrap();
    assert_eq!(outbox.len(), 2);
    assert_eq!(outbox[0].collection, "lists");
    assert_eq!(outbox[1].collection, "tasks");
}

#[test]
fn full_resync_preserves_never_synced_task_under_remote_current_list() {
    let file = NamedTempFile::new().unwrap();
    let tenant_id = Uuid::now_v7();
    let list = sample_list("00000000000000010000000000000000");
    let mut task = sample_task();
    task.list_id = list.id;
    task.parent_task_id = None;
    let mut crypto = SqliteLocalCryptoRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    crypto
        .bind_tenant_root(
            LocalProfileBinding {
                tenant_id,
                user_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
                bound_at: 1,
                updated_at: 1,
            },
            &local_tenant_root_bundle(tenant_id, 1),
        )
        .unwrap();
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    transaction.insert_list(list.clone()).unwrap();
    transaction.insert_task(task.clone()).unwrap();
    transaction
        .put_record_state(live_record_state(
            list.id,
            "lists",
            Some("r1"),
            "m1",
            "{}",
            1,
        ))
        .unwrap();
    transaction
        .put_record_state(live_record_state(task.id, "tasks", None, "m1", "{}", 1))
        .unwrap();
    transaction
        .put_outbox_head(new_live_outbox(
            task.id,
            "tasks",
            Uuid::now_v7(),
            None,
            "r1",
            "m1",
            vec![1],
        ))
        .unwrap();
    transaction.commit().unwrap();

    let generation_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .start_full_resync(generation_id, 1, 1, 10)
        .unwrap();
    transaction
        .mark_full_resync_record(generation_id, "lists", list.id)
        .unwrap();
    transaction
        .advance_full_resync_base(generation_id, None, true, 11)
        .unwrap();
    transaction
        .advance_full_resync_delta(generation_id, 1, 12)
        .unwrap();
    transaction
        .enter_full_resync_sweep(generation_id, 1, 13)
        .unwrap();
    transaction.commit().unwrap();

    loop {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        let batch = transaction
            .sweep_full_resync_batch(generation_id, 10, 20)
            .unwrap();
        transaction.commit().unwrap();
        if batch.scanned_records == 0 {
            break;
        }
    }
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert!(get_task_on(&connection, task.id).is_ok());
    assert!(has_outbox_head_on(&connection, "tasks", task.id).unwrap());
}

#[test]
fn v16_empty_database_migrates_to_typed_due_and_rejects_mixed_shape() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_raw_encrypted(file.path(), &KEY);
    connection
        .execute_batch(
            "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY NOT NULL, list_id TEXT NOT NULL,
                    parent_task_id TEXT, title TEXT NOT NULL, note TEXT NOT NULL,
                    status TEXT NOT NULL, priority INTEGER NOT NULL, due_at INTEGER,
                    scheduled_at INTEGER, estimated_minutes INTEGER, sort_order TEXT NOT NULL,
                    completed_at INTEGER, closed_reason TEXT, deleted_at INTEGER,
                    assignee TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 16;",
        )
        .unwrap();
    drop(connection);

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(
        read_user_version(&connection).unwrap(),
        LATEST_SCHEMA_VERSION
    );
    assert!(table_columns_raw(&connection, "tasks")
        .unwrap()
        .iter()
        .any(|column| column == "due_kind"));
    assert!(connection
        .execute(
            "INSERT INTO tasks (
                    id, list_id, title, note, status, priority,
                    due_kind, due_on, sort_order, created_at, updated_at
                 ) VALUES ('task', 'list', 'title', '', 'todo', 0,
                           'datetime', '2026-07-12', 'a0', 1, 1)",
            [],
        )
        .is_err());
}

#[test]
fn v17_to_v22_preserves_domain_and_resets_pre_protocol_v8_transport_state() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Preserved".into(), "a0".into(), 100).unwrap();
    let task = new_task(list.id, None, "Preserved task".into(), "a0".into(), 100).unwrap();
    let quarantine_id = Uuid::now_v7();
    let generation_id = Uuid::now_v7();
    create_baseline_v1_database(file.path(), &KEY, true);
    {
        let mut connection = open_raw_encrypted(file.path(), &KEY);
        let transaction = connection.transaction().unwrap();
        for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 17) {
            (migration.apply)(&transaction).unwrap();
        }
        set_user_version(&transaction, 17).unwrap();
        transaction.commit().unwrap();
        insert_list_on(&connection, &list).unwrap();
        insert_task_pre_v20(&connection, &task);
        connection
            .execute_batch(&format!(
                "INSERT INTO sync_outbox (
                         record_id, collection, op_id, revision_hlc, state_kind,
                         semantic_hlc, blob, created_at
                     ) VALUES ('{task_id}', 'tasks', '{op_id}', 'r1', 'live', 'm1', x'01', 1);
                     INSERT INTO sync_record_states (
                         record_id, collection, current_revision_hlc, state_kind,
                         semantic_hlc, plaintext_json, updated_at
                     ) VALUES ('{task_id}', 'tasks', 'r0', 'tombstone', 'd1', NULL, 1);
                     INSERT INTO sync_cursors(name, seq, updated_at) VALUES ('main', 42, 1);
                     INSERT INTO sync_record_origins(record_id, collection, origin_kind, updated_at)
                     VALUES ('{task_id}', 'tasks', 'server_seen', 1);
                     INSERT INTO sync_quarantine (
                         record_id, collection, seq, revision_hlc, state_kind,
                         semantic_hlc, blob, reason, required_list_id,
                         first_failed_at, last_failed_at, attempt_count
                     ) VALUES ('{quarantine_id}', 'tasks', 7, 'r7', 'live', 'm7', x'02',
                               'corrupt_envelope', NULL, 2, 3, 1);
                     INSERT INTO sync_full_resync_state (
                         singleton, generation_id, phase, base_seq,
                         base_cursor_collection, base_cursor_record_id, delta_cursor,
                         closure_high_water, sweep_cursor_collection, sweep_cursor_record_id,
                         started_at, updated_at, continuity_generation
                     ) VALUES (1, '{generation_id}', 'sweep', 42,
                               'tasks', '{task_id}', 42, 42, 'tasks', '{task_id}', 1, 2, 3);
                     INSERT INTO sync_full_resync_marks(generation_id, collection, record_id)
                     VALUES ('{generation_id}', 'tasks', '{task_id}');
                     PRAGMA user_version = 17;",
                task_id = task.id,
                op_id = Uuid::now_v7(),
                quarantine_id = quarantine_id,
                generation_id = generation_id,
            ))
            .unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(
        read_user_version(&connection).unwrap(),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(get_cursor_on(&connection, "main").unwrap(), None);
    for table in [
        "sync_outbox",
        "sync_record_states",
        "sync_quarantine",
        "sync_full_resync_state",
        "sync_full_resync_marks",
        "sync_record_origins",
    ] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0,
            "{table} must be reset for protocol v8"
        );
    }
    assert_eq!(
        SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .get(task.id)
            .unwrap(),
        task
    );
    for table in [
        "timer_sessions",
        "active_timer_session",
        "local_tenant_root_key_cache",
    ] {
        assert!(!table_columns_raw(&connection, table).unwrap().is_empty());
    }
    drop(connection);
    assert_eq!(
        read_user_version(&open_encrypted(file.path(), &KEY).unwrap()).unwrap(),
        LATEST_SCHEMA_VERSION
    );
}

#[test]
fn v18_to_v19_adds_durable_list_aliases_and_reopens() {
    let file = NamedTempFile::new().unwrap();
    let mut old_default = sample_list("a0");
    old_default.is_default = true;
    let canonical = sample_list("a1");
    create_baseline_v1_database(file.path(), &KEY, true);
    {
        let mut connection = open_raw_encrypted(file.path(), &KEY);
        let transaction = connection.transaction().unwrap();
        for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 18) {
            (migration.apply)(&transaction).unwrap();
        }
        set_user_version(&transaction, 18).unwrap();
        transaction.commit().unwrap();
        insert_list_on(&connection, &old_default).unwrap();
        insert_list_on(&connection, &canonical).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(
        read_user_version(&connection).unwrap(),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        table_columns_raw(&connection, "list_aliases").unwrap(),
        vec!["alias_list_id", "canonical_list_id", "updated_at"]
    );
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .materialize_canonical_list(canonical.id)
        .unwrap();
    transaction
        .replace_list_aliases(canonical.id, &[old_default.id], 123)
        .unwrap();
    transaction.commit().unwrap();

    let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(
        repository.resolve_list_alias(old_default.id).unwrap(),
        canonical.id
    );
    assert_eq!(
        repository.list_list_aliases().unwrap(),
        vec![ListAlias {
            alias_list_id: old_default.id,
            canonical_list_id: canonical.id,
            updated_at: 123,
        }]
    );
    assert_eq!(
        read_user_version(&open_encrypted(file.path(), &KEY).unwrap()).unwrap(),
        LATEST_SCHEMA_VERSION
    );
}

#[test]
fn v19_to_v22_preserves_tasks_and_resets_legacy_recurrence_sync_metadata() {
    let file = NamedTempFile::new().unwrap();
    create_baseline_v1_database(file.path(), &KEY, true);
    let list = new_list("Preserved".into(), "a0".into(), 100).unwrap();
    let task = new_task(list.id, None, "Preserved task".into(), "a0".into(), 100).unwrap();
    let tombstone_id = Uuid::now_v7();
    {
        let mut connection = open_raw_encrypted(file.path(), &KEY);
        let transaction = connection.transaction().unwrap();
        for migration in MIGRATIONS.iter().filter(|value| value.target_version <= 19) {
            (migration.apply)(&transaction).unwrap();
        }
        set_user_version(&transaction, 19).unwrap();
        transaction.commit().unwrap();
        insert_list_on(&connection, &list).unwrap();
        insert_task_pre_v20(&connection, &task);
        put_record_state_on(
            &connection,
            live_record_state(task.id, "tasks", Some("r0"), "m1", "{}", 1),
        )
        .unwrap();
        put_record_state_on(
            &connection,
            SyncRecordState {
                record_id: tombstone_id,
                collection: "tasks".to_string(),
                current_revision_hlc: Some("r1".to_string()),
                state: SyncRecordSemanticState::Tombstone {
                    delete_hlc: "d1".to_string(),
                },
                updated_at: 2,
            },
        )
        .unwrap();
        set_cursor_on(&connection, "main", 42, 3).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    assert_eq!(
        read_user_version(&connection).unwrap(),
        LATEST_SCHEMA_VERSION
    );
    let migrated = get_task_on(&connection, task.id).unwrap();
    assert_eq!(migrated.series_occurrence, None);
    assert_eq!(migrated.content.title, task.content.title);
    assert_eq!(get_cursor_on(&connection, "main").unwrap(), None);
    assert_eq!(
        get_record_state_on(&connection, "tasks", tombstone_id).unwrap(),
        None
    );
    for table in ["templates", "task_series"] {
        assert!(!table_columns_raw(&connection, table).unwrap().is_empty());
    }
    for collection in ["templates", "task_series"] {
        assert!(connection
            .execute(
                "INSERT INTO sync_cursors(name, seq, updated_at) VALUES (?1, 0, 1)",
                [format!("cursor-{collection}")],
            )
            .is_ok());
        assert!(connection
            .execute(
                "INSERT INTO sync_record_states (
                         record_id, collection, current_revision_hlc, state_kind,
                         semantic_hlc, plaintext_json, updated_at
                     ) VALUES (?1, ?2, NULL, 'tombstone', 'd1', NULL, 1)",
                params![Uuid::now_v7().to_string(), collection],
            )
            .is_ok());
    }
}

#[test]
fn canonical_materialization_hides_alias_and_resolves_product_reads() {
    let file = NamedTempFile::new().unwrap();
    let mut old_default = sample_list("a0");
    old_default.is_default = true;
    let canonical = sample_list("a1");
    let alias_task = new_task(
        old_default.id,
        None,
        "Alias task".to_string(),
        "a0".to_string(),
        100,
    )
    .unwrap();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(old_default.clone()).unwrap();
        lists.insert(canonical.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(alias_task.clone())
            .unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .materialize_canonical_list(canonical.id)
        .unwrap();
    transaction
        .replace_list_aliases(canonical.id, &[old_default.id], 200)
        .unwrap();
    let mut moved = transaction
        .list_all_tasks_by_list_for_sync(old_default.id)
        .unwrap()
        .pop()
        .unwrap();
    moved.list_id = canonical.id;
    transaction.upsert_task_for_sync(moved).unwrap();
    transaction.commit().unwrap();

    let repository_task = new_task(
        old_default.id,
        None,
        "Repository alias task".to_string(),
        "a1".to_string(),
        201,
    )
    .unwrap();
    let mut tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    tasks.insert(repository_task.clone()).unwrap();
    assert_eq!(tasks.get(repository_task.id).unwrap().list_id, canonical.id);

    let tx_task = new_task(
        old_default.id,
        None,
        "Transaction alias task".to_string(),
        "a2".to_string(),
        202,
    )
    .unwrap();
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    transaction.insert_task(tx_task.clone()).unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        get_task_on(&connection, tx_task.id).unwrap().list_id,
        canonical.id
    );

    let lists = SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    let product_lists = lists.list_all().unwrap();
    assert_eq!(product_lists.len(), 1);
    assert_eq!(product_lists[0].id, canonical.id);
    assert!(product_lists[0].is_default);
    assert_eq!(lists.get(old_default.id).unwrap().id, canonical.id);
    assert_eq!(
        lists.get_raw_for_sync(old_default.id).unwrap().id,
        old_default.id
    );
    assert_eq!(lists.list_all_for_sync().unwrap().len(), 2);
    let tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    let resolved = tasks.list_active_by_list(old_default.id).unwrap();
    assert_eq!(resolved.len(), 3);
    assert!(resolved.iter().all(|task| task.list_id == canonical.id));
}

#[test]
fn canonical_cutover_drop_rolls_back_domain_alias_task_and_transport_writes() {
    let file = NamedTempFile::new().unwrap();
    let mut old_default = sample_list("a0");
    old_default.is_default = true;
    let canonical = sample_list("a1");
    let alias_task = new_task(
        old_default.id,
        None,
        "Rollback task".to_string(),
        "a0".to_string(),
        100,
    )
    .unwrap();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut lists = SqliteListRepository::new(connection);
        lists.insert(old_default.clone()).unwrap();
        lists.insert(canonical.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(alias_task.clone())
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
        transaction
            .materialize_canonical_list(canonical.id)
            .unwrap();
        transaction
            .replace_list_aliases(canonical.id, &[old_default.id], 300)
            .unwrap();
        let mut moved = alias_task.clone();
        moved.list_id = canonical.id;
        transaction.upsert_task_for_sync(moved).unwrap();
        transaction
            .put_record_state(live_record_state(
                alias_task.id,
                "tasks",
                Some("r1"),
                "m1",
                "{}",
                300,
            ))
            .unwrap();
        // Drop without commit: OwnedSqliteWriteTx must roll back every table.
    }

    let lists = SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(lists.get_default().unwrap().unwrap().id, old_default.id);
    assert_eq!(lists.list_all().unwrap().len(), 2);
    let sync = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert!(sync.list_list_aliases().unwrap().is_empty());
    assert!(sync
        .get_record_state("tasks", alias_task.id)
        .unwrap()
        .is_none());
    let tasks = SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(tasks.get(alias_task.id).unwrap().list_id, old_default.id);
}

#[test]
fn raw_list_states_and_all_live_quarantine_reasons_are_visible_to_election() {
    let file = NamedTempFile::new().unwrap();
    let list_id = Uuid::now_v7();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = OwnedSqliteWriteTx::begin(connection).unwrap();
    transaction
        .put_record_state(live_record_state(
            list_id,
            "lists",
            Some("r1"),
            "m1",
            "{\"authenticated\":true}",
            400,
        ))
        .unwrap();
    transaction
        .put_quarantine(SyncQuarantineEntry {
            record_id: list_id,
            collection: "lists".to_string(),
            seq: 1,
            revision_hlc: "r2".to_string(),
            state: SyncOutboxState::Live {
                mutation_hlc: "m2".to_string(),
                blob: vec![1],
            },
            reason: "corrupt_envelope".to_string(),
            required_list_id: None,
            first_failed_at: 401,
            last_failed_at: 401,
            attempt_count: 1,
        })
        .unwrap();
    assert_eq!(transaction.list_record_states("lists").unwrap().len(), 1);
    assert!(transaction.has_live_quarantine("lists").unwrap());
    assert!(!transaction.has_live_quarantine("tasks").unwrap());
    transaction.commit().unwrap();

    let repository = SqliteSyncStateRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(repository.list_record_states("lists").unwrap().len(), 1);
    assert!(repository.has_live_quarantine("lists").unwrap());
}

#[test]
fn v16_profile_with_ambiguous_due_data_requires_recreation() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_raw_encrypted(file.path(), &KEY);
    connection
        .execute_batch(
            "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY NOT NULL, list_id TEXT NOT NULL,
                    parent_task_id TEXT, title TEXT NOT NULL, note TEXT NOT NULL,
                    status TEXT NOT NULL, priority INTEGER NOT NULL, due_at INTEGER,
                    scheduled_at INTEGER, estimated_minutes INTEGER, sort_order TEXT NOT NULL,
                    completed_at INTEGER, closed_reason TEXT, deleted_at INTEGER,
                    assignee TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 INSERT INTO tasks (
                    id, list_id, title, note, status, priority, due_at,
                    sort_order, created_at, updated_at
                 ) VALUES ('task', 'list', 'title', '', 'todo', 0, 0, 'a0', 1, 1);
                 PRAGMA user_version = 16;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        open_encrypted(file.path(), &KEY),
        Err(StorageError::MigrationFailed {
            target_version: 17,
            migration: "replace_task_due_semantics",
            ..
        })
    ));
}
