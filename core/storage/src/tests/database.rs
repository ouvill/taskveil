use super::*;

#[test]
fn encrypted_database_reopens_with_correct_key() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        insert_task_pre_v20(&connection, &task);
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteTaskRepository::new(connection);
    assert_eq!(repository.get(task.id).unwrap(), task);
}

#[test]
fn encrypted_database_rejects_wrong_key_on_query() {
    let file = NamedTempFile::new().unwrap();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        repository.insert(sample_task()).unwrap();
    }

    let result = open_encrypted(file.path(), &WRONG_KEY);

    match result {
        Err(StorageError::InvalidDatabaseKey) => {}
        Err(error) => panic!("expected invalid database key error, got {error}"),
        Ok(_) => panic!("database opened with wrong key"),
    }
}

#[test]
fn sqlcipher_rekey_preserves_data_and_rejects_the_old_key() {
    let file = NamedTempFile::new().unwrap();
    let task = sample_task();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
    }

    rekey_encrypted_database(file.path(), &KEY, &WRONG_KEY).unwrap();

    assert!(matches!(
        open_encrypted(file.path(), &KEY),
        Err(StorageError::InvalidDatabaseKey)
    ));
    let repository = SqliteTaskRepository::new(open_encrypted(file.path(), &WRONG_KEY).unwrap());
    assert_eq!(repository.get(task.id).unwrap(), task);
}

#[test]
fn device_key_store_derived_key_reopens_database_and_rejects_other_device_key() {
    let file = NamedTempFile::new().unwrap();
    let mut store = InMemoryDeviceKeyStore::new();
    let task = sample_task();

    {
        let device_key = ensure_device_key(&mut store).unwrap();
        let db_key = derive_local_db_key(&device_key);
        let connection = open_encrypted(file.path(), &db_key).unwrap();
        insert_task_pre_v20(&connection, &task);
    }

    {
        let device_key = ensure_device_key(&mut store).unwrap();
        let db_key = derive_local_db_key(&device_key);
        let connection = open_encrypted(file.path(), &db_key).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        assert_eq!(repository.get(task.id).unwrap(), task);
    }

    let mut other_store = InMemoryDeviceKeyStore::new();
    let other_device_key = ensure_device_key(&mut other_store).unwrap();
    let other_db_key = derive_local_db_key(&other_device_key);

    assert!(open_encrypted(file.path(), &other_db_key).is_err());
}

#[test]
fn encrypted_database_is_not_plain_sqlite() {
    let file = NamedTempFile::new().unwrap();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        repository.insert(sample_task()).unwrap();
    }

    let plain = Connection::open(file.path()).unwrap();
    let result: rusqlite::Result<i64> =
        plain.query_row("SELECT count(*) FROM tasks", [], |row| row.get(0));

    assert!(result.is_err());
}

#[test]
fn fts5_search_matches_title_and_note() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let mut task = sample_task();
    task.content.title = "Plan Kyoto trip".to_string();
    task.content.note = "Book shinkansen tickets".to_string();

    repository.insert(task.clone()).unwrap();

    assert_eq!(
        repository.search_tasks("kyoto").unwrap(),
        vec![task.clone()]
    );
    assert_eq!(repository.search_tasks("shinkansen").unwrap(), vec![task]);
    assert!(repository.search_tasks("").unwrap().is_empty());
}

#[test]
fn fts5_search_tracks_title_note_updates_and_deleted_at() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let mut task = sample_task();
    task.content.title = "Draft itinerary".to_string();
    task.content.note = "Reserve hotel".to_string();
    repository.insert(task.clone()).unwrap();

    assert_eq!(
        repository.search_tasks("hotel").unwrap(),
        vec![task.clone()]
    );

    let mut updated = task.clone();
    updated.content.title = "Final packing list".to_string();
    updated.content.note = "Bring passport".to_string();
    updated.updated_at += 1;
    repository.update(updated.clone()).unwrap();

    assert!(repository.search_tasks("hotel").unwrap().is_empty());
    assert_eq!(
        repository.search_tasks("passport").unwrap(),
        vec![updated.clone()]
    );

    let mut deleted = updated.clone();
    deleted.deleted_at = Some(updated.updated_at + 1);
    deleted.updated_at += 1;
    repository.update(deleted.clone()).unwrap();

    assert!(repository.search_tasks("passport").unwrap().is_empty());

    let mut restored = deleted.clone();
    restored.deleted_at = None;
    restored.updated_at += 1;
    repository.update(restored.clone()).unwrap();

    assert_eq!(repository.search_tasks("passport").unwrap(), vec![restored]);
}

#[test]
fn fts5_search_tracks_task_delete_and_list_rehoming() {
    let file = NamedTempFile::new().unwrap();
    let list = sample_list("a0");
    let mut default_list = sample_list("a1");
    default_list.is_default = true;
    let mut kept = sample_task();
    kept.list_id = list.id;
    kept.content.title = "Keep searchable".to_string();
    kept.content.note = "retained".to_string();
    kept.sort_order = "a0".to_string();
    let mut task_deleted_by_subtree = sample_task();
    task_deleted_by_subtree.list_id = list.id;
    task_deleted_by_subtree.content.title = "Delete searchable subtree".to_string();
    task_deleted_by_subtree.content.note = "temporary".to_string();
    task_deleted_by_subtree.sort_order = "a1".to_string();
    let mut task_deleted_by_list = sample_task();
    task_deleted_by_list.list_id = list.id;
    task_deleted_by_list.content.title = "Delete searchable list".to_string();
    task_deleted_by_list.content.note = "temporary".to_string();
    task_deleted_by_list.sort_order = "a2".to_string();

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut list_repository = SqliteListRepository::new(connection);
        list_repository.insert(list.clone()).unwrap();
        list_repository.insert(default_list.clone()).unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut task_repository = SqliteTaskRepository::new(connection);
        task_repository.insert(kept.clone()).unwrap();
        task_repository
            .insert(task_deleted_by_subtree.clone())
            .unwrap();
        task_repository
            .insert(task_deleted_by_list.clone())
            .unwrap();
        assert_eq!(task_repository.search_tasks("searchable").unwrap().len(), 3);

        task_repository
            .delete_subtree(task_deleted_by_subtree.id)
            .unwrap();
        let titles = task_repository
            .search_tasks("searchable")
            .unwrap()
            .into_iter()
            .map(|task| task.content.title)
            .collect::<Vec<_>>();
        let mut titles = titles;
        titles.sort();
        assert_eq!(titles, vec!["Delete searchable list", "Keep searchable"]);
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut list_repository = SqliteListRepository::new(connection);
    list_repository.delete_and_rehome_tasks(list.id).unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let task_repository = SqliteTaskRepository::new(connection);
    let mut remaining = task_repository
        .search_tasks("searchable")
        .unwrap()
        .into_iter()
        .map(|task| (task.content.title, task.list_id))
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(
        remaining,
        vec![
            ("Delete searchable list".to_string(), default_list.id),
            ("Keep searchable".to_string(), default_list.id),
        ]
    );
}

#[test]
fn fts5_search_supports_english_and_japanese_prefix_queries() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTaskRepository::new(connection);
    let mut english = sample_task();
    english.content.title = "Buy milk".to_string();
    english.content.note = "Organic whole milk".to_string();
    english.updated_at = 1_799_000_000_000;
    let mut japanese = sample_task();
    japanese.content.title = "牛乳を買う".to_string();
    japanese.content.note = "明日の朝".to_string();
    japanese.updated_at = 1_799_000_001_000;
    repository.insert(english.clone()).unwrap();
    repository.insert(japanese.clone()).unwrap();

    assert_eq!(repository.search_tasks("milk").unwrap(), vec![english]);
    assert_eq!(
        repository.search_tasks("牛乳").unwrap(),
        vec![japanese.clone()]
    );
    assert_eq!(repository.search_tasks("明日").unwrap(), vec![japanese]);
    assert!(repository.search_tasks("乳").unwrap().is_empty());
}

#[test]
fn local_crypto_cache_roundtrips_tenant_root_and_rejects_rebinding() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let tenant_id = Uuid::now_v7();
    let other_tenant_id = Uuid::now_v7();
    let binding = LocalProfileBinding {
        tenant_id,
        user_id: Uuid::now_v7(),
        device_id: Uuid::now_v7(),
        bound_at: 10,
        updated_at: 10,
    };
    let mut repository = SqliteLocalCryptoRepository::new(connection);
    repository
        .bind_tenant_root(binding.clone(), &local_tenant_root_bundle(tenant_id, 10))
        .unwrap();

    assert_eq!(repository.load_binding().unwrap(), Some(binding));
    assert_eq!(
        repository.load_tenant_root(tenant_id).unwrap(),
        Some(local_tenant_root_bundle(tenant_id, 10))
    );
    assert!(matches!(
        repository.load_tenant_root(other_tenant_id),
        Err(StorageError::LocalProfileTenantMismatch { .. })
    ));
}

#[test]
fn timer_v18_repository_restores_singleton_and_rejects_immutable_conflicts() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
    let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
    let task_id = task.id;
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteListRepository::new(connection).insert(list).unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        SqliteTaskRepository::new(connection).insert(task).unwrap();
    }
    let active = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task_id),
        mode: TimerMode::Stopwatch,
        phase: TimerPhase::Work,
        state: TimerRunState::Running,
        started_at: 1_000,
        last_resumed_at: Some(1_100),
        accumulated_active_ms: 500,
        target_duration_ms: None,
    };
    let completed = CompletedTimerSession {
        id: Uuid::now_v7(),
        task_id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: 1_000,
        ended_at: 5_000,
        active_duration_ms: 3_000,
        created_at: 5_100,
    };

    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            latest_migration_version(&connection),
            LATEST_MIGRATION_VERSION
        );
        let mut repository = SqliteTimerSessionRepository::new(connection);
        repository.start_active(active.clone(), 1_200).unwrap();
        assert!(repository.insert_completed(completed.clone()).unwrap());
        assert!(!repository.insert_completed(completed.clone()).unwrap());
        let mut conflicting = completed.clone();
        conflicting.active_duration_ms = 2_000;
        assert!(matches!(
            repository.insert_completed(conflicting),
            Err(StorageError::IncompatibleSchema(_))
        ));
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteTimerSessionRepository::new(connection);
    assert_eq!(repository.load_active().unwrap(), Some(active.clone()));
    assert_eq!(repository.get_completed(completed.id).unwrap(), completed);
    let paused = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task_id),
        mode: TimerMode::Pomodoro,
        phase: TimerPhase::Work,
        state: TimerRunState::Paused,
        started_at: 10_000,
        last_resumed_at: None,
        accumulated_active_ms: 2_000,
        target_duration_ms: Some(25 * 60 * 1_000),
    };
    assert!(repository.clear_active(active.session_id).unwrap());
    repository.start_active(paused.clone(), 12_000).unwrap();
    assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));
    assert!(!repository.clear_active(Uuid::now_v7()).unwrap());
    assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));
    assert!(repository.clear_active(paused.session_id).unwrap());
    assert_eq!(repository.load_active().unwrap(), None);

    assert!(repository
        .connection()
        .execute(
            "INSERT INTO active_timer_session (
                     singleton, session_id, task_id, mode, phase, state, started_at,
                     last_resumed_at, accumulated_active_ms, updated_at
                 ) VALUES (1, ?1, NULL, 'stopwatch', 'short_break', 'paused', 1, NULL, 0, 1)",
            [Uuid::now_v7().to_string()],
        )
        .is_err());
    let orphan_task_id = Uuid::now_v7();
    let orphan = CompletedTimerSession {
        id: Uuid::now_v7(),
        task_id: orphan_task_id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: 1,
        ended_at: 10,
        active_duration_ms: 5,
        created_at: 11,
    };
    assert!(matches!(
        repository.insert_completed(orphan),
        Err(StorageError::NotFound(id)) if id == orphan_task_id
    ));
    assert!(repository
        .connection()
        .execute(
            "INSERT INTO timer_sessions (
                     id, task_id, mode, finish_kind, started_at, ended_at,
                     active_duration_ms, created_at
                 ) VALUES (?1, ?2, 'stopwatch', 'completed', 1, 10, 5, 9)",
            params![Uuid::now_v7().to_string(), task_id.to_string()],
        )
        .is_err());
}

#[test]
fn finish_active_timer_rejects_start_mismatch_and_accepts_exact_retry() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
    let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
    let task_id = task.id;
    SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(list)
        .unwrap();
    SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(task)
        .unwrap();
    let active = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task_id),
        mode: TimerMode::Stopwatch,
        phase: TimerPhase::Work,
        state: TimerRunState::Paused,
        started_at: 2_000,
        last_resumed_at: None,
        accumulated_active_ms: 500,
        target_duration_ms: None,
    };
    SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .start_active(active.clone(), 3_000)
        .unwrap();
    let completed = CompletedTimerSession {
        id: active.session_id,
        task_id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: active.started_at,
        ended_at: 4_000,
        active_duration_ms: 500,
        created_at: 4_000,
    };

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    let mut mismatched = completed.clone();
    mismatched.started_at += 1;
    assert!(matches!(
        transaction.finish_active_timer_session(mismatched),
        Err(StorageError::IncompatibleSchema(_))
    ));
    drop(transaction);
    assert_eq!(
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .load_active()
            .unwrap(),
        Some(active.clone())
    );

    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    let mut duration_mismatch = completed.clone();
    duration_mismatch.active_duration_ms -= 1;
    assert!(matches!(
        transaction.finish_active_timer_session(duration_mismatch),
        Err(StorageError::CompletedTimerDurationMismatch {
            expected_ms: 500,
            actual_ms: 499,
        })
    ));
    drop(transaction);
    assert_eq!(
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .load_active()
            .unwrap(),
        Some(active.clone())
    );

    let mut repository =
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert!(repository.insert_completed(completed.clone()).unwrap());
    drop(repository);
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    assert!(!transaction
        .finish_active_timer_session(completed.clone())
        .unwrap());
    transaction.commit().unwrap();
    let repository = SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    assert_eq!(repository.load_active().unwrap(), None);
    assert_eq!(repository.get_completed(completed.id).unwrap(), completed);
}

#[test]
fn finish_running_timer_requires_restored_duration_and_accepts_pomodoro_target_instant() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
    let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
    SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(list)
        .unwrap();
    SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(task.clone())
        .unwrap();
    let running = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task.id),
        mode: TimerMode::Stopwatch,
        phase: TimerPhase::Work,
        state: TimerRunState::Running,
        started_at: 1_000,
        last_resumed_at: Some(2_000),
        accumulated_active_ms: 500,
        target_duration_ms: None,
    };
    SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .start_active(running.clone(), 2_000)
        .unwrap();
    let completed = CompletedTimerSession {
        id: running.session_id,
        task_id: task.id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: running.started_at,
        ended_at: 3_000,
        active_duration_ms: 1_500,
        created_at: 3_000,
    };
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    let mut mismatch = completed.clone();
    mismatch.active_duration_ms = 1_499;
    assert!(matches!(
        transaction.finish_active_timer_session(mismatch),
        Err(StorageError::CompletedTimerDurationMismatch { .. })
    ));
    drop(transaction);
    assert_eq!(
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
            .load_active()
            .unwrap(),
        Some(running)
    );
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    assert!(transaction.finish_active_timer_session(completed).unwrap());
    transaction.commit().unwrap();

    let pomodoro = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task.id),
        mode: TimerMode::Pomodoro,
        phase: TimerPhase::Work,
        state: TimerRunState::Running,
        started_at: 10_000,
        last_resumed_at: Some(15_000),
        accumulated_active_ms: 5_000,
        target_duration_ms: Some(25_000),
    };
    let reached_at = taskveil_domain::pomodoro_target_reached_at(&pomodoro).unwrap();
    assert_eq!(reached_at, 35_000);
    SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .start_active(pomodoro.clone(), 15_000)
        .unwrap();
    let completed = CompletedTimerSession {
        id: pomodoro.session_id,
        task_id: task.id,
        mode: TimerMode::Pomodoro,
        finish_kind: TimerFinishKind::Completed,
        started_at: pomodoro.started_at,
        ended_at: reached_at,
        active_duration_ms: 25_000,
        created_at: 40_000,
    };
    let mut connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut transaction = SqliteWriteTx::begin(&mut connection).unwrap();
    assert!(transaction.finish_active_timer_session(completed).unwrap());
    transaction.commit().unwrap();
}

#[test]
fn active_timer_start_conflicts_and_update_preserves_session_contract() {
    let file = NamedTempFile::new().unwrap();
    let list = new_list("Timer".into(), "a0".into(), 1_000).unwrap();
    let task = new_task(list.id, None, "Focus".into(), "a0".into(), 1_000).unwrap();
    SqliteListRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(list)
        .unwrap();
    SqliteTaskRepository::new(open_encrypted(file.path(), &KEY).unwrap())
        .insert(task.clone())
        .unwrap();
    let running = ActiveTimerSession {
        session_id: Uuid::now_v7(),
        task_id: Some(task.id),
        mode: TimerMode::Pomodoro,
        phase: TimerPhase::Work,
        state: TimerRunState::Running,
        started_at: 2_000,
        last_resumed_at: Some(2_000),
        accumulated_active_ms: 0,
        target_duration_ms: Some(25_000),
    };
    let mut repository =
        SqliteTimerSessionRepository::new(open_encrypted(file.path(), &KEY).unwrap());
    repository.start_active(running.clone(), 2_000).unwrap();

    let mut competing = running.clone();
    competing.session_id = Uuid::now_v7();
    assert!(matches!(
        repository.start_active(competing, 2_001),
        Err(StorageError::ActiveTimerConflict(id)) if id == running.session_id
    ));
    assert_eq!(repository.load_active().unwrap(), Some(running.clone()));

    let mut changed_identity = running.clone();
    changed_identity.session_id = Uuid::now_v7();
    assert!(matches!(
        repository.update_active(changed_identity, 2_002),
        Err(StorageError::InvalidActiveTimerUpdate(
            DomainError::ActiveTimerIdentityChanged
        ))
    ));

    let mut paused = running.clone();
    paused.state = TimerRunState::Paused;
    paused.last_resumed_at = None;
    paused.accumulated_active_ms = 500;
    repository.update_active(paused.clone(), 2_500).unwrap();
    assert_eq!(repository.load_active().unwrap(), Some(paused.clone()));

    let mut injected = paused.clone();
    injected.accumulated_active_ms += 1;
    assert!(matches!(
        repository.update_active(injected, 2_501),
        Err(StorageError::InvalidActiveTimerUpdate(
            DomainError::ActiveTimerProgressChangedOutsidePause
        ))
    ));

    let mut reduced_target = paused;
    reduced_target.target_duration_ms = Some(20_000);
    assert!(matches!(
        repository.update_active(reduced_target, 2_502),
        Err(StorageError::InvalidActiveTimerUpdate(
            DomainError::ActiveTimerTargetRegressed
        ))
    ));
}

#[test]
#[ignore = "task-67 manual performance verification for a 10k encrypted seed"]
fn task_67_reports_10000_task_storage_timings() {
    let file = NamedTempFile::new().unwrap();
    let seed = seed_performance_database(file.path(), &KEY, PerformanceSeedSchema::Latest);
    assert_eq!(seed.list_ids.len(), 10);
    assert_eq!(seed.task_count, 10_000);

    let mut rows: Vec<(&str, usize, u128, String)> = Vec::new();

    let started = std::time::Instant::now();
    let home_tasks = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        repository
            .list_home(seed.today_start_ms, seed.tomorrow_start_ms)
            .unwrap()
    };
    rows.push((
        "get_today_tasks(list_home)",
        home_tasks.len(),
        started.elapsed().as_millis(),
        "cross-list Home query on encrypted DB".to_string(),
    ));

    let started = std::time::Instant::now();
    let list_tasks = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        repository.list_active_by_list(seed.list_ids[0]).unwrap()
    };
    rows.push((
        "get_tasks(list 1)",
        list_tasks.len(),
        started.elapsed().as_millis(),
        "single list, 1000 tasks".to_string(),
    ));

    let started = std::time::Instant::now();
    let search_results = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        repository.search_tasks("alpha").unwrap()
    };
    rows.push((
        "search_tasks(alpha)",
        search_results.len(),
        started.elapsed().as_millis(),
        "FTS5 prefix query".to_string(),
    ));

    let started = std::time::Instant::now();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteListRepository::new(connection);
        repository
            .ensure_default_list("Inbox".to_string(), seed.today_start_ms)
            .unwrap();
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteListRepository::new(connection);
        assert_eq!(repository.list_all().unwrap().len(), 10);
    }
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteListRepository::new(connection);
        assert!(repository.list_archived().unwrap().is_empty());
    }
    let startup_home_count = {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let repository = SqliteTaskRepository::new(connection);
        repository
            .list_home(seed.today_start_ms, seed.tomorrow_start_ms)
            .unwrap()
            .len()
    };
    let startup_elapsed_ms = started.elapsed().as_millis();
    rows.push((
        "startup_approx(init+initial_queries)",
        startup_home_count,
        startup_elapsed_ms,
        "open+default list+lists+archived+Home".to_string(),
    ));

    println!(
        "task-67 Rust performance seed: lists=10 tasks={} due={} closed={}",
        seed.task_count, seed.due_task_count, seed.closed_task_count
    );
    println!("| operation | rows | elapsed_ms | note |");
    println!("|---|---:|---:|---|");
    for (operation, count, elapsed_ms, note) in &rows {
        println!("| {operation} | {count} | {elapsed_ms} | {note} |");
    }

    assert_eq!(list_tasks.len(), 1_000);
    assert!(!home_tasks.is_empty());
    assert!(!search_results.is_empty());
    assert!(
        startup_elapsed_ms < 2_000,
        "startup approximation exceeded F-50: {startup_elapsed_ms} ms"
    );
}

#[test]
fn fts5_search_works_after_reopening_encrypted_database() {
    let file = NamedTempFile::new().unwrap();
    let mut task = sample_task();
    task.content.title = "Encrypted search".to_string();
    task.content.note = "SQLCipher FTS5".to_string();
    {
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let mut repository = SqliteTaskRepository::new(connection);
        repository.insert(task.clone()).unwrap();
    }

    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteTaskRepository::new(connection);

    assert_eq!(repository.search_tasks("sqlcipher").unwrap(), vec![task]);
}

#[test]
fn new_database_is_created_from_initial_migration() {
    let file = NamedTempFile::new().unwrap();

    let connection = open_encrypted(file.path(), &KEY).unwrap();

    assert_eq!(
        latest_migration_version(&connection),
        LATEST_MIGRATION_VERSION
    );
    assert_eq!(
        archived_at_column(&connection),
        Some(("INTEGER".to_string(), 0))
    );
    assert_eq!(
        is_default_column(&connection),
        Some(("INTEGER".to_string(), 1, "0".to_string()))
    );
    assert_eq!(
        setting_column(&connection, "key"),
        Some(("TEXT".to_string(), 0))
    );
    assert_eq!(
        setting_column(&connection, "value"),
        Some(("TEXT".to_string(), 1))
    );
    assert_eq!(
        setting_column(&connection, "updated_at"),
        Some(("INTEGER".to_string(), 1))
    );
    assert_eq!(
        reminder_column(&connection, "id"),
        Some(("TEXT".to_string(), 1))
    );
    assert_eq!(
        reminder_column(&connection, "task_id"),
        Some(("TEXT".to_string(), 1))
    );
    assert_eq!(
        reminder_column(&connection, "remind_at"),
        Some(("INTEGER".to_string(), 1))
    );
    assert_eq!(
        reminder_column(&connection, "snoozed_until"),
        Some(("INTEGER".to_string(), 0))
    );
    assert_eq!(
        sync_outbox_column(&connection, "created_at"),
        Some(("INTEGER".to_string(), 1))
    );
    assert_eq!(
        sync_cursor_column(&connection, "updated_at"),
        Some(("INTEGER".to_string(), 1))
    );
}

#[test]
fn sqlite_settings_repository_returns_none_for_missing_key() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let repository = SqliteSettingsRepository::new(connection);

    assert_eq!(repository.get_setting("ui_mode").unwrap(), None);
}

#[test]
fn sqlite_settings_repository_roundtrips_setting() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSettingsRepository::new(connection);

    repository
        .set_setting("ui_mode", "simple", 1_799_000_000_000)
        .unwrap();

    assert_eq!(
        repository.get_setting("ui_mode").unwrap(),
        Some("simple".to_string())
    );
}

#[test]
fn sqlite_settings_repository_overwrites_existing_setting() {
    let file = NamedTempFile::new().unwrap();
    let connection = open_encrypted(file.path(), &KEY).unwrap();
    let mut repository = SqliteSettingsRepository::new(connection);

    repository
        .set_setting("ui_mode", "simple", 1_799_000_000_000)
        .unwrap();
    repository
        .set_setting("ui_mode", "advanced", 1_799_000_001_000)
        .unwrap();

    assert_eq!(
        repository.get_setting("ui_mode").unwrap(),
        Some("advanced".to_string())
    );
    let updated_at: i64 = repository
        .connection()
        .query_row(
            "SELECT updated_at FROM settings WHERE key = ?1",
            ["ui_mode"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(updated_at, 1_799_000_001_000);
}
