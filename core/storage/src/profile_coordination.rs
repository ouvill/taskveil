use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileRuntimeState {
    pub runtime_epoch: i64,
    pub capsule_generation: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncLease {
    pub owner_id: String,
    pub expires_at_ms: i64,
    pub fencing_token: i64,
    pub runtime_epoch: i64,
}

pub struct SqliteProfileCoordinationRepository {
    connection: Connection,
}

impl SqliteProfileCoordinationRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn load_runtime(&self) -> Result<ProfileRuntimeState, StorageError> {
        load_runtime_on(&self.connection)
    }

    pub fn assert_runtime_epoch(&self, expected: i64) -> Result<(), StorageError> {
        assert_runtime_epoch_on(&self.connection, expected)
    }

    pub fn bump_runtime_epoch(&mut self, now_ms: i64) -> Result<ProfileRuntimeState, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = bump_runtime_epoch_on(&transaction, now_ms)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn publish_capsule_generation(
        &mut self,
        generation: i64,
        now_ms: i64,
    ) -> Result<ProfileRuntimeState, StorageError> {
        if generation <= 0 {
            return Err(StorageError::IncompatibleSchema(
                "capsule generation must be positive".to_string(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_runtime_on(&transaction)?;
        ensure_monotonic_time(current.updated_at, now_ms)?;
        if generation < current.capsule_generation {
            return Err(StorageError::IncompatibleSchema(
                "capsule generation cannot move backwards".to_string(),
            ));
        }
        if generation == current.capsule_generation {
            return Ok(current);
        }
        let next = current
            .runtime_epoch
            .checked_add(1)
            .ok_or(StorageError::ProfileCoordinationOverflow)?;
        transaction.execute(
            "UPDATE local_profile_runtime
             SET runtime_epoch = ?1, capsule_generation = ?2, updated_at = ?3
             WHERE singleton = 1",
            params![next, generation, now_ms],
        )?;
        transaction.execute(
            "UPDATE sync_run_lease
             SET owner_id = NULL,
                 expires_at_ms = NULL,
                 runtime_epoch = ?1,
                 updated_at = ?2
             WHERE singleton = 1",
            params![next, now_ms],
        )?;
        transaction.commit()?;
        Ok(ProfileRuntimeState {
            runtime_epoch: next,
            capsule_generation: generation,
            updated_at: now_ms,
        })
    }

    pub fn acquire_sync_lease(
        &mut self,
        owner_id: &str,
        now_ms: i64,
        ttl_ms: i64,
        expected_runtime_epoch: i64,
    ) -> Result<SyncLease, StorageError> {
        validate_owner_and_ttl(owner_id, ttl_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        assert_runtime_epoch_on(&transaction, expected_runtime_epoch)?;
        let row = load_lease_row(&transaction)?;
        ensure_monotonic_time(row.updated_at, now_ms)?;
        if row
            .owner_id
            .as_deref()
            .is_some_and(|owner| owner != owner_id)
            && row.expires_at_ms.is_some_and(|expiry| expiry > now_ms)
        {
            return Err(StorageError::SyncLeaseBusy);
        }
        let fencing_token = if row.owner_id.as_deref() == Some(owner_id)
            && row.expires_at_ms.is_some_and(|expiry| expiry > now_ms)
        {
            row.fencing_token
        } else {
            row.fencing_token
                .checked_add(1)
                .ok_or(StorageError::ProfileCoordinationOverflow)?
        };
        let expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or(StorageError::ProfileCoordinationOverflow)?;
        transaction.execute(
            "UPDATE sync_run_lease
             SET owner_id = ?1,
                 expires_at_ms = ?2,
                 fencing_token = ?3,
                 runtime_epoch = ?4,
                 updated_at = ?5
             WHERE singleton = 1",
            params![
                owner_id,
                expires_at_ms,
                fencing_token,
                expected_runtime_epoch,
                now_ms
            ],
        )?;
        transaction.commit()?;
        Ok(SyncLease {
            owner_id: owner_id.to_string(),
            expires_at_ms,
            fencing_token,
            runtime_epoch: expected_runtime_epoch,
        })
    }

    pub fn renew_sync_lease(
        &mut self,
        lease: &SyncLease,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<SyncLease, StorageError> {
        validate_owner_and_ttl(&lease.owner_id, ttl_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = load_lease_row(&transaction)?;
        ensure_monotonic_time(row.updated_at, now_ms)?;
        assert_lease_row(&row, lease, now_ms)?;
        let expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or(StorageError::ProfileCoordinationOverflow)?;
        transaction.execute(
            "UPDATE sync_run_lease
             SET expires_at_ms = ?1, updated_at = ?2
             WHERE singleton = 1
               AND owner_id = ?3
               AND fencing_token = ?4
               AND runtime_epoch = ?5",
            params![
                expires_at_ms,
                now_ms,
                lease.owner_id,
                lease.fencing_token,
                lease.runtime_epoch
            ],
        )?;
        transaction.commit()?;
        Ok(SyncLease {
            expires_at_ms,
            ..lease.clone()
        })
    }

    pub fn assert_sync_lease(&self, lease: &SyncLease, now_ms: i64) -> Result<(), StorageError> {
        let row = load_lease_row(&self.connection)?;
        assert_lease_row(&row, lease, now_ms)
    }

    pub fn release_sync_lease(
        &mut self,
        lease: &SyncLease,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = load_lease_row(&transaction)?;
        ensure_monotonic_time(row.updated_at, now_ms)?;
        if row.owner_id.as_deref() != Some(lease.owner_id.as_str())
            || row.fencing_token != lease.fencing_token
            || row.runtime_epoch != lease.runtime_epoch
        {
            return Err(StorageError::SyncLeaseLost);
        }
        transaction.execute(
            "UPDATE sync_run_lease
             SET owner_id = NULL, expires_at_ms = NULL, updated_at = ?1
             WHERE singleton = 1",
            [now_ms],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

pub(crate) fn load_runtime_on(
    connection: &Connection,
) -> Result<ProfileRuntimeState, StorageError> {
    connection
        .query_row(
            "SELECT runtime_epoch, capsule_generation, updated_at
             FROM local_profile_runtime WHERE singleton = 1",
            [],
            |row| {
                Ok(ProfileRuntimeState {
                    runtime_epoch: row.get(0)?,
                    capsule_generation: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            },
        )
        .map_err(StorageError::from)
}

pub(crate) fn assert_runtime_epoch_on(
    connection: &Connection,
    expected: i64,
) -> Result<(), StorageError> {
    let actual = load_runtime_on(connection)?.runtime_epoch;
    if actual == expected {
        Ok(())
    } else {
        Err(StorageError::ProfileRuntimeEpochChanged { expected, actual })
    }
}

pub(crate) fn bump_runtime_epoch_on(
    connection: &Connection,
    now_ms: i64,
) -> Result<ProfileRuntimeState, StorageError> {
    let current = load_runtime_on(connection)?;
    ensure_monotonic_time(current.updated_at, now_ms)?;
    let next = current
        .runtime_epoch
        .checked_add(1)
        .ok_or(StorageError::ProfileCoordinationOverflow)?;
    connection.execute(
        "UPDATE local_profile_runtime
         SET runtime_epoch = ?1, updated_at = ?2
         WHERE singleton = 1",
        params![next, now_ms],
    )?;
    connection.execute(
        "UPDATE sync_run_lease
         SET owner_id = NULL,
             expires_at_ms = NULL,
             runtime_epoch = ?1,
             updated_at = ?2
         WHERE singleton = 1",
        params![next, now_ms],
    )?;
    Ok(ProfileRuntimeState {
        runtime_epoch: next,
        capsule_generation: current.capsule_generation,
        updated_at: now_ms,
    })
}

#[derive(Debug)]
struct LeaseRow {
    owner_id: Option<String>,
    expires_at_ms: Option<i64>,
    fencing_token: i64,
    runtime_epoch: i64,
    updated_at: i64,
}

fn load_lease_row(connection: &Connection) -> Result<LeaseRow, StorageError> {
    connection
        .query_row(
            "SELECT owner_id, expires_at_ms, fencing_token, runtime_epoch, updated_at
             FROM sync_run_lease WHERE singleton = 1",
            [],
            |row| {
                Ok(LeaseRow {
                    owner_id: row.get(0)?,
                    expires_at_ms: row.get(1)?,
                    fencing_token: row.get(2)?,
                    runtime_epoch: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(StorageError::from)
}

pub(crate) fn assert_sync_lease_on(
    connection: &Connection,
    lease: &SyncLease,
    now_ms: i64,
) -> Result<(), StorageError> {
    assert_lease_row(&load_lease_row(connection)?, lease, now_ms)
}

fn assert_lease_row(row: &LeaseRow, lease: &SyncLease, now_ms: i64) -> Result<(), StorageError> {
    if row.owner_id.as_deref() != Some(lease.owner_id.as_str())
        || row.fencing_token != lease.fencing_token
        || row.runtime_epoch != lease.runtime_epoch
        || row.expires_at_ms.is_none_or(|expiry| expiry <= now_ms)
    {
        return Err(StorageError::SyncLeaseLost);
    }
    Ok(())
}

fn validate_owner_and_ttl(owner_id: &str, ttl_ms: i64) -> Result<(), StorageError> {
    if owner_id.is_empty() || owner_id.len() > 128 || ttl_ms <= 0 {
        return Err(StorageError::IncompatibleSchema(
            "sync lease owner must contain 1-128 bytes and ttl must be positive".to_string(),
        ));
    }
    Ok(())
}

fn ensure_monotonic_time(previous: i64, now_ms: i64) -> Result<(), StorageError> {
    if now_ms < previous {
        Err(StorageError::ProfileCoordinationClockRollback)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const KEY: [u8; 32] = [0x91; 32];

    fn repository(temp: &TempDir) -> SqliteProfileCoordinationRepository {
        SqliteProfileCoordinationRepository::new(
            open_encrypted(&temp.path().join("profile.db"), &KEY).unwrap(),
        )
    }

    #[test]
    fn lease_contention_expiry_takeover_and_fencing() {
        let temp = TempDir::new().unwrap();
        let mut first = repository(&temp);
        let lease = first.acquire_sync_lease("first", 100, 50, 1).unwrap();
        assert!(matches!(
            repository(&temp).acquire_sync_lease("second", 120, 50, 1),
            Err(StorageError::SyncLeaseBusy)
        ));
        let takeover = repository(&temp)
            .acquire_sync_lease("second", 150, 50, 1)
            .unwrap();
        assert!(takeover.fencing_token > lease.fencing_token);
        assert!(matches!(
            first.assert_sync_lease(&lease, 151),
            Err(StorageError::SyncLeaseLost)
        ));
    }

    #[test]
    fn epoch_change_revokes_active_lease() {
        let temp = TempDir::new().unwrap();
        let lease = repository(&temp)
            .acquire_sync_lease("owner", 100, 50, 1)
            .unwrap();
        let runtime = repository(&temp).bump_runtime_epoch(110).unwrap();
        assert_eq!(runtime.runtime_epoch, 2);
        assert!(matches!(
            repository(&temp).assert_sync_lease(&lease, 111),
            Err(StorageError::SyncLeaseLost)
        ));
    }

    #[test]
    fn clock_rollback_and_fence_overflow_fail_closed() {
        let temp = TempDir::new().unwrap();
        repository(&temp)
            .acquire_sync_lease("owner", 100, 50, 1)
            .unwrap();
        assert!(matches!(
            repository(&temp).acquire_sync_lease("owner", 99, 50, 1),
            Err(StorageError::ProfileCoordinationClockRollback)
        ));
        let connection = open_encrypted(&temp.path().join("profile.db"), &KEY).unwrap();
        connection
            .execute(
                "UPDATE sync_run_lease
                 SET owner_id = NULL, expires_at_ms = NULL, fencing_token = ?1, updated_at = 100
                 WHERE singleton = 1",
                [i64::MAX],
            )
            .unwrap();
        assert!(matches!(
            repository(&temp).acquire_sync_lease("next", 100, 50, 1),
            Err(StorageError::ProfileCoordinationOverflow)
        ));
    }
}
