use crate::*;

/// SQLite-backed template and Task Series repository.
pub struct SqliteTemplateSeriesRepository {
    connection: Connection,
}

impl SqliteTemplateSeriesRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn upsert_template_for_sync(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.connection, &template)
    }

    pub fn upsert_series_for_sync(&mut self, series: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.connection, &series)
    }
}

impl TemplateSeriesRepository for SqliteTemplateSeriesRepository {
    fn get_template(&self, id: Uuid) -> Result<TaskTemplate, StorageError> {
        get_template_on(&self.connection, id)
    }

    fn list_templates(&self) -> Result<Vec<TaskTemplate>, StorageError> {
        list_templates_on(&self.connection)
    }

    fn upsert_template(&mut self, template: TaskTemplate) -> Result<(), StorageError> {
        upsert_template_on(&self.connection, &template)
    }

    fn delete_template(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_template_on(&self.connection, id)
    }

    fn get_series(&self, id: Uuid) -> Result<TaskSeries, StorageError> {
        get_series_on(&self.connection, id)
    }

    fn list_series(&self) -> Result<Vec<TaskSeries>, StorageError> {
        list_series_on(&self.connection)
    }

    fn list_due_series(&self, now_ms: i64) -> Result<Vec<TaskSeries>, StorageError> {
        list_due_series_on(&self.connection, now_ms)
    }

    fn upsert_series(&mut self, series: TaskSeries) -> Result<(), StorageError> {
        upsert_series_on(&self.connection, &series)
    }

    fn delete_series(&mut self, id: Uuid) -> Result<bool, StorageError> {
        delete_series_on(&self.connection, id)
    }
}

const TEMPLATE_SELECT: &str =
    "SELECT id, name, default_list_id, blueprint_json, blueprint_revision,
            created_at, updated_at FROM templates";
const SERIES_SELECT: &str =
    "SELECT id, blueprint_json, target_list_id, rrule, starts_at, time_zone,
            next_run_at, enabled, config_revision, config_parent_revision,
            config_effective_from, lineage_json, created_at, updated_at
       FROM task_series";

pub(super) fn get_template_on(
    connection: &Connection,
    id: Uuid,
) -> Result<TaskTemplate, StorageError> {
    connection
        .query_row(
            &format!("{TEMPLATE_SELECT} WHERE id = ?1"),
            [id.to_string()],
            row_to_template,
        )
        .optional()?
        .ok_or(StorageError::NotFound(id))
}

pub(super) fn list_templates_on(
    connection: &Connection,
) -> Result<Vec<TaskTemplate>, StorageError> {
    let mut statement = connection.prepare(&format!(
        "{TEMPLATE_SELECT} ORDER BY updated_at DESC, id ASC"
    ))?;
    let templates = statement
        .query_map([], row_to_template)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(templates)
}

pub(super) fn upsert_template_on(
    connection: &Connection,
    template: &TaskTemplate,
) -> Result<(), StorageError> {
    template.validate()?;
    let blueprint_json = serde_json::to_string(&template.blueprint)?;
    connection.execute(
        "INSERT INTO templates (
             id, name, default_list_id, blueprint_json, blueprint_revision,
             created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name,
             default_list_id = excluded.default_list_id,
             blueprint_json = excluded.blueprint_json,
             blueprint_revision = excluded.blueprint_revision,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            template.id.to_string(),
            template.name,
            template.default_list_id.map(|id| id.to_string()),
            blueprint_json,
            template.blueprint_revision,
            template.created_at,
            template.updated_at,
        ],
    )?;
    Ok(())
}

pub(super) fn delete_template_on(connection: &Connection, id: Uuid) -> Result<bool, StorageError> {
    Ok(connection.execute("DELETE FROM templates WHERE id = ?1", [id.to_string()])? == 1)
}

pub(super) fn get_series_on(connection: &Connection, id: Uuid) -> Result<TaskSeries, StorageError> {
    connection
        .query_row(
            &format!("{SERIES_SELECT} WHERE id = ?1"),
            [id.to_string()],
            row_to_series,
        )
        .optional()?
        .ok_or(StorageError::NotFound(id))
}

pub(super) fn list_series_on(connection: &Connection) -> Result<Vec<TaskSeries>, StorageError> {
    list_series_query_on(
        connection,
        &format!("{SERIES_SELECT} ORDER BY updated_at DESC, id ASC"),
        [],
    )
}

pub(super) fn list_due_series_on(
    connection: &Connection,
    now_ms: i64,
) -> Result<Vec<TaskSeries>, StorageError> {
    list_series_query_on(
        connection,
        &format!(
            "{SERIES_SELECT}
             WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at, id"
        ),
        [now_ms],
    )
}

fn list_series_query_on<P>(
    connection: &Connection,
    query: &str,
    parameters: P,
) -> Result<Vec<TaskSeries>, StorageError>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(query)?;
    let series = statement
        .query_map(parameters, row_to_series)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(series)
}

pub(super) fn upsert_series_on(
    connection: &Connection,
    series: &TaskSeries,
) -> Result<(), StorageError> {
    series.validate()?;
    let blueprint_json = serde_json::to_string(&series.config.blueprint)?;
    let lineage_json = serde_json::to_string(&series.config.lineage)?;
    connection.execute(
        "INSERT INTO task_series (
             id, blueprint_json, target_list_id, rrule, starts_at, time_zone,
             next_run_at, enabled, config_revision, config_parent_revision,
             config_effective_from, lineage_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(id) DO UPDATE SET
             blueprint_json = excluded.blueprint_json,
             target_list_id = excluded.target_list_id,
             rrule = excluded.rrule,
             starts_at = excluded.starts_at,
             time_zone = excluded.time_zone,
             next_run_at = excluded.next_run_at,
             enabled = excluded.enabled,
             config_revision = excluded.config_revision,
             config_parent_revision = excluded.config_parent_revision,
             config_effective_from = excluded.config_effective_from,
             lineage_json = excluded.lineage_json,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            series.id.to_string(),
            blueprint_json,
            series.config.target_list_id.map(|id| id.to_string()),
            series.config.rrule,
            series.config.starts_at,
            series.config.time_zone,
            series.cursor.next_run_at(),
            series.config.enabled,
            series.config.config_revision,
            series.config.config_parent_revision,
            series.config.config_effective_from,
            lineage_json,
            series.created_at,
            series.updated_at,
        ],
    )?;
    Ok(())
}

pub(super) fn delete_series_on(connection: &Connection, id: Uuid) -> Result<bool, StorageError> {
    Ok(connection.execute("DELETE FROM task_series WHERE id = ?1", [id.to_string()])? == 1)
}

fn row_to_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTemplate> {
    let id: String = row.get(0)?;
    let default_list_id: Option<String> = row.get(2)?;
    let blueprint_json: String = row.get(3)?;
    let template = TaskTemplate {
        id: parse_uuid(id, 0)?,
        name: row.get(1)?,
        default_list_id: parse_optional_uuid(default_list_id, 2)?,
        blueprint: serde_json::from_str(&blueprint_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        blueprint_revision: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    };
    template.validate().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(template)
}

fn row_to_series(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSeries> {
    let id: String = row.get(0)?;
    let blueprint_json: String = row.get(1)?;
    let target_list_id: Option<String> = row.get(2)?;
    let next_run_at: Option<i64> = row.get(6)?;
    let lineage_json: String = row.get(11)?;
    let series = TaskSeries {
        id: parse_uuid(id, 0)?,
        config: TaskSeriesConfig {
            blueprint: serde_json::from_str(&blueprint_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            target_list_id: parse_optional_uuid(target_list_id, 2)?,
            rrule: row.get(3)?,
            starts_at: row.get(4)?,
            time_zone: row.get(5)?,
            enabled: row.get(7)?,
            config_revision: row.get(8)?,
            config_parent_revision: row.get(9)?,
            config_effective_from: row.get(10)?,
            lineage: serde_json::from_str(&lineage_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        },
        cursor: next_run_at.map_or(SeriesCursor::Exhausted, SeriesCursor::Pending),
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    };
    series.validate().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(series)
}
