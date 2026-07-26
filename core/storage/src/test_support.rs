use rusqlite::{params, Connection};
use taskveil_domain::Uuid;

use crate::StorageError;

pub const HOME_CALENDAR_PERFORMANCE_TODAY_START_MS: i64 = 1_788_220_800_000;

/// Inserts a deterministic-shape 10,000-task fixture for cross-crate performance tests.
///
/// This is compiled only with the `test-support` feature. Product frontends must
/// exercise it through `taskveil-client`, preserving the client architecture boundary.
pub fn seed_home_calendar_performance_fixture(
    connection: &mut Connection,
) -> Result<usize, StorageError> {
    const TASK_COUNT: usize = 10_000;
    let today_start_ms = HOME_CALENDAR_PERFORMANCE_TODAY_START_MS;

    let list_id: String =
        connection.query_row("SELECT id FROM lists WHERE is_default = 1", [], |row| {
            row.get(0)
        })?;
    let transaction = connection.transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO tasks (
                 id, list_id, parent_task_id, title, note, status, priority,
                 due_kind, due_on, due_at_ms, due_time_zone, scheduled_at,
                 estimated_minutes, sort_order, completed_at, closed_reason,
                 deleted_at, assignee, created_at, updated_at
             ) VALUES (
                 ?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, NULL, NULL, ?16, ?17
             )",
        )?;
        for task_index in 0..TASK_COUNT {
            let status = match task_index % 10 {
                0 => "done",
                1 => "wont_do",
                2 | 3 => "in_progress",
                _ => "todo",
            };
            let due_at = match task_index % 6 {
                0 | 5 => None,
                1 => Some(today_start_ms - 86_400_000),
                2 => Some(today_start_ms + ((task_index % 12) as i64 * 3_600_000)),
                3 => Some(today_start_ms + 86_400_000),
                _ => Some(today_start_ms + 7 * 86_400_000),
            };
            let date_due = due_at.is_some() && task_index % 2 == 0;
            let due_kind = due_at.map(|_| if date_due { "date" } else { "datetime" });
            let due_on = date_due.then_some(match task_index % 6 {
                1 => "2026-08-31",
                2 => "2026-09-01",
                3 => "2026-09-02",
                _ => "2026-09-08",
            });
            let due_at_ms = (!date_due).then_some(due_at).flatten();
            let due_time_zone = due_at_ms.map(|_| "UTC");
            let scheduled_at = (status == "todo" || status == "in_progress")
                .then_some(due_at)
                .flatten()
                .map(|value| value - 3_600_000);
            let completed_at = match status {
                "done" | "wont_do" if task_index % 4 == 0 => {
                    Some(today_start_ms + ((task_index % 10) as i64 * 600_000))
                }
                "done" | "wont_do" => Some(today_start_ms - 2 * 86_400_000),
                _ => None,
            };

            insert.execute(params![
                Uuid::now_v7().to_string(),
                list_id,
                format!("FRB performance task {task_index:05}"),
                format!("10k SQLCipher bridge seed {task_index:05}"),
                status,
                (task_index % 4) as i32,
                due_kind,
                due_on,
                due_at_ms,
                due_time_zone,
                scheduled_at,
                30_i32,
                format!("a{task_index:05}"),
                completed_at,
                (status == "wont_do").then_some("not_now"),
                today_start_ms - 86_400_000 + task_index as i64,
                today_start_ms - 43_200_000 + task_index as i64,
            ])?;
        }
    }
    transaction.commit()?;
    Ok(TASK_COUNT)
}
