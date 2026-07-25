use crate::*;

pub(super) fn get_list_on(connection: &Connection, id: Uuid) -> Result<List, StorageError> {
    let list = connection
        .query_row(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE id = ?1",
            [id.to_string()],
            row_to_list,
        )
        .optional()?;
    list.ok_or(StorageError::NotFound(id))
}

pub(super) fn get_default_list_on(connection: &Connection) -> Result<Option<List>, StorageError> {
    connection
        .query_row(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE is_default = 1
             LIMIT 1",
            [],
            row_to_list,
        )
        .optional()
        .map_err(StorageError::from)
}

pub(super) fn insert_list_on(connection: &Connection, list: &List) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO lists (
            id, name, color, icon, sort_order, is_default, archived_at,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
        )",
        params![
            list.id.to_string(),
            list.name,
            list.color,
            list.icon,
            list.sort_order,
            list.is_default,
            list.archived_at,
            list.created_at,
            list.updated_at,
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_list_for_sync_on(
    connection: &Connection,
    list: List,
) -> Result<(), StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM lists WHERE id = ?1",
            [list.id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        update_list_on(connection, &list)
    } else {
        insert_list_on(connection, &list)
    }
}

pub(super) fn delete_list_and_rehome_tasks_for_sync_on(
    connection: &Connection,
    list_id: Uuid,
) -> Result<usize, StorageError> {
    let list = get_list_on(connection, list_id)?;
    if list.is_default {
        return Err(StorageError::DefaultListProtected {
            operation: "deleted",
            list_id,
        });
    }
    let default_list = get_default_list_on(connection)?.ok_or_else(|| {
        StorageError::IncompatibleSchema(
            "Tenant must have a default Inbox before deleting a List".to_string(),
        )
    })?;
    let task_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM tasks WHERE list_id = ?1",
            [list_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    connection.execute(
        "UPDATE tasks SET list_id = ?2 WHERE list_id = ?1",
        params![list_id.to_string(), default_list.id.to_string()],
    )?;
    connection.execute(
        "DELETE FROM list_aliases
         WHERE alias_list_id = ?1 OR canonical_list_id = ?1",
        [list_id.to_string()],
    )?;
    connection.execute("DELETE FROM lists WHERE id = ?1", [list_id.to_string()])?;
    usize::try_from(task_count)
        .map_err(|_| StorageError::IncompatibleSchema("list task count exceeded usize".to_string()))
}

pub(super) fn update_list_on(connection: &Connection, list: &List) -> Result<(), StorageError> {
    if list.is_default && list.archived_at.is_some() {
        return Err(StorageError::DefaultListProtected {
            operation: "archived",
            list_id: list.id,
        });
    }
    let changed = connection.execute(
        "UPDATE lists
         SET name = ?2,
             color = ?3,
             icon = ?4,
             sort_order = ?5,
             is_default = ?6,
             archived_at = ?7,
             created_at = ?8,
             updated_at = ?9
         WHERE id = ?1",
        params![
            list.id.to_string(),
            list.name,
            list.color,
            list.icon,
            list.sort_order,
            list.is_default,
            list.archived_at,
            list.created_at,
            list.updated_at,
        ],
    )?;
    if changed == 0 {
        return Err(StorageError::NotFound(list.id));
    }
    Ok(())
}

/// SQLite-backed implementation of [`ListRepository`].
pub struct SqliteListRepository {
    connection: Connection,
}

impl SqliteListRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Raw sync-facing lookup that intentionally does not resolve a durable
    /// alias. The authenticated list record keeps its own identity.
    pub fn get_raw_for_sync(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(&self.connection, id)
    }

    /// Raw sync-facing enumeration including archived and aliased list rows.
    pub fn list_all_for_sync(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?;
        Ok(lists)
    }

    pub fn upsert_for_sync(&mut self, list: List) -> Result<(), StorageError> {
        upsert_list_for_sync_on(&self.connection, list)
    }

    pub fn delete_and_rehome_tasks_for_sync(
        &mut self,
        list_id: Uuid,
    ) -> Result<usize, StorageError> {
        let transaction = self.connection.transaction()?;
        let task_count = delete_list_and_rehome_tasks_for_sync_on(&transaction, list_id)?;
        transaction.commit()?;
        Ok(task_count)
    }
}

impl ListRepository for SqliteListRepository {
    fn get(&self, id: Uuid) -> Result<List, StorageError> {
        get_list_on(
            &self.connection,
            resolve_list_alias_on(&self.connection, id)?,
        )
    }

    fn insert(&mut self, list: List) -> Result<(), StorageError> {
        insert_list_on(&self.connection, &list)
    }

    fn update(&mut self, list: List) -> Result<(), StorageError> {
        update_list_on(&self.connection, &list)
    }

    fn list_all(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE archived_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
               )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(lists)
    }

    fn list_archived(&self) -> Result<Vec<List>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, color, icon, sort_order, archived_at,
                    is_default, created_at, updated_at
             FROM lists
             WHERE archived_at IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM list_aliases alias WHERE alias.alias_list_id = lists.id
               )
             ORDER BY sort_order ASC, id ASC",
        )?;
        let lists = statement
            .query_map([], row_to_list)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(lists)
    }

    fn get_default(&self) -> Result<Option<List>, StorageError> {
        get_default_list_on(&self.connection)
    }

    fn ensure_default_list(&mut self, name: String, now_ms: i64) -> Result<List, StorageError> {
        if let Some(list) = self.get_default()? {
            return Ok(list);
        }

        let last_rank: Option<String> =
            self.connection
                .query_row("SELECT max(sort_order) FROM lists", [], |row| row.get(0))?;
        let sort_order = fractional_index_after(last_rank.as_deref())
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        let list = new_default_list(name, sort_order, now_ms)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        self.insert(list.clone())?;
        Ok(list)
    }

    fn count_tasks(&self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = resolve_list_alias_on(&self.connection, list_id)?;
        let count: i64 = self.connection.query_row(
            "SELECT count(*) FROM tasks WHERE list_id = ?1",
            [list_id.to_string()],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| {
            StorageError::IncompatibleSchema("list task count exceeded usize".to_string())
        })
    }

    fn delete_and_rehome_tasks(&mut self, list_id: Uuid) -> Result<usize, StorageError> {
        let list_id = resolve_list_alias_on(&self.connection, list_id)?;
        let list = get_list_on(&self.connection, list_id)?;
        if list.is_default {
            return Err(StorageError::DefaultListProtected {
                operation: "deleted",
                list_id,
            });
        }
        let transaction = self.connection.transaction()?;
        let task_count = delete_list_and_rehome_tasks_for_sync_on(&transaction, list_id)?;
        transaction.commit()?;
        Ok(task_count)
    }
}
