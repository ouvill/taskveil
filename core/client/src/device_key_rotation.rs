use std::path::Path;

use taskveil_crypto::{
    derive_local_db_key, LocalKeyCapsule, LocalKeyCapsuleSlot, LocalKeyCapsuleStore,
    PlatformLocalKeyCapsuleStore,
};
use taskveil_storage::{open_encrypted, rekey_encrypted_database, StorageError};
use zeroize::Zeroizing;

use crate::{
    runtime::{CryptoRuntimeState, TaskveilClient, ACCOUNT_USER_ID_SETTING_KEY},
    ClientError, Uuid,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceKeyRotationStep {
    PendingStored,
    DatabaseRekeyed,
    NewKeyVerified,
    ActivePromoted,
}

impl TaskveilClient {
    /// Rotates the local Device Key and SQLCipher key using the crash-safe
    /// active/pending capsule protocol. Returns the committed DK generation.
    pub fn rotate_device_key(&self) -> Result<u64, ClientError> {
        let _operation = self.begin_exclusive_operation()?;
        self.ensure_local_crypto_runtime_restored()?;

        let wrapping_material = {
            let account = self.account_state()?;
            match &account.crypto {
                CryptoRuntimeState::Anonymous => None,
                CryptoRuntimeState::Ready(crypto) => {
                    let user_id = self
                        .non_empty_setting(ACCOUNT_USER_ID_SETTING_KEY)?
                        .ok_or(ClientError::IncompleteAccountState)?
                        .parse::<Uuid>()
                        .map_err(|_| ClientError::IncompleteAccountState)?;
                    Some((user_id, Zeroizing::new(*crypto.master_key())))
                }
                CryptoRuntimeState::Unavailable(_) | CryptoRuntimeState::Unloaded => {
                    return Err(ClientError::AccountBoundUnavailable)
                }
            }
        };

        let mut store = PlatformLocalKeyCapsuleStore::new(&self.db_dir);
        let committed = rotate_device_key_with_store(
            &mut store,
            &self.db_path,
            |new_device_key| match wrapping_material {
                Some((user_id, master_key)) => {
                    taskveil_crypto::key_hierarchy::wrap_master_key_with_device_key(
                        user_id,
                        taskveil_crypto::key_hierarchy::INITIAL_KEY_GENERATION,
                        &master_key,
                        new_device_key,
                    )
                    .map(Some)
                    .map_err(|_| ClientError::LocalKeyState)
                }
                None => Ok(None),
            },
            |_| Ok(()),
        )?;
        let committed_db_key = Zeroizing::new(derive_local_db_key(committed.device_key()));
        self.replace_db_key(committed_db_key)?;
        let runtime = taskveil_storage::SqliteProfileCoordinationRepository::new(open_encrypted(
            &self.db_path,
            &self.db_key(),
        )?)
        .publish_capsule_generation(
            i64::try_from(committed.generation()).map_err(|_| ClientError::LocalKeyState)?,
            crate::runtime::now_ms()?,
        )?;
        self.publish_runtime_generation(runtime.runtime_epoch, committed.generation());
        Ok(committed.generation())
    }
}

pub(crate) fn resolve_active_capsule(
    store: &mut impl LocalKeyCapsuleStore,
    db_path: &Path,
) -> Result<LocalKeyCapsule, ClientError> {
    let active = store
        .load(LocalKeyCapsuleSlot::Active)
        .map_err(ClientError::KeyStore)?;
    let pending = store
        .load(LocalKeyCapsuleSlot::Pending)
        .map_err(ClientError::KeyStore)?;

    let active = match active {
        Some(active) => active,
        None if pending.is_some() || database_exists(db_path) => {
            return Err(ClientError::LocalKeyState)
        }
        None => {
            let active = LocalKeyCapsule::initial();
            store
                .store(LocalKeyCapsuleSlot::Active, &active)
                .map_err(ClientError::KeyStore)?;
            active
        }
    };

    let Some(pending) = pending else {
        // With no pending marker there is exactly one permitted open key. The
        // caller performs that open; this function must not try alternatives.
        return Ok(active);
    };
    if !database_exists(db_path) || pending.generation() <= active.generation() {
        if same_capsule_key(&active, &pending) && pending.generation() == active.generation() {
            store
                .delete(LocalKeyCapsuleSlot::Pending)
                .map_err(ClientError::KeyStore)?;
            return Ok(active);
        }
        return Err(ClientError::LocalKeyState);
    }

    match database_opens(db_path, active.device_key())? {
        true => {
            // Rekey never committed: discard only the pending capsule.
            store
                .delete(LocalKeyCapsuleSlot::Pending)
                .map_err(ClientError::KeyStore)?;
            Ok(active)
        }
        false if database_opens(db_path, pending.device_key())? => {
            // Rekey committed but promotion did not: finish the commit.
            store
                .store(LocalKeyCapsuleSlot::Active, &pending)
                .map_err(ClientError::KeyStore)?;
            store
                .delete(LocalKeyCapsuleSlot::Pending)
                .map_err(ClientError::KeyStore)?;
            Ok(pending)
        }
        false => Err(ClientError::LocalKeyRecoveryFailed),
    }
}

pub(crate) fn rotate_device_key_with_store(
    store: &mut impl LocalKeyCapsuleStore,
    db_path: &Path,
    wrapped_master_key_for: impl FnOnce(&[u8; 32]) -> Result<Option<Vec<u8>>, ClientError>,
    mut after_step: impl FnMut(DeviceKeyRotationStep) -> Result<(), ClientError>,
) -> Result<LocalKeyCapsule, ClientError> {
    let active = store
        .load(LocalKeyCapsuleSlot::Active)
        .map_err(ClientError::KeyStore)?
        .ok_or(ClientError::LocalKeyState)?;
    if store
        .load(LocalKeyCapsuleSlot::Pending)
        .map_err(ClientError::KeyStore)?
        .is_some()
    {
        return Err(ClientError::LocalKeyState);
    }
    let pending_without_wrap = active.next(None).map_err(ClientError::KeyStore)?;
    let wrapped_master_key = wrapped_master_key_for(pending_without_wrap.device_key())?;
    let pending = pending_without_wrap
        .with_wrapped_master_key(wrapped_master_key)
        .map_err(ClientError::KeyStore)?;

    store
        .store(LocalKeyCapsuleSlot::Pending, &pending)
        .map_err(ClientError::KeyStore)?;
    after_step(DeviceKeyRotationStep::PendingStored)?;

    let old_db_key = Zeroizing::new(derive_local_db_key(active.device_key()));
    let new_db_key = Zeroizing::new(derive_local_db_key(pending.device_key()));
    rekey_encrypted_database(db_path, &old_db_key, &new_db_key)?;
    after_step(DeviceKeyRotationStep::DatabaseRekeyed)?;

    if !database_opens(db_path, pending.device_key())?
        || database_opens(db_path, active.device_key())?
    {
        return Err(ClientError::LocalKeyRecoveryFailed);
    }
    after_step(DeviceKeyRotationStep::NewKeyVerified)?;

    store
        .store(LocalKeyCapsuleSlot::Active, &pending)
        .map_err(ClientError::KeyStore)?;
    after_step(DeviceKeyRotationStep::ActivePromoted)?;
    store
        .delete(LocalKeyCapsuleSlot::Pending)
        .map_err(ClientError::KeyStore)?;
    Ok(pending)
}

fn database_exists(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.len() > 0)
}

fn database_opens(path: &Path, device_key: &[u8; 32]) -> Result<bool, ClientError> {
    let db_key = Zeroizing::new(derive_local_db_key(device_key));
    match open_encrypted(path, &db_key) {
        Ok(connection) => {
            drop(connection);
            Ok(true)
        }
        Err(StorageError::InvalidDatabaseKey) => Ok(false),
        Err(error) => Err(ClientError::Storage(error)),
    }
}

fn same_capsule_key(left: &LocalKeyCapsule, right: &LocalKeyCapsule) -> bool {
    left.device_key() == right.device_key()
}

#[cfg(test)]
mod tests {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    use super::*;
    use taskveil_crypto::{InMemoryLocalKeyCapsuleStore, LocalKeyCapsuleStore};
    use tempfile::TempDir;

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    const CAPSULE_ROTATION_ROLE: &str = "TASKVEIL_CAPSULE_ROTATION_ROLE";
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    const CAPSULE_ROTATION_PROFILE: &str = "TASKVEIL_CAPSULE_ROTATION_PROFILE";
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    const CAPSULE_ROTATION_LIST: &str = "TASKVEIL_CAPSULE_ROTATION_LIST";

    fn fixture() -> (TempDir, std::path::PathBuf, InMemoryLocalKeyCapsuleStore) {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("taskveil.db");
        let mut store = InMemoryLocalKeyCapsuleStore::default();
        let active = resolve_active_capsule(&mut store, &db_path).unwrap();
        open_encrypted(&db_path, &derive_local_db_key(active.device_key())).unwrap();
        (temp, db_path, store)
    }

    #[test]
    fn every_rotation_failure_boundary_converges_after_restart() {
        for injected in [
            DeviceKeyRotationStep::PendingStored,
            DeviceKeyRotationStep::DatabaseRekeyed,
            DeviceKeyRotationStep::NewKeyVerified,
            DeviceKeyRotationStep::ActivePromoted,
        ] {
            let (_temp, db_path, mut store) = fixture();
            let result = rotate_device_key_with_store(
                &mut store,
                &db_path,
                |_| Ok(None),
                |step| {
                    if step == injected {
                        Err(ClientError::InjectedDeviceKeyRotationFailure)
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(matches!(
                result,
                Err(ClientError::InjectedDeviceKeyRotationFailure)
            ));

            let recovered = resolve_active_capsule(&mut store, &db_path).unwrap();
            assert!(store.load(LocalKeyCapsuleSlot::Pending).unwrap().is_none());
            assert!(open_encrypted(&db_path, &derive_local_db_key(recovered.device_key())).is_ok());
            let expected_generation = match injected {
                DeviceKeyRotationStep::PendingStored => 1,
                DeviceKeyRotationStep::DatabaseRekeyed
                | DeviceKeyRotationStep::NewKeyVerified
                | DeviceKeyRotationStep::ActivePromoted => 2,
            };
            assert_eq!(recovered.generation(), expected_generation);
        }
    }

    #[test]
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    fn shared_operation_recovers_pending_capsule_without_lock_upgrade_deadlock() {
        let temp = TempDir::new().unwrap();
        let client = TaskveilClient::open(crate::LocalProfileConfig::new(temp.path(), "Inbox"))
            .expect("open profile");
        let mut store = PlatformLocalKeyCapsuleStore::new(temp.path());

        let interrupted = rotate_device_key_with_store(
            &mut store,
            client.db_path(),
            |_| Ok(None),
            |step| {
                if step == DeviceKeyRotationStep::DatabaseRekeyed {
                    Err(ClientError::InjectedDeviceKeyRotationFailure)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            interrupted,
            Err(ClientError::InjectedDeviceKeyRotationFailure)
        ));
        assert!(store.load(LocalKeyCapsuleSlot::Pending).unwrap().is_some());

        // A normal read initially enters through the shared-operation path.
        // It must release that lock, perform bounded exclusive recovery, then
        // reacquire shared access before touching the rekeyed database.
        assert!(!client.get_lists().expect("recovered read").is_empty());
        assert!(store.load(LocalKeyCapsuleSlot::Pending).unwrap().is_none());
        assert_eq!(
            store
                .load(LocalKeyCapsuleSlot::Active)
                .unwrap()
                .unwrap()
                .generation(),
            2
        );
    }

    #[test]
    fn no_pending_capsule_means_no_compatibility_fallback() {
        let (_temp, db_path, mut store) = fixture();
        let active = store.load(LocalKeyCapsuleSlot::Active).unwrap().unwrap();
        let foreign = active.next(None).unwrap();
        rekey_encrypted_database(
            &db_path,
            &derive_local_db_key(active.device_key()),
            &derive_local_db_key(foreign.device_key()),
        )
        .unwrap();

        let resolved = resolve_active_capsule(&mut store, &db_path).unwrap();
        assert_eq!(resolved.generation(), active.generation());
        assert!(open_encrypted(&db_path, &derive_local_db_key(resolved.device_key())).is_err());
    }

    #[test]
    fn registered_rotation_rewraps_master_key_inside_the_new_capsule() {
        let (_temp, db_path, mut store) = fixture();
        let user_id = Uuid::now_v7();
        let master_key = Zeroizing::new([0x6d; 32]);
        let active = store.load(LocalKeyCapsuleSlot::Active).unwrap().unwrap();
        let old_wrap = taskveil_crypto::key_hierarchy::wrap_master_key_with_device_key(
            user_id,
            taskveil_crypto::key_hierarchy::INITIAL_KEY_GENERATION,
            &master_key,
            active.device_key(),
        )
        .unwrap();
        store
            .store(
                LocalKeyCapsuleSlot::Active,
                &active
                    .with_wrapped_master_key(Some(old_wrap.clone()))
                    .unwrap(),
            )
            .unwrap();

        let committed = rotate_device_key_with_store(
            &mut store,
            &db_path,
            |new_device_key| {
                taskveil_crypto::key_hierarchy::wrap_master_key_with_device_key(
                    user_id,
                    taskveil_crypto::key_hierarchy::INITIAL_KEY_GENERATION,
                    &master_key,
                    new_device_key,
                )
                .map(Some)
                .map_err(|_| ClientError::LocalKeyState)
            },
            |_| Ok(()),
        )
        .unwrap();

        let new_wrap = committed.wrapped_master_key().unwrap();
        assert_ne!(new_wrap, old_wrap);
        let unwrapped = taskveil_crypto::key_hierarchy::unwrap_master_key_with_device_key(
            user_id,
            taskveil_crypto::key_hierarchy::INITIAL_KEY_GENERATION,
            new_wrap,
            committed.device_key(),
        )
        .unwrap();
        assert_eq!(unwrapped, *master_key);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn real_process_capsule_rotation_reloads_stale_client() {
        match std::env::var(CAPSULE_ROTATION_ROLE).as_deref() {
            Ok("coordinator") => {
                run_capsule_rotation_coordinator();
                return;
            }
            Ok("stale-client") => {
                run_stale_capsule_client();
                return;
            }
            Ok(role) => panic!("unexpected capsule rotation test role: {role}"),
            Err(_) => {}
        }

        let temp = TempDir::new().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_key_rotation::tests::real_process_capsule_rotation_reloads_stale_client",
                "--nocapture",
            ])
            .env(CAPSULE_ROTATION_ROLE, "coordinator")
            .env(CAPSULE_ROTATION_PROFILE, temp.path())
            // Keep the complete two-process scenario on the profile-scoped
            // file test backend, including on unsigned macOS test binaries.
            .env("FLUTTER_TEST", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn run_capsule_rotation_coordinator() {
        let profile = std::path::PathBuf::from(std::env::var_os(CAPSULE_ROTATION_PROFILE).unwrap());
        let client = TaskveilClient::open(crate::LocalProfileConfig::new(&profile, "Inbox"))
            .expect("open rotation coordinator");
        let list_id = client.get_lists().unwrap()[0].id;
        let active = PlatformLocalKeyCapsuleStore::new(&profile)
            .load(LocalKeyCapsuleSlot::Active)
            .unwrap()
            .unwrap();
        let old_db_key = derive_local_db_key(active.device_key());

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_key_rotation::tests::real_process_capsule_rotation_reloads_stale_client",
                "--nocapture",
            ])
            .env(CAPSULE_ROTATION_ROLE, "stale-client")
            .env(CAPSULE_ROTATION_PROFILE, &profile)
            .env(CAPSULE_ROTATION_LIST, list_id.to_string())
            .env("FLUTTER_TEST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.contains("TASKVEIL_CAPSULE_ROTATION_READY") {
                break;
            }
        }

        assert_eq!(client.rotate_device_key().unwrap(), 2);
        assert!(open_encrypted(client.db_path(), &old_db_key).is_err());
        child.stdin.take().unwrap().write_all(b"rotated\n").unwrap();
        assert!(child.wait().unwrap().success());

        let tasks = client.get_tasks(list_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].content.title, "after capsule rotation");
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn run_stale_capsule_client() {
        let profile = std::path::PathBuf::from(std::env::var_os(CAPSULE_ROTATION_PROFILE).unwrap());
        let list_id = std::env::var(CAPSULE_ROTATION_LIST)
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let client = TaskveilClient::open(crate::LocalProfileConfig::new(&profile, "Inbox"))
            .expect("open stale capsule client");
        let active = PlatformLocalKeyCapsuleStore::new(&profile)
            .load(LocalKeyCapsuleSlot::Active)
            .unwrap()
            .unwrap();
        let old_db_key = derive_local_db_key(active.device_key());

        println!("TASKVEIL_CAPSULE_ROTATION_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();

        assert!(open_encrypted(client.db_path(), &old_db_key).is_err());
        assert_eq!(client.get_lists().unwrap()[0].id, list_id);
        client
            .create_task(crate::CreateTaskCommand {
                list_id,
                title: "after capsule rotation".into(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
            })
            .expect("stale client reloads the capsule and database key");
    }
}
