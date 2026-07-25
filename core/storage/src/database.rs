use crate::*;

pub(super) fn optional_not_found<T>(
    result: Result<T, StorageError>,
) -> Result<Option<T>, StorageError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Opens a SQLCipher encrypted SQLite database and migrates it to the latest schema.
pub fn open_encrypted(path: &Path, key: &[u8; 32]) -> Result<Connection, StorageError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(LOCAL_DB_BUSY_TIMEOUT)?;
    apply_sqlcipher_key(&connection, key)?;
    ensure_schema(&mut connection, MIGRATIONS)?;
    Ok(connection)
}

/// Rekeys an existing SQLCipher database and returns immediately after the
/// SQLCipher operation has completed.
///
/// SQLCipher performs `PRAGMA rekey` atomically at the database-file boundary.
/// The active/pending capsule coordinator in `taskveil-client` deliberately
/// performs new-key reopen verification as a separate crash boundary.
pub fn rekey_encrypted_database(
    path: &Path,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<(), StorageError> {
    if old_key == new_key {
        return Ok(());
    }

    let connection = open_encrypted(path, old_key)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let new_key_hex = hex::encode(new_key);
    connection.execute_batch(&format!("PRAGMA rekey = \"x'{new_key_hex}'\";"))?;
    drop(connection);
    Ok(())
}

pub(super) fn apply_sqlcipher_key(
    connection: &Connection,
    key: &[u8; 32],
) -> Result<(), StorageError> {
    let key_hex = hex::encode(key);
    connection.execute_batch(&format!("PRAGMA key = \"x'{key_hex}'\";"))?;
    Ok(())
}
