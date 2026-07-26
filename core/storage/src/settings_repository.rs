use crate::*;

/// Closed set of settings owned by product UI/application behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSettingKey {
    UiMode,
    OnboardingCompleted,
    CalendarWeekStart,
    TimerSettings,
}

impl AppSettingKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UiMode => "ui_mode",
            Self::OnboardingCompleted => "onboarding_completed",
            Self::CalendarWeekStart => "calendar_week_start",
            Self::TimerSettings => "timer_settings_v1",
        }
    }
}

/// SQLite-backed application preference repository.
pub struct SqliteAppSettingsRepository {
    connection: Connection,
}

impl SqliteAppSettingsRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl AppSettingsRepository for SqliteAppSettingsRepository {
    fn get_app_setting(&self, key: AppSettingKey) -> Result<Option<String>, StorageError> {
        get_app_setting_on(&self.connection, key)
    }

    fn set_app_setting(
        &mut self,
        key: AppSettingKey,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_app_setting_on(&self.connection, key, value, updated_at)
    }
}

/// SQLite-backed internal runtime metadata repository.
pub struct SqliteInternalMetadataRepository {
    connection: Connection,
}

impl SqliteInternalMetadataRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl InternalMetadataRepository for SqliteInternalMetadataRepository {
    fn get_internal_metadata(&self, key: &str) -> Result<Option<String>, StorageError> {
        get_internal_metadata_on(&self.connection, key)
    }

    fn set_internal_metadata(
        &mut self,
        key: &str,
        value: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        set_internal_metadata_on(&self.connection, key, value, updated_at)
    }
}

pub(super) fn get_app_setting_on(
    connection: &Connection,
    key: AppSettingKey,
) -> Result<Option<String>, StorageError> {
    get_value_on(connection, "app_settings", key.as_str())
}

pub(super) fn set_app_setting_on(
    connection: &Connection,
    key: AppSettingKey,
    value: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    set_value_on(connection, "app_settings", key.as_str(), value, updated_at)
}

pub(super) fn get_internal_metadata_on(
    connection: &Connection,
    key: &str,
) -> Result<Option<String>, StorageError> {
    get_value_on(connection, "internal_metadata", key)
}

pub(super) fn set_internal_metadata_on(
    connection: &Connection,
    key: &str,
    value: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    set_value_on(connection, "internal_metadata", key, value, updated_at)
}

fn get_value_on(
    connection: &Connection,
    table: &str,
    key: &str,
) -> Result<Option<String>, StorageError> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    connection
        .query_row(&sql, [key], |row| row.get(0))
        .optional()
        .map_err(StorageError::from)
}

fn set_value_on(
    connection: &Connection,
    table: &str,
    key: &str,
    value: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    let sql = format!(
        "INSERT INTO {table} (key, value, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at"
    );
    connection.execute(&sql, params![key, value, updated_at])?;
    Ok(())
}
