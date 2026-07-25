use super::*;

use std::collections::HashMap;

use taskveil_crypto::key_hierarchy::KEY_LEN;
use taskveil_domain::{
    new_task, CompletedTimerSession, SeriesCursor, TaskBlueprint, TaskBlueprintNode, TaskContent,
    TaskSeriesConfig, TimerFinishKind, TimerMode, TASK_BLUEPRINT_SCHEMA_REVISION,
};
use zeroize::Zeroizing;

use crate::{
    LocalMutationSyncStore, LocalSyncOutboxEntry, NewLocalSyncOutboxEntry, TimerSessionPlaintext,
    TIMER_SESSIONS_COLLECTION,
};

fn test_tenant_id() -> Uuid {
    Uuid::from_u128(100)
}

fn encrypt_plaintext(
    dek: &[u8; 32],
    collection: &str,
    record_id: &str,
    plaintext: &SyncPlaintext,
) -> Result<Vec<u8>, EnvelopeError> {
    crate::envelope::encrypt_plaintext(
        dek,
        test_tenant_id(),
        1,
        collection,
        Uuid::parse_str(record_id).map_err(|_| EnvelopeError::InvalidIdentity)?,
        plaintext,
    )
}

fn decrypt_plaintext(
    dek: &[u8; 32],
    collection: &str,
    record_id: &str,
    blob: &[u8],
) -> Result<SyncPlaintext, EnvelopeError> {
    crate::envelope::decrypt_plaintext(
        dek,
        test_tenant_id(),
        1,
        collection,
        Uuid::parse_str(record_id).map_err(|_| EnvelopeError::InvalidIdentity)?,
        blob,
    )
}

#[derive(Default)]
struct FakeStore {
    lists: HashMap<Uuid, List>,
    tasks: HashMap<Uuid, Task>,
    templates: HashMap<Uuid, TaskTemplate>,
    series: HashMap<Uuid, TaskSeries>,
    timer_sessions: HashMap<Uuid, CompletedTimerSession>,
    active_timer_task: Option<Uuid>,
    record_states: HashMap<(SyncCollection, Uuid), LocalSyncRecordState>,
    outbox: Vec<LocalSyncOutboxEntry>,
    aliases: Vec<LocalListAlias>,
    live_list_quarantine: bool,
    settings: HashMap<String, String>,
}

impl LocalMutationSyncStore for FakeStore {
    fn has_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        Ok(self
            .outbox
            .iter()
            .any(|entry| entry.collection == collection && entry.record_id == record_id))
    }

    fn get_setting(&mut self, key: &str) -> Result<Option<String>, String> {
        Ok(self.settings.get(key).cloned())
    }

    fn set_setting(&mut self, key: &str, value: &str, _updated_at: i64) -> Result<(), String> {
        self.settings.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn put_outbox_head(&mut self, entry: NewLocalSyncOutboxEntry) -> Result<(), String> {
        self.outbox.retain(|head| head.record_id != entry.record_id);
        self.outbox.push(LocalSyncOutboxEntry {
            op_id: entry.op_id,
            record_id: entry.record_id,
            collection: entry.collection,
            base_revision_hlc: entry.base_revision_hlc,
            revision_hlc: entry.revision_hlc,
            state: entry.state,
            created_at: entry.created_at,
        });
        Ok(())
    }

    fn get_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<Option<LocalSyncRecordState>, String> {
        Ok(self.record_states.get(&(collection, record_id)).cloned())
    }

    fn put_record_state(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
        state: LocalSyncRecordState,
        _updated_at: i64,
    ) -> Result<(), String> {
        self.record_states.insert((collection, record_id), state);
        Ok(())
    }
}

impl LocalSyncStore for FakeStore {
    fn list_outbox_heads(&mut self, limit: usize) -> Result<Vec<LocalSyncOutboxEntry>, String> {
        Ok(self.outbox.iter().take(limit).cloned().collect())
    }

    fn ack_outbox_op(&mut self, op_id: Uuid) -> Result<bool, String> {
        let previous_len = self.outbox.len();
        self.outbox.retain(|entry| entry.op_id != op_id);
        Ok(previous_len != self.outbox.len())
    }

    fn delete_outbox_head(
        &mut self,
        collection: SyncCollection,
        record_id: Uuid,
    ) -> Result<bool, String> {
        let previous_len = self.outbox.len();
        self.outbox
            .retain(|entry| entry.collection != collection || entry.record_id != record_id);
        Ok(previous_len != self.outbox.len())
    }

    fn get_cursor_seq(&mut self, _name: &str) -> Result<Option<i64>, String> {
        Ok(None)
    }

    fn set_cursor(&mut self, _name: &str, _seq: i64, _updated_at: i64) -> Result<(), String> {
        Ok(())
    }

    fn delete_cursor(&mut self, _name: &str) -> Result<(), String> {
        Ok(())
    }

    fn list_record_states(
        &mut self,
        collection: SyncCollection,
    ) -> Result<Vec<(Uuid, LocalSyncRecordState)>, String> {
        Ok(self
            .record_states
            .iter()
            .filter(|((stored_collection, _), _)| *stored_collection == collection)
            .map(|((_, record_id), state)| (*record_id, state.clone()))
            .collect())
    }

    fn has_live_quarantine(&mut self, collection: SyncCollection) -> Result<bool, String> {
        Ok(collection == SyncCollection::Lists && self.live_list_quarantine)
    }

    fn list_list_aliases(&mut self) -> Result<Vec<LocalListAlias>, String> {
        Ok(self.aliases.clone())
    }

    fn replace_list_aliases(
        &mut self,
        aliases: &[LocalListAlias],
        _updated_at: i64,
    ) -> Result<(), String> {
        self.aliases = aliases.to_vec();
        Ok(())
    }

    fn resolve_list_alias(&mut self, list_id: Uuid) -> Result<Uuid, String> {
        Ok(self
            .aliases
            .iter()
            .find(|alias| alias.alias_list_id == list_id)
            .map_or(list_id, |alias| alias.canonical_list_id))
    }

    fn materialize_canonical_list(&mut self, canonical_list_id: Uuid) -> Result<(), String> {
        if !self.lists.contains_key(&canonical_list_id) {
            return Err("canonical list is missing".to_string());
        }
        for list in self.lists.values_mut() {
            list.is_default = list.id == canonical_list_id;
        }
        Ok(())
    }

    fn default_list_id(&mut self) -> Result<Option<Uuid>, String> {
        Ok(self
            .lists
            .values()
            .find(|list| list.is_default)
            .map(|list| list.id))
    }

    fn get_list(&mut self, id: Uuid) -> Result<Option<List>, String> {
        Ok(self.lists.get(&id).cloned())
    }

    fn upsert_list_for_sync(&mut self, list: List) -> Result<(), String> {
        if list.is_default
            && self
                .lists
                .values()
                .any(|existing| existing.is_default && existing.id != list.id)
        {
            return Err("default list conflict".to_string());
        }
        self.lists.insert(list.id, list);
        Ok(())
    }

    fn delete_list_and_rehome_tasks_for_sync(&mut self, list_id: Uuid) -> Result<usize, String> {
        let default_list_id = self
            .default_list_id()?
            .filter(|default_id| *default_id != list_id)
            .ok_or_else(|| "Tenant must retain a default Inbox".to_string())?;
        self.lists.remove(&list_id);
        let mut rehomed = 0;
        for task in self.tasks.values_mut() {
            if task.list_id == list_id {
                task.list_id = default_list_id;
                rehomed += 1;
            }
        }
        Ok(rehomed)
    }

    fn get_task(&mut self, id: Uuid) -> Result<Option<Task>, String> {
        Ok(self.tasks.get(&id).cloned())
    }

    fn list_tasks_by_list_for_sync(&mut self, list_id: Uuid) -> Result<Vec<Task>, String> {
        Ok(self
            .tasks
            .values()
            .filter(|task| task.list_id == list_id)
            .cloned()
            .collect())
    }

    fn list_all_tasks_for_sync(&mut self) -> Result<Vec<Task>, String> {
        Ok(self.tasks.values().cloned().collect())
    }

    fn upsert_task_for_sync(&mut self, task: Task) -> Result<(), String> {
        self.tasks.insert(task.id, task);
        Ok(())
    }

    fn delete_task_subtree_for_sync(&mut self, _task_id: Uuid) -> Result<usize, String> {
        Ok(usize::from(self.tasks.remove(&_task_id).is_some()))
    }

    fn get_template(&mut self, id: Uuid) -> Result<Option<TaskTemplate>, String> {
        Ok(self.templates.get(&id).cloned())
    }

    fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), String> {
        self.templates.insert(template.id, template);
        Ok(())
    }

    fn delete_template_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        Ok(self.templates.remove(&id).is_some())
    }

    fn get_series(&mut self, id: Uuid) -> Result<Option<TaskSeries>, String> {
        Ok(self.series.get(&id).cloned())
    }

    fn upsert_series_for_sync(&mut self, series: TaskSeries) -> Result<(), String> {
        self.series.insert(series.id, series);
        Ok(())
    }

    fn delete_series_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        Ok(self.series.remove(&id).is_some())
    }

    fn get_timer_session(&mut self, id: Uuid) -> Result<Option<CompletedTimerSession>, String> {
        Ok(self.timer_sessions.get(&id).cloned())
    }

    fn upsert_timer_session_for_sync(
        &mut self,
        session: CompletedTimerSession,
    ) -> Result<(), String> {
        self.timer_sessions.insert(session.id, session);
        Ok(())
    }

    fn delete_timer_session_for_sync(&mut self, id: Uuid) -> Result<bool, String> {
        Ok(self.timer_sessions.remove(&id).is_some())
    }

    fn list_timer_sessions_by_task(
        &mut self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, String> {
        Ok(self
            .timer_sessions
            .values()
            .filter(|session| session.task_id == task_id)
            .cloned()
            .collect())
    }

    fn clear_active_timer_for_task(&mut self, task_id: Uuid) -> Result<bool, String> {
        let matched = self.active_timer_task == Some(task_id);
        if matched {
            self.active_timer_task = None;
        }
        Ok(matched)
    }
}

#[test]
fn remote_task_tombstone_cascades_timer_with_same_delete_hlc_and_clears_active() {
    let list_id = uuid(90);
    let task_id = uuid(91);
    let session_id = uuid(92);
    let task = new_task(
        list_id,
        None,
        "timed".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1_799_000_000_000,
    )
    .unwrap();
    let mut task = task;
    task.id = task_id;
    let session = CompletedTimerSession {
        id: session_id,
        task_id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: 1_799_000_000_000,
        ended_at: 1_799_000_060_000,
        active_duration_ms: 60_000,
        created_at: 1_799_000_060_000,
    };
    let delete_hlc = Hlc {
        wall_ms: 1_799_000_070_000,
        counter: 0,
        device_id: "remote".to_string(),
    }
    .encode()
    .unwrap();
    let record = PullRecord {
        seq: 1,
        record_id: task_id,
        collection: SyncCollection::Tasks,
        revision_hlc: delete_hlc.clone(),
        state: EncryptedSyncState::Tombstone {
            delete_hlc: delete_hlc.clone(),
        },
    };
    let mut store = FakeStore::default();
    store.lists.insert(list_id, sample_list(list_id, false));
    store.tasks.insert(task_id, task);
    store.timer_sessions.insert(session_id, session);
    store.active_timer_task = Some(task_id);
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_task(
        &record,
        &context_for(list_id, [9; KEY_LEN]),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert!(!store.timer_sessions.contains_key(&session_id));
    assert_eq!(store.active_timer_task, None);
    let timer_tombstone = store
        .outbox
        .iter()
        .find(|entry| entry.collection == SyncCollection::TimerSessions)
        .unwrap();
    assert_eq!(timer_tombstone.record_id, session_id);
    assert_eq!(
        timer_tombstone.state,
        EncryptedSyncState::Tombstone { delete_hlc }
    );
}

#[test]
fn late_timer_uses_terminal_task_delete_hlc() {
    let list_id = uuid(93);
    let task_id = uuid(94);
    let session_id = uuid(95);
    let root = [0x95; KEY_LEN];
    let session = CompletedTimerSession {
        id: session_id,
        task_id,
        mode: TimerMode::Stopwatch,
        finish_kind: TimerFinishKind::Completed,
        started_at: 1_799_000_000_000,
        ended_at: 1_799_000_030_000,
        active_duration_ms: 30_000,
        created_at: 1_799_000_030_000,
    };
    let session_hlc = Hlc {
        wall_ms: 1_799_000_050_000,
        counter: 0,
        device_id: "remote".into(),
    };
    let task_delete_hlc = Hlc {
        wall_ms: 1_799_000_040_000,
        counter: 0,
        device_id: "deleting-device".into(),
    }
    .encode()
    .unwrap();
    let plaintext = SyncPlaintext::TimerSession(TimerSessionPlaintext {
        value: session,
        hlc: session_hlc.clone(),
    });
    let record = PullRecord {
        record_id: session_id,
        collection: SyncCollection::TimerSessions,
        seq: 1,
        revision_hlc: session_hlc.encode().unwrap(),
        state: EncryptedSyncState::Live {
            mutation_hlc: session_hlc.encode().unwrap(),
            blob: encrypt_plaintext(
                &root,
                TIMER_SESSIONS_COLLECTION,
                &session_id.to_string(),
                &plaintext,
            )
            .unwrap(),
        },
    };
    let mut context = context_for(list_id, [0x93; KEY_LEN]);
    context.keys.tenant_root_dek = Some(Zeroizing::new(root));
    let mut store = FakeStore::default();
    store.record_states.insert(
        (SyncCollection::Tasks, task_id),
        LocalSyncRecordState {
            current_revision_hlc: Some(task_delete_hlc.clone()),
            state: LocalSyncSemanticState::Tombstone {
                delete_hlc: task_delete_hlc.clone(),
            },
        },
    );
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    assert_eq!(
        apply_pull_timer_session(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
        ApplyDisposition::Rebased
    );
    let tombstone = store.outbox.first().unwrap();
    assert_eq!(
        tombstone.state,
        EncryptedSyncState::Tombstone {
            delete_hlc: task_delete_hlc
        }
    );
}

#[test]
fn pull_default_list_with_existing_different_default_demotes_local_row_only() {
    let local_default = sample_list(uuid(1), true);
    let incoming_list = sample_list(uuid(2), true);
    let dek = [0x7a; KEY_LEN];
    let record = encrypted_list_record(&incoming_list, &dek);
    let context = context_for(incoming_list.id, dek);
    let mut store = FakeStore::default();
    store.lists.insert(local_default.id, local_default);
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

    assert!(!store.lists.get(&incoming_list.id).unwrap().is_default);
    let stored_plaintext =
        stored_sync_plaintext(&mut store, SyncCollection::Lists, incoming_list.id).unwrap();
    let SyncPlaintext::List(stored) = stored_plaintext.unwrap() else {
        panic!("list");
    };
    assert!(stored.is_default.value);
    assert_eq!(store.outbox.len(), 0);
    assert_eq!(summary.applied_count, 1);
    assert_eq!(summary.repush_count, 0);
}

#[test]
fn pull_default_list_without_existing_default_keeps_default_flag() {
    let incoming_list = sample_list(uuid(3), true);
    let dek = [0x3b; KEY_LEN];
    let record = encrypted_list_record(&incoming_list, &dek);
    let context = context_for(incoming_list.id, dek);
    let mut store = FakeStore::default();
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

    assert!(store.lists.get(&incoming_list.id).unwrap().is_default);
    assert_eq!(summary.applied_count, 1);
    assert_eq!(summary.repush_count, 0);
}

#[test]
fn pull_smaller_default_candidate_materializes_deterministically() {
    let existing = sample_list(uuid(20), true);
    let incoming = sample_list(uuid(10), true);
    let dek = [0x3c; KEY_LEN];
    let record = encrypted_list_record(&incoming, &dek);
    let context = context_for(incoming.id, dek);
    let mut store = FakeStore::default();
    store.lists.insert(existing.id, existing.clone());
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

    assert!(store.lists.get(&incoming.id).unwrap().is_default);
    assert!(!store.lists.get(&existing.id).unwrap().is_default);
    let stored = stored_sync_plaintext(&mut store, SyncCollection::Lists, incoming.id)
        .unwrap()
        .unwrap();
    let SyncPlaintext::List(stored) = stored else {
        panic!("list");
    };
    assert!(stored.is_default.value);
}

#[test]
fn canonical_reconcile_moves_tasks_reencrypts_and_is_idempotent() {
    let canonical = sample_list(uuid(1), true);
    let loser = sample_list(uuid(2), true);
    let mut local_canonical = canonical.clone();
    local_canonical.is_default = false;
    let canonical_dek = [0x11; KEY_LEN];
    let loser_dek = [0x22; KEY_LEN];
    let mut task = new_task(
        loser.id,
        None,
        "alias task".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1_799_000_000_000,
    )
    .unwrap();
    task.id = uuid(30);
    let mut store = FakeStore::default();
    store.lists.insert(canonical.id, local_canonical);
    store.lists.insert(loser.id, loser.clone());
    store.tasks.insert(task.id, task.clone());
    put_live_plaintext_state(
        &mut store,
        SyncCollection::Lists,
        canonical.id,
        list_plaintext(&canonical, test_hlc(1, "canonical")),
    );
    put_live_plaintext_state(
        &mut store,
        SyncCollection::Lists,
        loser.id,
        list_plaintext(&loser, test_hlc(1, "loser")),
    );
    put_live_plaintext_state(
        &mut store,
        SyncCollection::Tasks,
        task.id,
        task_plaintext(&task, test_hlc(1, "task")),
    );
    let context = context_with_keys(&[(canonical.id, canonical_dek), (loser.id, loser_dek)]);
    let mut now = ticking_now();

    reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

    assert!(store.lists.get(&canonical.id).unwrap().is_default);
    assert!(!store.lists.get(&loser.id).unwrap().is_default);
    assert_eq!(store.tasks.get(&task.id).unwrap().list_id, canonical.id);
    assert_eq!(
        store.aliases,
        vec![LocalListAlias {
            alias_list_id: loser.id,
            canonical_list_id: canonical.id,
        }]
    );
    assert_eq!(store.outbox.len(), 1);
    let encrypted = &store.outbox[0].state;
    let EncryptedSyncState::Live { blob, .. } = encrypted else {
        panic!("live");
    };
    let plaintext =
        decrypt_plaintext(&canonical_dek, TASKS_COLLECTION, &task.id.to_string(), blob).unwrap();
    let SyncPlaintext::Task(plaintext) = plaintext else {
        panic!("task");
    };
    assert_eq!(plaintext.placement.value.list_id, canonical.id);
    assert!(decrypt_plaintext(&loser_dek, TASKS_COLLECTION, &task.id.to_string(), blob).is_err());
    let op_id = store.outbox[0].op_id;

    reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();
    assert_eq!(store.outbox.len(), 1);
    assert_eq!(store.outbox[0].op_id, op_id);
}

#[test]
fn later_smaller_candidate_flattens_existing_aliases() {
    let first = sample_list(uuid(20), true);
    let old_loser = sample_list(uuid(30), true);
    let later = sample_list(uuid(10), true);
    let mut store = FakeStore::default();
    store.lists.insert(first.id, first.clone());
    let mut old_loser_domain = old_loser.clone();
    old_loser_domain.is_default = false;
    store.lists.insert(old_loser.id, old_loser_domain);
    let mut later_domain = later.clone();
    later_domain.is_default = false;
    store.lists.insert(later.id, later_domain);
    for list in [&first, &old_loser, &later] {
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Lists,
            list.id,
            list_plaintext(list, test_hlc(1, "remote")),
        );
    }
    store.aliases = vec![LocalListAlias {
        alias_list_id: old_loser.id,
        canonical_list_id: first.id,
    }];
    let context = context_with_keys(&[
        (first.id, [0x20; KEY_LEN]),
        (old_loser.id, [0x30; KEY_LEN]),
        (later.id, [0x10; KEY_LEN]),
    ]);
    let mut now = ticking_now();

    reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

    assert!(store.lists.get(&later.id).unwrap().is_default);
    assert_eq!(
        store.aliases,
        vec![
            LocalListAlias {
                alias_list_id: first.id,
                canonical_list_id: later.id,
            },
            LocalListAlias {
                alias_list_id: old_loser.id,
                canonical_list_id: later.id,
            },
        ]
    );
}

#[test]
fn live_list_quarantine_defers_election_without_writes() {
    let first = sample_list(uuid(1), true);
    let second = sample_list(uuid(2), true);
    let mut store = FakeStore::default();
    store.lists.insert(first.id, first.clone());
    let mut second_domain = second.clone();
    second_domain.is_default = false;
    store.lists.insert(second.id, second_domain);
    for list in [&first, &second] {
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Lists,
            list.id,
            list_plaintext(list, test_hlc(1, "remote")),
        );
    }
    store.live_list_quarantine = true;
    let context = context_with_keys(&[(first.id, [0x11; KEY_LEN]), (second.id, [0x22; KEY_LEN])]);
    let mut now = ticking_now();

    reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).unwrap();

    assert!(store.aliases.is_empty());
    assert!(store.outbox.is_empty());
    assert!(store.lists.get(&first.id).unwrap().is_default);
}

#[test]
fn pulled_old_generation_live_head_uses_history_key_and_repushed_active_generation() {
    let list = sample_list(uuid(77), false);
    let old_dek = [0x31; KEY_LEN];
    let active_dek = [0x32; KEY_LEN];
    let record = encrypted_list_record(&list, &old_dek);
    let mut context = context_with_keys(&[(list.id, active_dek)]);
    context.keys.tenant_generation = 2;
    context
        .keys
        .historical_tenant_root_deks
        .push((1, Zeroizing::new(old_dek)));
    let mut store = FakeStore::default();
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    assert_eq!(
        apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
        ApplyDisposition::Rebased
    );
    let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
        panic!("live")
    };
    assert_eq!(
        crate::parse_envelope_header(blob).unwrap().key_generation,
        2
    );
    assert!(crate::envelope::decrypt_plaintext(
        &active_dek,
        test_tenant_id(),
        2,
        LISTS_COLLECTION,
        list.id,
        blob,
    )
    .is_ok());
}

#[test]
fn missing_tenant_key_fails_before_canonical_materialization() {
    let canonical = sample_list(uuid(1), true);
    let loser = sample_list(uuid(2), true);
    let mut canonical_domain = canonical.clone();
    canonical_domain.is_default = false;
    let mut store = FakeStore::default();
    store.lists.insert(canonical.id, canonical_domain);
    store.lists.insert(loser.id, loser.clone());
    for list in [&canonical, &loser] {
        put_live_plaintext_state(
            &mut store,
            SyncCollection::Lists,
            list.id,
            list_plaintext(list, test_hlc(1, "remote")),
        );
    }
    let mut context = context_with_keys(&[(loser.id, [0x22; KEY_LEN])]);
    context.keys.tenant_root_dek = None;
    let mut now = ticking_now();

    assert!(reconcile_canonical_inbox_in_transaction(&context, &mut store, &mut now).is_err());
    assert!(!store.lists.get(&canonical.id).unwrap().is_default);
    assert!(store.lists.get(&loser.id).unwrap().is_default);
    assert!(store.aliases.is_empty());
    assert!(store.outbox.is_empty());
}

#[test]
fn pulled_task_for_durable_alias_is_rehomed_and_reencrypted() {
    let canonical = sample_list(uuid(1), true);
    let mut alias = sample_list(uuid(2), true);
    alias.is_default = false;
    let tenant_dek = [0x41; KEY_LEN];
    let mut task = new_task(
        alias.id,
        None,
        "late".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1_799_000_000_000,
    )
    .unwrap();
    task.id = uuid(40);
    let task_hlc = test_hlc(1, "remote");
    let task_plaintext = task_plaintext(&task, task_hlc.clone());
    let record = encrypted_task_record(task.id, &task_plaintext, &tenant_dek, &task_hlc);
    let context = context_with_keys(&[(canonical.id, tenant_dek)]);
    let mut store = FakeStore::default();
    store.lists.insert(canonical.id, canonical.clone());
    store.lists.insert(alias.id, alias.clone());
    store.aliases = vec![LocalListAlias {
        alias_list_id: alias.id,
        canonical_list_id: canonical.id,
    }];
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    assert_eq!(
        apply_pull_task(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
        ApplyDisposition::Rebased
    );
    assert_eq!(store.tasks.get(&task.id).unwrap().list_id, canonical.id);
    let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
        panic!("live");
    };
    let plaintext =
        decrypt_plaintext(&tenant_dek, TASKS_COLLECTION, &task.id.to_string(), blob).unwrap();
    let SyncPlaintext::Task(plaintext) = plaintext else {
        panic!("task");
    };
    assert_eq!(plaintext.placement.value.list_id, canonical.id);
    assert_eq!(summary.repush_count, 1);
}

#[test]
fn remote_tombstone_discards_newer_local_live_and_outbox() {
    let list_id = uuid(30);
    let local = sample_list(list_id, false);
    let default_list = sample_list(uuid(29), true);
    let local_hlc = Hlc {
        wall_ms: 1_799_000_000_500,
        counter: 0,
        device_id: "local".to_string(),
    }
    .encode()
    .unwrap();
    let delete_hlc = Hlc {
        wall_ms: 1_799_000_000_100,
        counter: 0,
        device_id: "remote".to_string(),
    }
    .encode()
    .unwrap();
    let revision_hlc = Hlc {
        wall_ms: 1_799_000_000_600,
        counter: 0,
        device_id: "remote".to_string(),
    }
    .encode()
    .unwrap();
    let mut store = FakeStore::default();
    store.lists.insert(default_list.id, default_list);
    store.lists.insert(list_id, local.clone());
    store.record_states.insert(
        (SyncCollection::Lists, list_id),
        LocalSyncRecordState {
            current_revision_hlc: Some(local_hlc.clone()),
            state: LocalSyncSemanticState::Live {
                mutation_hlc: local_hlc.clone(),
                plaintext_json: serde_json::to_string(&list_plaintext(
                    &local,
                    Hlc::decode(&local_hlc).unwrap(),
                ))
                .unwrap(),
            },
        },
    );
    store.outbox.push(LocalSyncOutboxEntry {
        op_id: Uuid::now_v7(),
        record_id: list_id,
        collection: SyncCollection::Lists,
        base_revision_hlc: Some(local_hlc.clone()),
        revision_hlc: local_hlc,
        state: EncryptedSyncState::Live {
            mutation_hlc: Hlc::decode(&revision_hlc).unwrap().encode().unwrap(),
            blob: vec![1],
        },
        created_at: 1,
    });
    let record = PullRecord {
        record_id: list_id,
        collection: SyncCollection::Lists,
        seq: 2,
        revision_hlc: revision_hlc.clone(),
        state: EncryptedSyncState::Tombstone {
            delete_hlc: delete_hlc.clone(),
        },
    };
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(
        &record,
        &context_for(list_id, [3; KEY_LEN]),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert!(!store.lists.contains_key(&list_id));
    assert!(store.outbox.is_empty());
    assert_eq!(
        store.record_states[&(SyncCollection::Lists, list_id)],
        LocalSyncRecordState {
            current_revision_hlc: Some(revision_hlc),
            state: LocalSyncSemanticState::Tombstone { delete_hlc },
        }
    );
    assert_eq!(summary.repush_count, 0);
}

#[test]
fn remote_series_tombstone_removes_only_series_and_keeps_generated_task() {
    let series = sample_series(uuid(31));
    let list = sample_list(uuid(32), true);
    let mut generated = new_task(
        list.id,
        None,
        "Generated".into(),
        "7fffffffffffffffffffffffffffffff".into(),
        1,
    )
    .unwrap();
    generated.series_occurrence = Some(taskveil_domain::SeriesOccurrenceRef {
        series_id: series.id,
        series_revision: series.config.config_revision.clone(),
        occurrence_at: series.config.starts_at,
        blueprint_node_key: "root".into(),
    });
    let delete_hlc = test_hlc(1, "remote").encode().unwrap();
    let revision_hlc = test_hlc(2, "remote").encode().unwrap();
    let record = PullRecord {
        record_id: series.id,
        collection: SyncCollection::TaskSeries,
        seq: 2,
        revision_hlc: revision_hlc.clone(),
        state: EncryptedSyncState::Tombstone {
            delete_hlc: delete_hlc.clone(),
        },
    };
    let mut store = FakeStore::default();
    store.lists.insert(list.id, list);
    store.series.insert(series.id, series.clone());
    store.tasks.insert(generated.id, generated.clone());
    store.outbox.push(LocalSyncOutboxEntry {
        op_id: Uuid::now_v7(),
        record_id: series.id,
        collection: SyncCollection::TaskSeries,
        base_revision_hlc: None,
        revision_hlc: test_hlc(1, "local").encode().unwrap(),
        state: EncryptedSyncState::Live {
            mutation_hlc: test_hlc(1, "local").encode().unwrap(),
            blob: vec![1],
        },
        created_at: 1,
    });
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_task_series(
        &record,
        &context_with_keys(&[]),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert!(!store.series.contains_key(&series.id));
    assert_eq!(store.tasks.get(&generated.id), Some(&generated));
    assert!(store.outbox.is_empty());
    assert_eq!(
        store.record_states[&(SyncCollection::TaskSeries, series.id)],
        LocalSyncRecordState {
            current_revision_hlc: Some(revision_hlc),
            state: LocalSyncSemanticState::Tombstone { delete_hlc },
        }
    );
    assert_eq!(summary.deleted_count, 1);
}

#[test]
fn remote_template_tombstone_keeps_independent_series_and_task() {
    let template = sample_template(uuid(41));
    let mut series = sample_series(uuid(42));
    series.config.blueprint = template.blueprint.clone();
    let list = sample_list(uuid(43), true);
    let generated = new_task(
        list.id,
        None,
        "Generated".into(),
        "7fffffffffffffffffffffffffffffff".into(),
        1,
    )
    .unwrap();
    let delete_hlc = test_hlc(1, "remote").encode().unwrap();
    let revision_hlc = test_hlc(2, "remote").encode().unwrap();
    let record = PullRecord {
        record_id: template.id,
        collection: SyncCollection::Templates,
        seq: 2,
        revision_hlc,
        state: EncryptedSyncState::Tombstone { delete_hlc },
    };
    let mut store = FakeStore::default();
    store.templates.insert(template.id, template.clone());
    store.series.insert(series.id, series.clone());
    store.tasks.insert(generated.id, generated.clone());
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_template(
        &record,
        &context_with_keys(&[]),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert!(!store.templates.contains_key(&template.id));
    assert_eq!(store.series.get(&series.id), Some(&series));
    assert_eq!(store.tasks.get(&generated.id), Some(&generated));
    assert_eq!(summary.deleted_count, 1);
}

#[test]
fn remote_list_tombstone_rehomes_known_descendant_and_republishes_it() {
    let list_id = uuid(33);
    let list = sample_list(list_id, false);
    let default_list = sample_list(uuid(34), true);
    let task = new_task(
        list_id,
        None,
        "known descendant".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1_799_000_000_000,
    )
    .unwrap();
    let current_revision = Hlc {
        wall_ms: 1_799_000_000_100,
        counter: 0,
        device_id: "local".to_string(),
    }
    .encode()
    .unwrap();
    let delete_hlc = Hlc {
        wall_ms: 1_799_000_000_200,
        counter: 0,
        device_id: "remote".to_string(),
    }
    .encode()
    .unwrap();
    let mut store = FakeStore::default();
    store.lists.insert(default_list.id, default_list.clone());
    store.lists.insert(list_id, list);
    store.tasks.insert(task.id, task.clone());
    store.record_states.insert(
        (SyncCollection::Tasks, task.id),
        LocalSyncRecordState {
            current_revision_hlc: Some(current_revision.clone()),
            state: LocalSyncSemanticState::Live {
                mutation_hlc: current_revision.clone(),
                plaintext_json: serde_json::to_string(&task_plaintext(
                    &task,
                    Hlc::decode(&current_revision).unwrap(),
                ))
                .unwrap(),
            },
        },
    );
    store.outbox.push(LocalSyncOutboxEntry {
        op_id: Uuid::now_v7(),
        record_id: task.id,
        collection: SyncCollection::Tasks,
        base_revision_hlc: Some(current_revision.clone()),
        revision_hlc: current_revision,
        state: EncryptedSyncState::Live {
            mutation_hlc: delete_hlc.clone(),
            blob: vec![1],
        },
        created_at: 1,
    });
    let record = PullRecord {
        record_id: list_id,
        collection: SyncCollection::Lists,
        seq: 3,
        revision_hlc: delete_hlc.clone(),
        state: EncryptedSyncState::Tombstone {
            delete_hlc: delete_hlc.clone(),
        },
    };
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(
        &record,
        &context_for(list_id, [5; KEY_LEN]),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert_eq!(store.tasks.get(&task.id).unwrap().list_id, default_list.id);
    assert_eq!(store.outbox.len(), 1);
    assert_eq!(store.outbox[0].record_id, task.id);
    assert!(matches!(
        store.outbox[0].state,
        EncryptedSyncState::Live { .. }
    ));
}

#[test]
fn late_descendant_of_tombstoned_list_is_rehomed_to_default_inbox() {
    let list_id = uuid(31);
    let task_id = uuid(32);
    let default_list = sample_list(uuid(30), true);
    let dek = [4; KEY_LEN];
    let task = new_task(
        list_id,
        None,
        "late".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1_799_000_000_000,
    )
    .unwrap();
    let mut task = task;
    task.id = task_id;
    let hlc = Hlc {
        wall_ms: 1_799_000_000_200,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let plaintext = task_plaintext(&task, hlc.clone());
    let record = encrypted_task_record(task_id, &plaintext, &dek, &hlc);
    let mut store = FakeStore::default();
    store.lists.insert(default_list.id, default_list.clone());
    store.record_states.insert(
        (SyncCollection::Lists, list_id),
        LocalSyncRecordState {
            current_revision_hlc: Some(record.revision_hlc.clone()),
            state: LocalSyncSemanticState::Tombstone {
                delete_hlc: record.revision_hlc.clone(),
            },
        },
    );
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_task(
        &record,
        &context_for(list_id, dek),
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert_eq!(store.tasks.get(&task_id).unwrap().list_id, default_list.id);
    assert_eq!(store.outbox.len(), 1);
    assert!(matches!(
        store.outbox[0].state,
        EncryptedSyncState::Live { .. }
    ));
    assert_eq!(summary.repush_count, 1);
}

#[test]
fn conflict_current_merges_distinct_fields_and_rebases_without_first_client() {
    let list_id = uuid(4);
    let dek = [0x4c; KEY_LEN];
    let base_clock = Hlc {
        wall_ms: 1_799_000_000_000,
        counter: 0,
        device_id: "base".to_string(),
    };
    let local_clock = Hlc {
        wall_ms: 1_799_000_000_100,
        counter: 0,
        device_id: "client-b".to_string(),
    };
    let remote_clock = Hlc {
        wall_ms: 1_799_000_000_101,
        counter: 0,
        device_id: "client-a".to_string(),
    };
    let server_revision = Hlc {
        wall_ms: 1_799_000_000_102,
        counter: 0,
        device_id: "client-a".to_string(),
    }
    .encode()
    .unwrap();
    let base_list = sample_list(list_id, false);
    let mut local_list_for_plaintext = base_list.clone();
    local_list_for_plaintext.color = "#00ff00".to_string();
    let local_plaintext = list_plaintext(&base_list, base_clock.clone())
        .stamp_list_changes(&local_list_for_plaintext, local_clock.clone())
        .unwrap();
    let mut remote_list_for_plaintext = base_list.clone();
    remote_list_for_plaintext.name = "Remote name".to_string();
    let remote_plaintext = list_plaintext(&base_list, base_clock)
        .stamp_list_changes(&remote_list_for_plaintext, remote_clock.clone())
        .unwrap();

    let blob = encrypt_plaintext(
        &dek,
        LISTS_COLLECTION,
        &list_id.to_string(),
        &remote_plaintext,
    )
    .unwrap();
    let record = PullRecord {
        record_id: list_id,
        collection: SyncCollection::Lists,
        seq: 2,
        revision_hlc: server_revision.clone(),
        state: EncryptedSyncState::Live {
            mutation_hlc: remote_clock.encode().unwrap(),
            blob,
        },
    };
    let context = context_for(list_id, dek);
    let mut local_list = base_list;
    local_list.color = "#00ff00".to_string();
    let mut store = FakeStore::default();
    store.lists.insert(list_id, local_list);
    store.record_states.insert(
        (SyncCollection::Lists, list_id),
        LocalSyncRecordState {
            current_revision_hlc: Some(
                Hlc {
                    wall_ms: 1_799_000_000_001,
                    counter: 0,
                    device_id: "base".to_string(),
                }
                .encode()
                .unwrap(),
            ),
            state: LocalSyncSemanticState::Live {
                mutation_hlc: local_clock.encode().unwrap(),
                plaintext_json: serde_json::to_string(&local_plaintext).unwrap(),
            },
        },
    );
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    apply_pull_list(&record, &context, &mut store, &mut now, &mut summary).unwrap();

    let merged_list = store.lists.get(&list_id).unwrap();
    assert_eq!(merged_list.name, "Remote name");
    assert_eq!(merged_list.color, "#00ff00");
    assert_eq!(summary.repush_count, 1);
    assert_eq!(store.outbox.len(), 1);
    assert_eq!(
        store.outbox[0].base_revision_hlc.as_deref(),
        Some(server_revision.as_str())
    );
    let EncryptedSyncState::Live { blob, .. } = &store.outbox[0].state else {
        panic!("expected rebased live head");
    };
    let rebased = decrypt_plaintext(&dek, LISTS_COLLECTION, &list_id.to_string(), blob).unwrap();
    let SyncPlaintext::List(rebased) = rebased else {
        panic!("list");
    };
    assert_eq!(rebased.name.value, "Remote name");
    assert_eq!(rebased.color.value, "#00ff00");
}

#[test]
fn undecryptable_conflict_current_keeps_the_local_outbox_head() {
    let list_id = uuid(5);
    let dek = [0x5d; KEY_LEN];
    let semantic_hlc = Hlc {
        wall_ms: 1_799_000_000_010,
        counter: 0,
        device_id: "local".to_string(),
    }
    .encode()
    .unwrap();
    let revision_hlc = Hlc {
        wall_ms: 1_799_000_000_011,
        counter: 0,
        device_id: "local".to_string(),
    }
    .encode()
    .unwrap();
    let stale_op_id = Uuid::now_v7();
    let mut store = FakeStore::default();
    store.outbox.push(LocalSyncOutboxEntry {
        op_id: stale_op_id,
        record_id: list_id,
        collection: SyncCollection::Lists,
        base_revision_hlc: None,
        revision_hlc: revision_hlc.clone(),
        state: EncryptedSyncState::Live {
            mutation_hlc: semantic_hlc,
            blob: vec![1, 2, 3],
        },
        created_at: 1,
    });
    let current = PullRecord {
        record_id: list_id,
        collection: SyncCollection::Lists,
        seq: 1,
        revision_hlc,
        state: EncryptedSyncState::Live {
            mutation_hlc: Hlc {
                wall_ms: 1_799_000_000_009,
                counter: 0,
                device_id: "remote".to_string(),
            }
            .encode()
            .unwrap(),
            blob: vec![0xff; crate::envelope::ENVELOPE_MIN_LEN],
        },
    };
    let context = context_for(list_id, dek);
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    assert_eq!(
        apply_pull_record(&current, &context, &mut store, &mut now, &mut summary).unwrap(),
        ApplyDisposition::UpgradeRequired(0xff)
    );

    assert_eq!(store.outbox.len(), 1);
    assert_eq!(store.outbox[0].op_id, stale_op_id);
    assert_eq!(summary.decrypt_failed_count, 0);
}

#[test]
fn pull_rejects_authenticated_task_with_unencodable_field_clock() {
    let list_id = uuid(50);
    let record_id = uuid(51);
    let dek = [0x51; KEY_LEN];
    let task = new_task(
        list_id,
        None,
        "invalid clock".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        1,
    )
    .unwrap();
    let clock = Hlc {
        wall_ms: 1_799_000_000_050,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let mut plaintext = SyncPlaintext::from_task(&task, clock.clone()).unwrap();
    let SyncPlaintext::Task(fields) = &mut plaintext else {
        panic!("task");
    };
    fields.note.hlc.device_id.clear();
    let plaintext_json = serde_json::to_vec(&plaintext).unwrap();
    let mut aad = Vec::new();
    aad.extend_from_slice(b"TDA5");
    aad.extend_from_slice(&taskveil_crypto::CRYPTO_SUITE_ID.to_be_bytes());
    aad.extend_from_slice(&1_u64.to_be_bytes());
    aad.extend_from_slice(test_tenant_id().as_bytes());
    aad.extend_from_slice(&(TASKS_COLLECTION.len() as u16).to_be_bytes());
    aad.extend_from_slice(TASKS_COLLECTION.as_bytes());
    aad.extend_from_slice(record_id.as_bytes());
    let mut blob = Vec::new();
    blob.extend_from_slice(b"TDE5");
    blob.extend_from_slice(&taskveil_crypto::CRYPTO_SUITE_ID.to_be_bytes());
    blob.extend_from_slice(&1_u64.to_be_bytes());
    let record_key = taskveil_crypto::key_hierarchy::derive_record_key(
        &dek,
        test_tenant_id(),
        1,
        TASKS_COLLECTION,
        record_id,
    )
    .unwrap();
    blob.extend_from_slice(&taskveil_crypto::encrypt(&record_key, &plaintext_json, &aad).unwrap());
    let record = PullRecord {
        record_id,
        collection: SyncCollection::Tasks,
        seq: 1,
        revision_hlc: Hlc {
            wall_ms: clock.wall_ms + 1,
            ..clock.clone()
        }
        .encode()
        .unwrap(),
        state: EncryptedSyncState::Live {
            mutation_hlc: clock.encode().unwrap(),
            blob,
        },
    };
    let context = context_for(list_id, dek);
    let mut store = FakeStore::default();
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    assert_eq!(
        apply_pull_record(&record, &context, &mut store, &mut now, &mut summary).unwrap(),
        ApplyDisposition::Deferred(PullFailureReason::InvalidPlaintext, None)
    );
    assert_eq!(summary.decrypt_failed_count, 0);
    assert!(!store
        .record_states
        .contains_key(&(SyncCollection::Tasks, record_id)));
}

#[test]
fn stale_response_does_not_apply_after_a_newer_local_head_replaces_its_op() {
    let list_id = uuid(6);
    let dek = [0x6e; KEY_LEN];
    let current = encrypted_list_record(
        &List {
            name: "Remote stale".to_string(),
            ..sample_list(list_id, false)
        },
        &dek,
    );
    let newer_op_id = Uuid::now_v7();
    let clock = Hlc {
        wall_ms: 1_799_000_000_020,
        counter: 0,
        device_id: "new-local".to_string(),
    }
    .encode()
    .unwrap();
    let mut store = FakeStore::default();
    store.lists.insert(
        list_id,
        List {
            name: "New local".to_string(),
            ..sample_list(list_id, false)
        },
    );
    store.outbox.push(LocalSyncOutboxEntry {
        op_id: newer_op_id,
        record_id: list_id,
        collection: SyncCollection::Lists,
        base_revision_hlc: None,
        revision_hlc: clock.clone(),
        state: EncryptedSyncState::Live {
            mutation_hlc: clock,
            blob: vec![1],
        },
        created_at: 1,
    });
    let context = context_for(list_id, dek);
    let mut now = ticking_now();
    let mut summary = SyncRunSummary::default();

    reconcile_nonaccepted_push_in_transaction(
        &current,
        Uuid::now_v7(),
        &context,
        &mut store,
        &mut now,
        &mut summary,
    )
    .unwrap();

    assert_eq!(store.lists[&list_id].name, "New local");
    assert_eq!(store.outbox.len(), 1);
    assert_eq!(store.outbox[0].op_id, newer_op_id);
    assert_eq!(summary.applied_count, 0);
}

fn encrypted_list_record(list: &List, dek: &[u8; KEY_LEN]) -> PullRecord {
    let hlc = Hlc {
        wall_ms: list.updated_at,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let plaintext = list_plaintext(list, hlc.clone());
    let blob = encrypt_plaintext(dek, LISTS_COLLECTION, &list.id.to_string(), &plaintext)
        .expect("test list plaintext encrypts");
    PullRecord {
        record_id: list.id,
        collection: SyncCollection::Lists,
        seq: 1,
        revision_hlc: hlc.encode().unwrap(),
        state: EncryptedSyncState::Live {
            mutation_hlc: hlc.encode().unwrap(),
            blob,
        },
    }
}

fn put_live_plaintext_state(
    store: &mut FakeStore,
    collection: SyncCollection,
    record_id: Uuid,
    plaintext: SyncPlaintext,
) {
    let mutation_hlc = plaintext.record_hlc().encode().unwrap();
    store.record_states.insert(
        (collection, record_id),
        LocalSyncRecordState {
            current_revision_hlc: Some(mutation_hlc.clone()),
            state: LocalSyncSemanticState::Live {
                mutation_hlc,
                plaintext_json: serde_json::to_string(&plaintext).unwrap(),
            },
        },
    );
}

fn encrypted_task_record(
    record_id: Uuid,
    plaintext: &SyncPlaintext,
    dek: &[u8; KEY_LEN],
    hlc: &Hlc,
) -> PullRecord {
    PullRecord {
        record_id,
        collection: SyncCollection::Tasks,
        seq: 1,
        revision_hlc: hlc.encode().unwrap(),
        state: EncryptedSyncState::Live {
            mutation_hlc: hlc.encode().unwrap(),
            blob: encrypt_plaintext(dek, TASKS_COLLECTION, &record_id.to_string(), plaintext)
                .unwrap(),
        },
    }
}

fn context_for(list_id: Uuid, dek: [u8; KEY_LEN]) -> ActiveSyncContext {
    context_with_keys(&[(list_id, dek)])
}

#[test]
fn persisted_manifest_anchor_requires_complete_authenticated_successor_chain() {
    let tenant_id = test_tenant_id();
    let first = crate::KeyManifest::authenticate_personal(
        tenant_id,
        1,
        crate::RotationStatus::Active,
        1,
        [0; 32],
        Vec::new(),
        &[0x41; 32],
    )
    .unwrap();
    let prepared = crate::KeyManifest::authenticate_personal(
        tenant_id,
        2,
        crate::RotationStatus::Prepared,
        1,
        first.authenticated_hash().unwrap(),
        Vec::new(),
        &[0x41; 32],
    )
    .unwrap();
    let active = crate::KeyManifest::authenticate_personal(
        tenant_id,
        2,
        crate::RotationStatus::Active,
        2,
        prepared.authenticated_hash().unwrap(),
        Vec::new(),
        &[0x41; 32],
    )
    .unwrap();
    let mut store = FakeStore::default();
    store.settings.insert(
        manifest_anchor_key(),
        STANDARD.encode(first.authenticated_bytes().unwrap()),
    );
    let context = context_with_keys(&[]);
    let descriptor = crate::protocol::KeyManifestDescriptor {
        suite_id: taskveil_crypto::CRYPTO_SUITE_ID,
        generation: 2,
        status: crate::RotationStatus::Active,
        minimum_write_generation: 2,
        signed_manifest: STANDARD.encode(active.authenticated_bytes().unwrap()),
        predecessor_manifests: vec![STANDARD.encode(prepared.authenticated_bytes().unwrap())],
    };

    verify_manifest_anchor(&descriptor, &active, &context, &mut store).unwrap();
    let mut missing_predecessor = descriptor;
    missing_predecessor.predecessor_manifests.clear();
    assert!(verify_manifest_anchor(&missing_predecessor, &active, &context, &mut store).is_err());
}

fn context_with_keys(keys: &[(Uuid, [u8; KEY_LEN])]) -> ActiveSyncContext {
    ActiveSyncContext {
        server_url: "http://localhost".to_string(),
        tenant_id: uuid(100),
        device_id: "local".to_string(),
        session_token: crate::SecretString::new("token"),
        keys: LocalSyncKeys {
            tenant_id: test_tenant_id(),
            tenant_root_dek: Some(Zeroizing::new(
                keys.first().map_or([0x7b; KEY_LEN], |(_, key)| *key),
            )),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        },
        manifest_auth_key: crate::derive_personal_manifest_auth_key(&[0x41; 32]).unwrap(),
    }
}

#[test]
fn active_context_debug_never_renders_bearer_token() {
    let mut context = context_with_keys(&[]);
    context.session_token = crate::SecretString::new("bearer-secret");
    let rendered = format!("{context:?}");

    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("bearer-secret"));
}

fn test_hlc(counter: u32, device_id: &str) -> Hlc {
    Hlc {
        wall_ms: 1_799_000_000_000,
        counter,
        device_id: device_id.to_string(),
    }
}

fn sample_list(id: Uuid, is_default: bool) -> List {
    List {
        id,
        name: format!("List {id}"),
        color: "#ffffff".to_string(),
        icon: "list".to_string(),
        sort_order: "7fffffffffffffffffffffffffffffff".to_string(),
        is_default,
        archived_at: None,
        created_at: 1_799_000_000_000,
        updated_at: 1_799_000_000_000,
    }
}

fn sample_series(id: Uuid) -> TaskSeries {
    TaskSeries {
        id,
        config: TaskSeriesConfig {
            blueprint: TaskBlueprint {
                schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
                nodes: vec![TaskBlueprintNode {
                    node_key: "root".into(),
                    parent_node_key: None,
                    sibling_order: 0,
                    content: TaskContent {
                        title: "Generated".into(),
                        note: String::new(),
                        priority: 0,
                        estimated_minutes: None,
                    },
                }],
            },
            target_list_id: None,
            rrule: "FREQ=DAILY;COUNT=1".into(),
            starts_at: 1_800_000_000_000,
            time_zone: "UTC".into(),
            enabled: true,
            config_revision: "revision-a".into(),
            config_parent_revision: None,
            config_effective_from: 1,
            lineage: Vec::new(),
        },
        cursor: SeriesCursor::Pending(1_800_000_000_000),
        created_at: 1,
        updated_at: 1,
    }
}

fn sample_template(id: Uuid) -> TaskTemplate {
    TaskTemplate {
        id,
        name: "Reusable".into(),
        default_list_id: None,
        blueprint: sample_series(uuid(999)).config.blueprint,
        blueprint_revision: "template-revision-a".into(),
        created_at: 1,
        updated_at: 1,
    }
}

fn ticking_now() -> impl FnMut() -> Result<i64, String> {
    let mut now = 1_799_000_000_000;
    move || {
        now += 1;
        Ok(now)
    }
}

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

#[test]
fn durable_upgrade_block_reopens_when_supported_versions_catch_up() {
    assert!(upgrade_block_is_active("9:5"));
    assert!(upgrade_block_is_active("8:6"));
    assert!(!upgrade_block_is_active("8:5"));
    assert!(!upgrade_block_is_active("6:5"));
    assert!(!upgrade_block_is_active("7:3"));
    assert!(!upgrade_block_is_active("0:0"));
    assert!(upgrade_block_is_active("invalid"));
}
