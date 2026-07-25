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

pub(super) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sql: include_str!("../migrations/0001_initial.sql"),
}];

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
    validate_database_key(connection)?;
    validate_manifest(migrations)?;

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

fn validate_manifest(migrations: &[Migration]) -> Result<(), StorageError> {
    for (expected, migration) in (1_i32..).zip(migrations) {
        if migration.version != expected {
            return Err(StorageError::IncompatibleSchema(format!(
                "migration manifest is missing version {expected}"
            )));
        }
    }
    if migrations.last().map(|migration| migration.version) != Some(LATEST_MIGRATION_VERSION) {
        return Err(StorageError::IncompatibleSchema(format!(
            "migration manifest does not end at version {LATEST_MIGRATION_VERSION}"
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

        assert_eq!(ledger_count(&connection), 1);
        let (version, name, checksum_length): (i32, String, i64) = connection
            .query_row(
                "SELECT version, name, length(checksum)
                 FROM _taskveil_migrations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(version, LATEST_MIGRATION_VERSION);
        assert_eq!(name, "initial");
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

        assert_eq!(ledger_count(&connection), 1);
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
        assert_eq!(ledger_count(&open_raw(file.path())), 1);
    }

    #[test]
    fn unknown_newer_migration_is_rejected() {
        let file = NamedTempFile::new().unwrap();
        drop(open_encrypted(file.path(), &KEY).unwrap());
        let connection = open_raw(file.path());
        connection
            .execute(
                "INSERT INTO _taskveil_migrations (version, name, checksum)
                 VALUES (2, 'future', x'00')",
                [],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_encrypted(file.path(), &KEY),
            Err(StorageError::UnsupportedMigrationVersion {
                found: 2,
                latest: 1
            })
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
            .execute(
                "UPDATE _taskveil_migrations SET version = 2 WHERE version = 1",
                [],
            )
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
        let failing = [Migration {
            version: 1,
            name: "failing",
            sql: "CREATE TABLE partial (id INTEGER);
                  SELECT value FROM missing_failure_injection_table;",
        }];

        let result = ensure_schema(&mut connection, &failing);

        assert!(matches!(
            result,
            Err(StorageError::MigrationFailed {
                target_version: 1,
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

        assert_eq!(ledger_count(&open_raw(&path)), 1);
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
}
