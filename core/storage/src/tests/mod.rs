use super::*;
use taskveil_crypto::{derive_local_db_key, ensure_device_key, InMemoryDeviceKeyStore};
use taskveil_domain::{
    new_list, new_task, transition_task, update_title, TaskBlueprint, TaskBlueprintNode,
    TaskContent, TaskSeriesConfig, TASK_BLUEPRINT_SCHEMA_REVISION,
};
use tempfile::NamedTempFile;

const KEY: [u8; 32] = [0x11; 32];
const WRONG_KEY: [u8; 32] = [0x22; 32];

fn local_tenant_root_bundle(tenant_id: Uuid, updated_at: i64) -> LocalTenantRootKeyBundle {
    LocalTenantRootKeyBundle {
        tenant_id,
        generation: 1,
        wrapped_tenant_root_dek: vec![0x5a; 48],
        updated_at,
    }
}

fn sample_task() -> Task {
    Task {
        id: Uuid::now_v7(),
        list_id: Uuid::now_v7(),
        parent_task_id: Some(Uuid::now_v7()),
        content: TaskContent {
            title: "Buy milk".to_string(),
            note: "Organic whole milk".to_string(),
            priority: 2,
            estimated_minutes: Some(15),
        },
        status: TaskStatus::Todo,
        due: Some(TaskDue::date_time(1_800_000_000_000, "UTC").unwrap()),
        scheduled_at: Some(1_799_900_000_000),
        sort_order: "a0".to_string(),
        completed_at: None,
        closed_reason: None,
        deleted_at: None,
        assignee: Some(Uuid::now_v7()),
        series_occurrence: None,
        created_at: 1_799_000_000_000,
        updated_at: 1_799_000_000_000,
    }
}

fn insert_task_pre_v20(connection: &Connection, task: &Task) {
    let (due_kind, due_on, due_at_ms, due_time_zone) = task_due_parts(task.due.as_ref());
    connection
        .execute(
            "INSERT INTO tasks (
                     id, list_id, parent_task_id, title, note, status, priority,
                     due_kind, due_on, due_at_ms, due_time_zone, scheduled_at,
                     estimated_minutes, sort_order, completed_at, closed_reason,
                     deleted_at, assignee, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                 )",
            params![
                task.id.to_string(),
                task.list_id.to_string(),
                task.parent_task_id.map(|id| id.to_string()),
                task.content.title,
                task.content.note,
                status_to_str(task.status),
                task.content.priority,
                due_kind,
                due_on,
                due_at_ms,
                due_time_zone,
                task.scheduled_at,
                task.content.estimated_minutes,
                task.sort_order,
                task.completed_at,
                task.closed_reason,
                task.deleted_at,
                task.assignee.map(|id| id.to_string()),
                task.created_at,
                task.updated_at,
            ],
        )
        .unwrap();
}

fn sample_list(sort_order: &str) -> List {
    List {
        id: Uuid::now_v7(),
        name: format!("List {sort_order}"),
        color: "#4F8EF7".to_string(),
        icon: "list".to_string(),
        sort_order: sort_order.to_string(),
        is_default: false,
        archived_at: None,
        created_at: 1_799_000_000_000,
        updated_at: 1_799_000_000_000,
    }
}

fn sample_template() -> TaskTemplate {
    TaskTemplate {
        id: Uuid::now_v7(),
        name: "Template".to_string(),
        default_list_id: None,
        blueprint: TaskBlueprint {
            schema_revision: TASK_BLUEPRINT_SCHEMA_REVISION,
            nodes: vec![TaskBlueprintNode {
                node_key: "root".to_string(),
                parent_node_key: None,
                sibling_order: 0,
                content: TaskContent {
                    title: "Generated task".to_string(),
                    note: String::new(),
                    priority: 0,
                    estimated_minutes: None,
                },
            }],
        },
        blueprint_revision: "template-r1".to_string(),
        created_at: 1,
        updated_at: 1,
    }
}

fn sample_schedule(template_id: Uuid) -> TaskSeries {
    TaskSeries {
        id: Uuid::now_v7(),
        config: TaskSeriesConfig {
            blueprint: {
                let mut blueprint = sample_template().blueprint;
                blueprint.nodes[0].node_key = template_id.to_string();
                blueprint
            },
            target_list_id: None,
            rrule: "FREQ=DAILY".to_string(),
            starts_at: 1_800_000_000_000,
            time_zone: "UTC".to_string(),
            enabled: true,
            config_revision: "schedule-r1".to_string(),
            config_parent_revision: None,
            config_effective_from: 1,
            lineage: Vec::new(),
        },
        cursor: SeriesCursor::Pending(1_800_000_000_000),
        created_at: 1,
        updated_at: 1,
    }
}

fn new_live_outbox(
    record_id: Uuid,
    collection: &str,
    op_id: Uuid,
    base_revision_hlc: Option<&str>,
    revision_hlc: &str,
    mutation_hlc: &str,
    blob: Vec<u8>,
) -> NewSyncOutboxEntry {
    NewSyncOutboxEntry {
        op_id,
        record_id,
        collection: collection.to_string(),
        base_revision_hlc: base_revision_hlc.map(str::to_string),
        revision_hlc: revision_hlc.to_string(),
        state: SyncOutboxState::Live {
            mutation_hlc: mutation_hlc.to_string(),
            blob,
        },
        created_at: 1_799_000_000_000,
    }
}

fn live_record_state(
    record_id: Uuid,
    collection: &str,
    current_revision_hlc: Option<&str>,
    mutation_hlc: &str,
    plaintext_json: &str,
    updated_at: i64,
) -> SyncRecordState {
    SyncRecordState {
        record_id,
        collection: collection.to_string(),
        current_revision_hlc: current_revision_hlc.map(str::to_string),
        state: SyncRecordSemanticState::Live {
            mutation_hlc: mutation_hlc.to_string(),
            plaintext_json: plaintext_json.to_string(),
        },
        updated_at,
    }
}

fn open_raw_encrypted(path: &Path, key: &[u8; 32]) -> Connection {
    let connection = Connection::open(path).unwrap();
    apply_sqlcipher_key(&connection, key).unwrap();
    connection
}

fn create_baseline_v1_database(path: &Path, key: &[u8; 32], set_version: bool) {
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    transaction.execute_batch(SCHEMA).unwrap();
    if set_version {
        set_user_version(&transaction, BASELINE_SCHEMA_VERSION).unwrap();
    }
    transaction.commit().unwrap();
}

fn insert_baseline_v1_list(connection: &Connection, list: &List) {
    connection
        .execute(
            "INSERT INTO lists (
                    id, name, color, icon, org_id, sort_order, created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
                )",
            params![
                list.id.to_string(),
                list.name,
                list.color,
                list.icon,
                Option::<String>::None,
                list.sort_order,
                list.created_at,
                list.updated_at,
            ],
        )
        .unwrap();
}

fn create_v2_database(path: &Path, key: &[u8; 32]) {
    create_baseline_v1_database(path, key, true);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_lists_archived_at(&transaction).unwrap();
    set_user_version(&transaction, 2).unwrap();
    transaction.commit().unwrap();
}

fn create_v3_database(path: &Path, key: &[u8; 32]) {
    create_v2_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_lists_is_default(&transaction).unwrap();
    set_user_version(&transaction, 3).unwrap();
    transaction.commit().unwrap();
}

fn create_v4_database(path: &Path, key: &[u8; 32]) {
    create_v3_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    rebuild_tasks_fts_triggers(&transaction).unwrap();
    set_user_version(&transaction, 4).unwrap();
    transaction.commit().unwrap();
}

fn create_v5_database(path: &Path, key: &[u8; 32]) {
    create_v4_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_settings(&transaction).unwrap();
    set_user_version(&transaction, 5).unwrap();
    transaction.commit().unwrap();
}

fn create_v6_database(path: &Path, key: &[u8; 32]) {
    create_v5_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_reminders(&transaction).unwrap();
    set_user_version(&transaction, 6).unwrap();
    transaction.commit().unwrap();
}

fn create_v7_database(path: &Path, key: &[u8; 32]) {
    create_v6_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_performance_indexes(&transaction).unwrap();
    set_user_version(&transaction, 7).unwrap();
    transaction.commit().unwrap();
}

fn create_v9_database(path: &Path, key: &[u8; 32]) {
    create_v7_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_sync_outbox_and_cursors(&transaction).unwrap();
    set_user_version(&transaction, 8).unwrap();
    add_sync_record_states(&transaction).unwrap();
    set_user_version(&transaction, 9).unwrap();
    transaction.commit().unwrap();
}

fn create_v10_database(path: &Path, key: &[u8; 32]) {
    create_v9_database(path, key);
    let mut connection = open_raw_encrypted(path, key);
    let transaction = connection.transaction().unwrap();
    add_local_crypto_cache(&transaction).unwrap();
    set_user_version(&transaction, 10).unwrap();
    transaction.commit().unwrap();
}

fn insert_v2_list(connection: &Connection, list: &List) {
    connection
        .execute(
            "INSERT INTO lists (
                    id, name, color, icon, org_id, sort_order, archived_at,
                    created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
                )",
            params![
                list.id.to_string(),
                list.name,
                list.color,
                list.icon,
                Option::<String>::None,
                list.sort_order,
                list.archived_at,
                list.created_at,
                list.updated_at,
            ],
        )
        .unwrap();
}

fn list_column(connection: &Connection, target: &str) -> Option<(String, i32, String)> {
    let mut statement = connection.prepare("PRAGMA table_info(lists)").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .find_map(|(name, column_type, not_null, default_value)| {
            (name == target).then_some((column_type, not_null, default_value))
        })
}

fn archived_at_column(connection: &Connection) -> Option<(String, i32)> {
    list_column(connection, "archived_at").map(|(column_type, not_null, _)| (column_type, not_null))
}

fn is_default_column(connection: &Connection) -> Option<(String, i32, String)> {
    list_column(connection, "is_default")
}

fn index_exists(connection: &Connection, index_name: &str) -> bool {
    connection
        .query_row(
            "SELECT 1
                 FROM sqlite_master
                 WHERE type = 'index' AND name = ?1
                 LIMIT 1",
            [index_name],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_some()
}

fn setting_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
    let mut statement = connection.prepare("PRAGMA table_info(settings)").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .find_map(|(name, column_type, not_null)| {
            (name == target).then_some((column_type, not_null))
        })
}

fn reminder_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
    let mut statement = connection.prepare("PRAGMA table_info(reminders)").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .find_map(|(name, column_type, not_null)| {
            (name == target).then_some((column_type, not_null))
        })
}

fn sync_outbox_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
    let mut statement = connection
        .prepare("PRAGMA table_info(sync_outbox)")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .find_map(|(name, column_type, not_null)| {
            (name == target).then_some((column_type, not_null))
        })
}

fn sync_cursor_column(connection: &Connection, target: &str) -> Option<(String, i32)> {
    let mut statement = connection
        .prepare("PRAGMA table_info(sync_cursors)")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .find_map(|(name, column_type, not_null)| {
            (name == target).then_some((column_type, not_null))
        })
}

fn count_archived_at_columns(connection: &Connection) -> usize {
    let mut statement = connection.prepare("PRAGMA table_info(lists)").unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .into_iter()
        .filter(|column| column == "archived_at")
        .count()
}

fn schema_version(connection: &Connection) -> i32 {
    connection
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap()
}

#[derive(Clone, Copy)]
enum PerformanceSeedSchema {
    Latest,
    V3,
}

struct PerformanceSeed {
    list_ids: Vec<Uuid>,
    today_start_ms: i64,
    tomorrow_start_ms: i64,
    task_count: usize,
    due_task_count: usize,
    closed_task_count: usize,
}

fn seed_performance_database(
    path: &Path,
    key: &[u8; 32],
    schema: PerformanceSeedSchema,
) -> PerformanceSeed {
    match schema {
        PerformanceSeedSchema::Latest => {
            let mut connection = open_encrypted(path, key).unwrap();
            insert_performance_seed(&mut connection)
        }
        PerformanceSeedSchema::V3 => {
            create_v3_database(path, key);
            let mut connection = open_raw_encrypted(path, key);
            insert_performance_seed(&mut connection)
        }
    }
}

fn insert_performance_seed(connection: &mut Connection) -> PerformanceSeed {
    const LIST_COUNT: usize = 10;
    const TASKS_PER_LIST: usize = 1_000;
    const ROOT_TASKS_PER_LIST: usize = 700;
    const CHILD_TASKS_PER_LIST: usize = 220;

    let today_start_ms = 1_788_220_800_000;
    let tomorrow_start_ms = today_start_ms + 86_400_000;
    let mut list_ids = Vec::with_capacity(LIST_COUNT);
    let mut due_task_count = 0;
    let mut closed_task_count = 0;
    let transaction = connection.transaction().unwrap();

    {
        let mut insert_list = transaction
            .prepare(
                "INSERT INTO lists (
                        id, name, color, icon, sort_order, is_default,
                        archived_at, created_at, updated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
                    )",
            )
            .unwrap();
        for list_index in 0..LIST_COUNT {
            let id = Uuid::now_v7();
            list_ids.push(id);
            insert_list
                .execute(params![
                    id.to_string(),
                    format!("Performance List {}", list_index + 1),
                    "#4F8EF7",
                    "list",
                    format!("a{list_index:02}"),
                    list_index == 0,
                    Option::<i64>::None,
                    today_start_ms - 86_400_000,
                    today_start_ms - 86_400_000,
                ])
                .unwrap();
        }
    }

    {
        let mut insert_task = transaction
                .prepare(
                    "INSERT INTO tasks (
                        id, list_id, parent_task_id, title, note, status, priority,
                        due_kind, due_on, due_at_ms, due_time_zone, scheduled_at, estimated_minutes, sort_order,
                        completed_at, closed_reason, deleted_at, assignee,
                        created_at, updated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                    )",
                )
                .unwrap();

        for (list_index, list_id) in list_ids.iter().copied().enumerate() {
            let mut root_ids = Vec::with_capacity(ROOT_TASKS_PER_LIST);
            let mut child_ids = Vec::with_capacity(CHILD_TASKS_PER_LIST);
            for task_index in 0..TASKS_PER_LIST {
                let id = Uuid::now_v7();
                let parent_task_id = if task_index < ROOT_TASKS_PER_LIST {
                    root_ids.push(id);
                    None
                } else if task_index < ROOT_TASKS_PER_LIST + CHILD_TASKS_PER_LIST {
                    let parent_id = root_ids[(task_index - ROOT_TASKS_PER_LIST) % root_ids.len()];
                    child_ids.push(id);
                    Some(parent_id)
                } else {
                    Some(
                        child_ids[(task_index - ROOT_TASKS_PER_LIST - CHILD_TASKS_PER_LIST)
                            % child_ids.len()],
                    )
                };
                let global_index = (list_index * TASKS_PER_LIST) + task_index;
                let status = match global_index % 10 {
                    0 => "done",
                    1 => "wont_do",
                    2 | 3 => "in_progress",
                    _ => "todo",
                };
                let due_at = match global_index % 6 {
                    0 => None,
                    1 => Some(today_start_ms - 86_400_000),
                    2 => Some(today_start_ms + ((global_index % 12) as i64 * 3_600_000)),
                    3 => Some(tomorrow_start_ms + ((global_index % 8) as i64 * 3_600_000)),
                    4 => Some(tomorrow_start_ms + 7 * 86_400_000),
                    _ => None,
                };
                if due_at.is_some() {
                    due_task_count += 1;
                }
                let is_closed = status == "done" || status == "wont_do";
                let completed_at = if is_closed {
                    closed_task_count += 1;
                    if global_index.is_multiple_of(4) {
                        Some(today_start_ms + ((global_index % 10) as i64 * 600_000))
                    } else {
                        Some(today_start_ms - 2 * 86_400_000)
                    }
                } else {
                    None
                };
                let keyword = if global_index.is_multiple_of(17) {
                    "alpha"
                } else if global_index.is_multiple_of(19) {
                    "日本語"
                } else {
                    "routine"
                };

                insert_task
                    .execute(params![
                        id.to_string(),
                        list_id.to_string(),
                        parent_task_id.map(|parent_id| parent_id.to_string()),
                        format!("Task {global_index:05} {keyword}"),
                        format!("Seeded note {global_index:05} for {keyword} project"),
                        status,
                        (global_index % 4) as i32,
                        due_at.map(|_| "datetime"),
                        Option::<String>::None,
                        due_at,
                        due_at.map(|_| "UTC"),
                        due_at.map(|value| value - 3_600_000),
                        Some(15 + (global_index % 6) as i32 * 10),
                        format!("a{task_index:04}"),
                        completed_at,
                        (status == "wont_do").then_some("not_now".to_string()),
                        Option::<i64>::None,
                        Option::<String>::None,
                        today_start_ms - 86_400_000 + global_index as i64,
                        today_start_ms - 43_200_000 + global_index as i64,
                    ])
                    .unwrap();
            }
        }
    }

    transaction.commit().unwrap();

    PerformanceSeed {
        list_ids,
        today_start_ms,
        tomorrow_start_ms,
        task_count: LIST_COUNT * TASKS_PER_LIST,
        due_task_count,
        closed_task_count,
    }
}

fn default_list_ids(connection: &Connection) -> Vec<String> {
    let mut statement = connection
        .prepare("SELECT id FROM lists WHERE is_default = 1 ORDER BY id ASC")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn failing_archived_at_migration(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "ALTER TABLE lists ADD COLUMN archived_at INTEGER NULL;
             SELECT value FROM missing_failure_injection_table;",
    )
}

mod database;
mod repositories;
mod sync_and_reminders;
mod transactions_and_resync;
