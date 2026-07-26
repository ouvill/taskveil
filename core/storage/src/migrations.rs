use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha384};

use super::{StorageError, LATEST_MIGRATION_VERSION};

const MIGRATION_TABLE: &str = "_taskveil_migrations";
const CREATE_MIGRATION_TABLE: &str = "
    CREATE TABLE _taskveil_migrations (
        version INTEGER PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        checksum BLOB NOT NULL,
        applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
";

pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "home_calendar_range_indexes",
        sql: include_str!("../migrations/0002_home_calendar_range_indexes.sql"),
    },
    Migration {
        version: 3,
        name: "resync_page_tokens",
        sql: include_str!("../migrations/0003_resync_page_tokens.sql"),
    },
    Migration {
        version: 4,
        name: "profile_coordination",
        sql: include_str!("../migrations/0004_profile_coordination.sql"),
    },
    Migration {
        version: 5,
        name: "settings_metadata_boundary",
        sql: include_str!("../migrations/0005_settings_metadata_boundary.sql"),
    },
    Migration {
        version: 5,
        name: "reminder_notification_reconciliation",
        sql: include_str!("../migrations/0005_reminder_notification_reconciliation.sql"),
    },
];

#[derive(Clone, Copy)]
pub(super) struct Migration {
    pub(super) version: i32,
    pub(super) name: &'static str,
    pub(super) sql: &'static str,
}

struct AppliedMigration {
    version: i32,
    name: String,
    checksum: Vec<u8>,
}

pub(super) fn ensure_schema(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<(), StorageError> {
    ensure_schema_at_version(connection, migrations, LATEST_MIGRATION_VERSION)
}

pub(super) fn ensure_schema_at_version(
    connection: &mut Connection,
    migrations: &[Migration],
    expected_latest_version: i32,
) -> Result<(), StorageError> {
    validate_database_key(connection)?;
    validate_manifest(migrations, expected_latest_version)?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_ledger = table_exists(&transaction, MIGRATION_TABLE)?;
    if !has_ledger {
        if has_untracked_schema(&transaction)? {
            return Err(StorageError::IncompatibleSchema(
                "database contains schema objects without a migration ledger".to_string(),
            ));
        }
        transaction.execute_batch(CREATE_MIGRATION_TABLE)?;
    }

    let applied = read_applied_migrations(&transaction)?;
    validate_applied_migrations(&applied, migrations)?;

    for migration in migrations.iter().skip(applied.len()) {
        transaction.execute_batch(migration.sql).map_err(|source| {
            StorageError::MigrationFailed {
                target_version: migration.version,
                migration: migration.name,
                source,
            }
        })?;
        transaction
            .execute(
                "INSERT INTO _taskveil_migrations (version, name, checksum)
                 VALUES (?1, ?2, ?3)",
                params![
                    migration.version,
                    migration.name,
                    migration_checksum(migration)
                ],
            )
            .map_err(|source| StorageError::MigrationFailed {
                target_version: migration.version,
                migration: migration.name,
                source,
            })?;
    }

    transaction.commit()?;
    Ok(())
}

fn validate_database_key(connection: &Connection) -> Result<(), StorageError> {
    connection
        .query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
        .map_err(|_| StorageError::InvalidDatabaseKey)
}

fn validate_manifest(
    migrations: &[Migration],
    expected_latest_version: i32,
) -> Result<(), StorageError> {
    for (expected, migration) in (1_i32..).zip(migrations) {
        if migration.version != expected {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration manifest is missing version {expected}"
            )));
        }
    }
    if migrations.last().map(|migration| migration.version) != Some(expected_latest_version) {
        return Err(StorageError::IncompatibleSchema(format!(
            "migration manifest does not end at version {expected_latest_version}"
        )));
    }
    Ok(())
}

fn table_exists(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT 1
             FROM sqlite_schema
             WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn has_untracked_schema(connection: &Connection) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
         )",
        [],
        |row| row.get(0),
    )
}

fn read_applied_migrations(connection: &Connection) -> Result<Vec<AppliedMigration>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT version, name, checksum
         FROM _taskveil_migrations
         ORDER BY version ASC",
    )?;
    let applied = statement
        .query_map([], |row| {
            Ok(AppliedMigration {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(applied)
}

fn validate_applied_migrations(
    applied: &[AppliedMigration],
    migrations: &[Migration],
) -> Result<(), StorageError> {
    for (index, applied_migration) in applied.iter().enumerate() {
        let expected_version = i32::try_from(index + 1).map_err(|_| {
            StorageError::IncompatibleSchema("migration ledger is too large".to_string())
        })?;
        if applied_migration.version != expected_version {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration ledger is missing version {expected_version}"
            )));
        }
        let Some(migration) = migrations.get(index) else {
            return Err(StorageError::UnsupportedMigrationVersion {
                found: applied_migration.version,
                latest: LATEST_MIGRATION_VERSION,
            });
        };
        if applied_migration.name != migration.name {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration name mismatch at version {}",
                migration.version
            )));
        }
        if applied_migration.checksum != migration_checksum(migration) {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration checksum mismatch at version {}",
                migration.version
            )));
        }
    }
    Ok(())
}

fn migration_checksum(migration: &Migration) -> Vec<u8> {
    Sha384::digest(migration.sql.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    use taskveil_domain::Uuid;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::{apply_sqlcipher_key, open_encrypted, LOCAL_DB_BUSY_TIMEOUT};

    const KEY: [u8; 32] = [0x31; 32];

    fn open_raw(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection.busy_timeout(LOCAL_DB_BUSY_TIMEOUT).unwrap();
        apply_sqlcipher_key(&connection, &KEY).unwrap();
        connection
    }

    fn ledger_count(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT count(*) FROM _taskveil_migrations", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn fresh_database_applies_embedded_migration_and_records_checksum() {
        let file = NamedTempFile::new().unwrap();

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
        let (version, name, checksum_length): (i32, String, i64) = connection
            .query_row(
                "SELECT version, name, length(checksum)
                 FROM _taskveil_migrations
                 WHERE version = ?1",
                [LATEST_MIGRATION_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(version, LATEST_MIGRATION_VERSION);
        assert_eq!(name, "reminder_notification_reconciliation");
        assert_eq!(checksum_length, 48);
        assert!(table_exists(&connection, "tasks").unwrap());
    }

    #[test]
    fn initial_migration_rejects_invalid_due_and_partial_series_provenance() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();

        let mixed_due = connection.execute(
            "INSERT INTO tasks (
                 id, list_id, title, note, status, priority,
                 due_kind, due_on, due_at_ms, due_time_zone,
                 sort_order, created_at, updated_at
             ) VALUES (
                 'mixed-due', 'list', 'title', '', 'todo', 0,
                 'date', '2026-07-26', 1, 'UTC',
                 'a0', 1, 1
             )",
            [],
        );
        assert!(matches!(
            mixed_due,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));

        let partial_series_provenance = connection.execute(
            "INSERT INTO tasks (
                 id, list_id, title, note, status, priority,
                 sort_order, created_at, updated_at, series_id
             ) VALUES (
                 'partial-series', 'list', 'title', '', 'todo', 0,
                 'a0', 1, 1, 'series'
             )",
            [],
        );
        assert!(matches!(
            partial_series_provenance,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn migration_directory_matches_embedded_manifest() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut actual = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".sql"))
            .collect::<Vec<_>>();
        actual.sort();
        let expected = MIGRATIONS
            .iter()
            .map(|migration| format!("{:04}_{}.sql", migration.version, migration.name))
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn version_one_database_upgrades_to_range_indexes_without_rewriting_history() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        ensure_schema_at_version(&mut connection, &MIGRATIONS[..1], 1).unwrap();
        assert!(index_exists(&connection, "idx_tasks_home_targets"));
        assert!(!index_exists(
            &connection,
            "idx_tasks_active_scheduled_range"
        ));

        ensure_schema(&mut connection, MIGRATIONS).unwrap();

        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
        assert!(!index_exists(&connection, "idx_tasks_home_targets"));
        for index in [
            "idx_tasks_active_date_due_range",
            "idx_tasks_active_datetime_due_range",
            "idx_tasks_active_scheduled_range",
            "idx_tasks_closed_completed_range",
        ] {
            assert!(index_exists(&connection, index), "{index} was not created");
        }
    }

    #[test]
    fn reopening_latest_database_does_not_reapply_migration() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let applied_at: String = connection
            .query_row(
                "SELECT applied_at FROM _taskveil_migrations WHERE version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();

        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT applied_at FROM _taskveil_migrations WHERE version = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            applied_at
        );
    }

    #[test]
    fn page_token_migration_clears_unresumable_legacy_full_resync_state() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute_batch(CREATE_MIGRATION_TABLE).unwrap();
        transaction.execute_batch(MIGRATIONS[0].sql).unwrap();
        transaction
            .execute(
                "INSERT INTO _taskveil_migrations (version, name, checksum)
                 VALUES (?1, ?2, ?3)",
                params![
                    MIGRATIONS[0].version,
                    MIGRATIONS[0].name,
                    migration_checksum(&MIGRATIONS[0])
                ],
            )
            .unwrap();
        let generation_id = Uuid::now_v7().to_string();
        transaction
            .execute(
                "INSERT INTO sync_full_resync_state (
                     singleton, generation_id, phase, base_seq, delta_cursor,
                     started_at, updated_at, continuity_generation
                 ) VALUES (1, ?1, 'base', 0, 0, 1, 1, 1)",
                [&generation_id],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO sync_full_resync_marks
                     (generation_id, collection, record_id)
                 VALUES (?1, 'tasks', ?2)",
                [&generation_id, &Uuid::now_v7().to_string()],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM sync_full_resync_state", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM sync_full_resync_marks", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn profile_coordination_migrates_v3_database_with_initialized_epoch_and_lease() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute_batch(CREATE_MIGRATION_TABLE).unwrap();
        for migration in &MIGRATIONS[..3] {
            transaction.execute_batch(migration.sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO _taskveil_migrations (version, name, checksum)
                     VALUES (?1, ?2, ?3)",
                    params![
                        migration.version,
                        migration.name,
                        migration_checksum(migration)
                    ],
                )
                .unwrap();
        }
        let tenant_id = Uuid::now_v7().to_string();
        transaction
            .execute(
                "INSERT INTO local_tenant_root_key_cache (
                     tenant_id, generation, wrapped_tenant_root_dek, updated_at
                 ) VALUES (?1, 7, x'0102', 10)",
                [&tenant_id],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let runtime: (i64, i64) = connection
            .query_row(
                "SELECT runtime_epoch, capsule_generation
                 FROM local_profile_runtime WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let lease: (Option<String>, i64, i64) = connection
            .query_row(
                "SELECT owner_id, fencing_token, runtime_epoch
                 FROM sync_run_lease WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(runtime, (1, 1));
        assert_eq!(lease, (None, 0, 1));
        assert_eq!(
            connection
                .query_row(
                    "SELECT generation, wrapped_tenant_root_dek
                     FROM local_tenant_root_key_cache
                     WHERE tenant_id = ?1",
                    [&tenant_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .unwrap(),
            (7, vec![1, 2])
        );
        connection
            .execute(
                "INSERT INTO local_tenant_root_key_cache (
                     tenant_id, generation, wrapped_tenant_root_dek, updated_at
                 ) VALUES (?1, 8, x'0304', 11)",
                [&tenant_id],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM local_tenant_root_key_cache
                     WHERE tenant_id = ?1",
                    [&tenant_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
    }

    #[test]
    fn settings_boundary_migrates_v3_values_without_exposing_unknown_keys() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute_batch(CREATE_MIGRATION_TABLE).unwrap();
        for migration in &MIGRATIONS[..3] {
            transaction.execute_batch(migration.sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO _taskveil_migrations (version, name, checksum)
                     VALUES (?1, ?2, ?3)",
                    params![
                        migration.version,
                        migration.name,
                        migration_checksum(migration)
                    ],
                )
                .unwrap();
        }
        for (key, value, updated_at) in [
            ("ui_mode", "advanced", 1),
            ("onboarding_completed", "1", 2),
            ("calendar_week_start", "monday", 3),
            ("timer_settings_v1", r#"{"version":1}"#, 4),
            ("timer_runtime_v1", r#"{"completedWorkCycles":2}"#, 5),
            ("sync_local_hlc", "encoded-hlc", 6),
            ("sync_server_url", "https://sync.example.com", 7),
            ("future_internal_marker", "preserved", 8),
        ] {
            transaction
                .execute(
                    "INSERT INTO settings (key, value, updated_at)
                     VALUES (?1, ?2, ?3)",
                    params![key, value, updated_at],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let app_values = {
            let mut statement = connection
                .prepare("SELECT key, value, updated_at FROM app_settings ORDER BY updated_at")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let internal_values = {
            let mut statement = connection
                .prepare("SELECT key, value, updated_at FROM internal_metadata ORDER BY updated_at")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        assert_eq!(
            app_values,
            vec![
                ("ui_mode".to_string(), "advanced".to_string(), 1),
                ("onboarding_completed".to_string(), "1".to_string(), 2),
                ("calendar_week_start".to_string(), "monday".to_string(), 3),
                (
                    "timer_settings_v1".to_string(),
                    r#"{"version":1}"#.to_string(),
                    4,
                ),
            ]
        );
        assert_eq!(
            internal_values,
            vec![
                (
                    "timer_runtime_v1".to_string(),
                    r#"{"completedWorkCycles":2}"#.to_string(),
                    5,
                ),
                ("sync_local_hlc".to_string(), "encoded-hlc".to_string(), 6),
                (
                    "sync_server_url".to_string(),
                    "https://sync.example.com".to_string(),
                    7,
                ),
                (
                    "future_internal_marker".to_string(),
                    "preserved".to_string(),
                    8,
                ),
            ]
        );
        assert!(!table_exists(&connection, "settings").unwrap());
        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
    }

    #[test]
    fn reminder_notification_reconciliation_migrates_v4_reminders_with_stable_mapping() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute_batch(CREATE_MIGRATION_TABLE).unwrap();
        for migration in &MIGRATIONS[..4] {
            transaction.execute_batch(migration.sql).unwrap();
            transaction
                .execute(
                    "INSERT INTO _taskveil_migrations (version, name, checksum)
                     VALUES (?1, ?2, ?3)",
                    params![
                        migration.version,
                        migration.name,
                        migration_checksum(migration)
                    ],
                )
                .unwrap();
        }
        let list_id = Uuid::now_v7();
        let task_id = Uuid::now_v7();
        let reminder_id = Uuid::now_v7();
        transaction
            .execute(
                "INSERT INTO lists (
                     id, name, color, icon, sort_order, created_at, updated_at
                 ) VALUES (?1, 'Inbox', '', '', 'a0', 1, 1)",
                [list_id.to_string()],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO tasks (
                     id, list_id, title, note, status, priority, sort_order,
                     created_at, updated_at
                 ) VALUES (?1, ?2, '', '', 'todo', 0, 'a0', 1, 1)",
                params![task_id.to_string(), list_id.to_string()],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO reminders (
                     id, task_id, remind_at, snoozed_until, created_at
                 ) VALUES (?1, ?2, 1000, NULL, 1)",
                params![reminder_id.to_string(), task_id.to_string()],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_encrypted(file.path(), &KEY).unwrap();
        let (platform_id, revision): (i64, i64) = connection
            .query_row(
                "SELECT platform_id, command_revision
                 FROM reminder_notification_ids
                 WHERE reminder_id = ?1",
                [reminder_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(platform_id, 1);
        assert_eq!(revision, 1);
        assert_eq!(
            connection
                .query_row(
                    "SELECT action
                     FROM reminder_notification_commands
                     WHERE reminder_id = ?1",
                    [reminder_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "schedule"
        );
        assert_eq!(
            ledger_count(&connection),
            i64::from(LATEST_MIGRATION_VERSION)
        );
    }

    #[test]
    fn checksum_mismatch_is_rejected_without_database_changes() {
        let file = NamedTempFile::new().unwrap();
        drop(open_encrypted(file.path(), &KEY).unwrap());
        let connection = open_raw(file.path());
        connection
            .execute(
                "UPDATE _taskveil_migrations SET checksum = x'00' WHERE version = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let result = open_encrypted(file.path(), &KEY);

        assert!(matches!(
            result,
            Err(StorageError::IncompatibleSchema(message))
                if message == "migration checksum mismatch at version 1"
        ));
        assert_eq!(
            ledger_count(&open_raw(file.path())),
            i64::from(LATEST_MIGRATION_VERSION)
        );
    }

    #[test]
    fn unknown_newer_migration_is_rejected() {
        let file = NamedTempFile::new().unwrap();
        drop(open_encrypted(file.path(), &KEY).unwrap());
        let connection = open_raw(file.path());
        connection
            .execute(
                "INSERT INTO _taskveil_migrations (version, name, checksum)
                 VALUES (?1, 'future', x'00')",
                [LATEST_MIGRATION_VERSION + 1],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::UnsupportedMigrationVersion {
                found,
                latest
            }) if found == LATEST_MIGRATION_VERSION + 1
                && latest == LATEST_MIGRATION_VERSION
        ));
    }

    #[test]
    fn migration_name_mismatch_is_rejected() {
        let file = NamedTempFile::new().unwrap();
        drop(open_encrypted(file.path(), &KEY).unwrap());
        let connection = open_raw(file.path());
        connection
            .execute(
                "UPDATE _taskveil_migrations SET name = 'renamed' WHERE version = 1",
                [],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::IncompatibleSchema(message))
                if message == "migration name mismatch at version 1"
        ));
    }

    #[test]
    fn migration_ledger_gap_is_rejected() {
        let file = NamedTempFile::new().unwrap();
        drop(open_encrypted(file.path(), &KEY).unwrap());
        let connection = open_raw(file.path());
        connection
            .execute("DELETE FROM _taskveil_migrations WHERE version = 1", [])
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::IncompatibleSchema(message))
                if message == "migration ledger is missing version 1"
        ));
    }

    #[test]
    fn untracked_schema_is_rejected_instead_of_adopted() {
        let file = NamedTempFile::new().unwrap();
        let connection = open_raw(file.path());
        connection
            .execute_batch("CREATE TABLE legacy_data (id INTEGER PRIMARY KEY);")
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::IncompatibleSchema(message))
                if message == "database contains schema objects without a migration ledger"
        ));
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_ledger() {
        let file = NamedTempFile::new().unwrap();
        let mut connection = open_raw(file.path());
        let failing = [
            Migration {
                version: 1,
                name: "baseline",
                sql: "CREATE TABLE baseline (id INTEGER);",
            },
            Migration {
                version: 2,
                name: "middle",
                sql: "CREATE TABLE middle (id INTEGER);",
            },
            Migration {
                version: 3,
                name: "prelude",
                sql: "CREATE TABLE prelude (id INTEGER);",
            },
            Migration {
                version: 4,
                name: "boundary",
                sql: "CREATE TABLE boundary (id INTEGER);",
            },
            Migration {
                version: 5,
                name: "failing",
                sql: "CREATE TABLE partial (id INTEGER);
                      SELECT value FROM missing_failure_injection_table;",
            },
        ];

        let result = ensure_schema_at_version(&mut connection, &failing, 4);

        assert!(matches!(
            result,
            Err(StorageError::MigrationFailed {
                target_version: 5,
                migration: "failing",
                ..
            })
        ));
        assert!(!table_exists(&connection, "partial").unwrap());
        assert!(!table_exists(&connection, MIGRATION_TABLE).unwrap());
    }

    #[test]
    fn concurrent_initialization_is_serialized() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut connection = open_raw(&path);
                    barrier.wait();
                    ensure_schema(&mut connection, MIGRATIONS)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        assert_eq!(
            ledger_count(&open_raw(&path)),
            i64::from(LATEST_MIGRATION_VERSION)
        );
    }

    #[test]
    fn concurrent_initialization_respects_busy_timeout() {
        let file = NamedTempFile::new().unwrap();
        let mut first = open_raw(file.path());
        let first_transaction = first
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let path = file.path().to_path_buf();
        let handle = thread::spawn(move || {
            let mut second = open_raw(&path);
            ensure_schema(&mut second, MIGRATIONS)
        });
        thread::sleep(Duration::from_millis(50));
        first_transaction.rollback().unwrap();

        handle.join().unwrap().unwrap();
    }

    fn index_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS (
                     SELECT 1
                     FROM sqlite_schema
                     WHERE type = 'index' AND name = ?1
                 )",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }
}
