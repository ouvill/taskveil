use crate::*;

/// SQLite-backed implementation of [`SettingsRepository`].
pub struct SqliteSettingsRepository {
    connection: Connection,
}

impl SqliteSettingsRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl SettingsRepository for SqliteSettingsRepository {
    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_setting_on(&self.connection, key)
    }

    fn set_setting(&mut self, key: &str, value: &str, updated_at: i64) -> Result<(), StorageError> {
        set_setting_on(&self.connection, key, value, updated_at)
    }
}

pub(super) fn get_setting_on(
    connection: &Connection,
    key: &str,
) -> Result<Option<String>, StorageError> {
    connection
        .query_row(
            "SELECT value
             FROM settings
             WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
}

pub(super) fn set_setting_on(
    connection: &Connection,
    key: &str,
    value: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    connection.execute(
        "INSERT INTO settings (key, value, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
        params![key, value, updated_at],
    )?;
    Ok(())
}
