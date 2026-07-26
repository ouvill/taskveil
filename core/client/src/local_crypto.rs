use std::{collections::BTreeMap, path::Path};

use taskveil_crypto::key_hierarchy::{
    unwrap_local_tenant_root_dek_with_master_key, wrap_local_tenant_root_dek_with_master_key,
    KEY_LEN,
};
use taskveil_domain::Uuid;
use taskveil_storage::{
    open_encrypted, LocalCryptoRepository, LocalProfileBinding, LocalTenantRootKeyBundle,
    OwnedSqliteWriteTx, SqliteLocalCryptoRepository, StorageError,
};
#[cfg(any(test, feature = "test-support"))]
use taskveil_sync::account::AccountKeyMaterial;
use taskveil_sync::LocalSyncKeys;
use zeroize::Zeroizing;

use crate::LocalMutationContext;

pub enum LocalCryptoAvailability {
    Anonymous,
    Ready(Box<LocalCryptoContext>),
    AccountBoundUnavailable(LocalCryptoUnavailable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalCryptoIdentity {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCryptoUnavailable {
    MissingMasterKey,
    CorruptKeyCache,
    MissingTenantRootKey,
}

pub struct LocalCryptoContext {
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    master_key: Zeroizing<[u8; KEY_LEN]>,
    sync_keys: LocalSyncKeys,
}

impl LocalCryptoContext {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    pub fn device_id(&self) -> Uuid {
        self.device_id
    }

    pub fn master_key(&self) -> &[u8; KEY_LEN] {
        &self.master_key
    }

    pub fn sync_keys(&self) -> &LocalSyncKeys {
        &self.sync_keys
    }

    pub fn mutation_context(&self) -> LocalMutationContext {
        LocalMutationContext {
            device_id: self.device_id.to_string(),
            keys: self.sync_keys.clone(),
        }
    }
}

pub fn load_local_crypto_context(
    db_path: &Path,
    db_key: &[u8; 32],
    master_key: Option<[u8; KEY_LEN]>,
) -> Result<LocalCryptoAvailability, StorageError> {
    let connection = open_encrypted(db_path, db_key)?;
    let repository = SqliteLocalCryptoRepository::new(connection);
    let Some(binding) = repository.load_binding()? else {
        return Ok(LocalCryptoAvailability::Anonymous);
    };
    let Some(master_key) = master_key else {
        return Ok(LocalCryptoAvailability::AccountBoundUnavailable(
            LocalCryptoUnavailable::MissingMasterKey,
        ));
    };
    let mut tenant_roots = repository.load_tenant_roots(binding.tenant_id)?;
    let Some(tenant_root) = tenant_roots.pop() else {
        return Ok(LocalCryptoAvailability::AccountBoundUnavailable(
            LocalCryptoUnavailable::MissingTenantRootKey,
        ));
    };
    let sync_keys = match unwrap_local_cache_entries(
        binding.tenant_id,
        &tenant_root,
        &tenant_roots,
        &master_key,
    ) {
        Ok(keys) => keys,
        Err(_) => {
            return Ok(LocalCryptoAvailability::AccountBoundUnavailable(
                LocalCryptoUnavailable::CorruptKeyCache,
            ));
        }
    };

    Ok(LocalCryptoAvailability::Ready(Box::new(
        LocalCryptoContext {
            tenant_id: binding.tenant_id,
            user_id: binding.user_id,
            device_id: binding.device_id,
            master_key: Zeroizing::new(master_key),
            sync_keys,
        },
    )))
}

#[cfg(any(test, feature = "test-support"))]
pub fn persist_account_crypto_context(
    db_path: &Path,
    db_key: &[u8; 32],
    identity: LocalCryptoIdentity,
    keys: &AccountKeyMaterial,
    now_ms: i64,
) -> Result<LocalCryptoContext, StorageError> {
    persist_local_crypto_context(
        db_path,
        db_key,
        identity,
        &keys.master_key,
        LocalSyncKeys::from_account_keys(identity.tenant_id, keys),
        now_ms,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn persist_local_crypto_context(
    db_path: &Path,
    db_key: &[u8; 32],
    identity: LocalCryptoIdentity,
    master_key: &[u8; KEY_LEN],
    sync_keys: LocalSyncKeys,
    now_ms: i64,
) -> Result<LocalCryptoContext, StorageError> {
    let connection = open_encrypted(db_path, db_key)?;
    let mut transaction = OwnedSqliteWriteTx::begin(connection)?;
    let (context, _) = persist_local_crypto_context_in_transaction(
        &mut transaction,
        identity,
        master_key,
        sync_keys,
        now_ms,
    )?;
    transaction.commit()?;
    Ok(context)
}

pub(crate) fn persist_local_crypto_context_in_transaction(
    transaction: &mut OwnedSqliteWriteTx,
    identity: LocalCryptoIdentity,
    master_key: &[u8; KEY_LEN],
    sync_keys: LocalSyncKeys,
    now_ms: i64,
) -> Result<(LocalCryptoContext, bool), StorageError> {
    let active_tenant_root_dek = sync_keys.tenant_root_dek.as_deref().ok_or_else(|| {
        StorageError::IncompatibleSchema("local Tenant Root DEK is missing".to_string())
    })?;
    if sync_keys.tenant_generation == 0 {
        return Err(StorageError::IncompatibleSchema(
            "invalid active Tenant Root DEK generation".to_string(),
        ));
    }
    let existing_binding = transaction.load_local_crypto_binding()?;
    if let Some(existing) = &existing_binding {
        if existing.tenant_id != identity.tenant_id {
            return Err(StorageError::LocalProfileTenantMismatch {
                bound_tenant_id: existing.tenant_id,
                requested_tenant_id: identity.tenant_id,
            });
        }
        if existing.user_id != identity.user_id {
            return Err(StorageError::LocalProfileUserMismatch {
                bound_user_id: existing.user_id,
                requested_user_id: identity.user_id,
            });
        }
    }
    let existing_roots = transaction.load_tenant_roots(identity.tenant_id)?;
    if existing_binding.is_none() && !existing_roots.is_empty() {
        return Err(StorageError::IncompatibleSchema(
            "Tenant Root DEK cache exists without a profile binding".to_string(),
        ));
    }
    if existing_roots
        .last()
        .is_some_and(|root| root.generation > sync_keys.tenant_generation)
    {
        return Err(StorageError::IncompatibleSchema(
            "active Tenant Root DEK generation cannot move backwards".to_string(),
        ));
    }

    let mut existing_semantic_keys = BTreeMap::<u64, Zeroizing<[u8; KEY_LEN]>>::new();
    let mut existing_wrapped = BTreeMap::new();
    for root in existing_roots {
        let unwrapped = Zeroizing::new(
            unwrap_local_tenant_root_dek_with_master_key(
                identity.tenant_id,
                root.generation,
                &root.wrapped_tenant_root_dek,
                master_key,
            )
            .map_err(|_| {
                StorageError::IncompatibleSchema(
                    "stored local Tenant Root DEK cannot be authenticated".to_string(),
                )
            })?,
        );
        existing_semantic_keys.insert(root.generation, unwrapped);
        existing_wrapped.insert(root.generation, root);
    }
    let mut semantic_keys = BTreeMap::<u64, Zeroizing<[u8; KEY_LEN]>>::new();
    merge_semantic_tenant_key(
        &mut semantic_keys,
        sync_keys.tenant_generation,
        active_tenant_root_dek,
    )?;
    for (generation, key) in &sync_keys.historical_tenant_root_deks {
        if *generation >= sync_keys.tenant_generation {
            return Err(StorageError::IncompatibleSchema(
                "historical Tenant Root DEK generation must precede the active generation"
                    .to_string(),
            ));
        }
        merge_semantic_tenant_key(&mut semantic_keys, *generation, key)?;
    }

    let mut tenant_roots = Vec::with_capacity(semantic_keys.len());
    for (generation, key) in &semantic_keys {
        if existing_semantic_keys
            .get(generation)
            .is_some_and(|existing| existing.as_ref() != key.as_ref())
        {
            return Err(StorageError::IncompatibleSchema(
                "Tenant Root DEK changed without a generation change".to_string(),
            ));
        }
        if let Some(existing) = existing_wrapped.remove(generation) {
            tenant_roots.push(existing);
        } else {
            tenant_roots.push(LocalTenantRootKeyBundle {
                tenant_id: identity.tenant_id,
                generation: *generation,
                wrapped_tenant_root_dek: wrap_local_tenant_root_dek_with_master_key(
                    identity.tenant_id,
                    *generation,
                    key,
                    master_key,
                )
                .map_err(|_| {
                    StorageError::IncompatibleSchema(
                        "invalid local Tenant Root DEK material".to_string(),
                    )
                })?,
                updated_at: now_ms,
            });
        }
    }
    let binding = LocalProfileBinding {
        tenant_id: identity.tenant_id,
        user_id: identity.user_id,
        device_id: identity.device_id,
        bound_at: existing_binding
            .as_ref()
            .map_or(now_ms, |value| value.bound_at),
        updated_at: now_ms,
    };
    let runtime_changed = transaction.bind_tenant_roots(binding, &tenant_roots)?;
    let normalized_sync_keys = LocalSyncKeys {
        tenant_id: identity.tenant_id,
        tenant_root_dek: Some(Zeroizing::new(*active_tenant_root_dek)),
        tenant_generation: sync_keys.tenant_generation,
        historical_tenant_root_deks: semantic_keys
            .into_iter()
            .filter(|(generation, _)| *generation != sync_keys.tenant_generation)
            .collect(),
    };
    Ok((
        LocalCryptoContext {
            tenant_id: identity.tenant_id,
            user_id: identity.user_id,
            device_id: identity.device_id,
            master_key: Zeroizing::new(*master_key),
            sync_keys: normalized_sync_keys,
        },
        runtime_changed,
    ))
}

fn merge_semantic_tenant_key(
    keys: &mut BTreeMap<u64, Zeroizing<[u8; KEY_LEN]>>,
    generation: u64,
    key: &[u8; KEY_LEN],
) -> Result<(), StorageError> {
    if generation == 0 {
        return Err(StorageError::IncompatibleSchema(
            "invalid Tenant Root DEK generation".to_string(),
        ));
    }
    if let Some(existing) = keys.get(&generation) {
        if existing.as_ref() != key {
            return Err(StorageError::IncompatibleSchema(
                "Tenant Root DEK changed without a generation change".to_string(),
            ));
        }
        return Ok(());
    }
    keys.insert(generation, Zeroizing::new(*key));
    Ok(())
}

fn unwrap_local_cache_entries(
    tenant_id: Uuid,
    active: &LocalTenantRootKeyBundle,
    historical: &[LocalTenantRootKeyBundle],
    master_key: &[u8; KEY_LEN],
) -> Result<LocalSyncKeys, ()> {
    let tenant_root_dek = Zeroizing::new(
        unwrap_local_tenant_root_dek_with_master_key(
            tenant_id,
            active.generation,
            &active.wrapped_tenant_root_dek,
            master_key,
        )
        .map_err(|_| ())?,
    );
    let historical_tenant_root_deks = historical
        .iter()
        .map(|root| {
            unwrap_local_tenant_root_dek_with_master_key(
                tenant_id,
                root.generation,
                &root.wrapped_tenant_root_dek,
                master_key,
            )
            .map(|key| (root.generation, Zeroizing::new(key)))
            .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LocalSyncKeys {
        tenant_id,
        tenant_root_dek: Some(tenant_root_dek),
        tenant_generation: active.generation,
        historical_tenant_root_deks,
    })
}

#[cfg(test)]
mod tests {
    use taskveil_domain::{new_list, new_task};
    use taskveil_storage::{
        ListRepository, SqliteListRepository, SqliteProfileCoordinationRepository,
        SqliteSyncStateRepository, SqliteTaskRepository, SyncStateRepository, TaskRepository,
    };
    use taskveil_sync::account::AccountKeyMaterial;
    use tempfile::TempDir;

    use super::*;

    const DB_KEY: [u8; 32] = [0x84; 32];
    const MASTER_KEY: [u8; KEY_LEN] = [0x52; KEY_LEN];
    const NOW: i64 = 1_799_000_000_000;

    fn account_keys() -> AccountKeyMaterial {
        let root = taskveil_crypto::organization::generate_account_root(Uuid::now_v7()).unwrap();
        AccountKeyMaterial {
            generation: 1,
            tenant_generation: 1,
            master_key: Zeroizing::new(MASTER_KEY),
            account_root_private: root.private,
            account_root_public: root.public,
            tenant_root_dek: Zeroizing::new([0x22; KEY_LEN]),
        }
    }

    fn sync_keys(
        tenant_id: Uuid,
        generation: u64,
        key: [u8; KEY_LEN],
        historical: Vec<(u64, Zeroizing<[u8; KEY_LEN]>)>,
    ) -> LocalSyncKeys {
        LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new(key)),
            tenant_generation: generation,
            historical_tenant_root_deks: historical,
        }
    }

    #[test]
    fn semantic_key_identity_avoids_rewrap_cutover_and_persists_history() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("semantic-keys.sqlite3");
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let first_key = [0x21; KEY_LEN];

        persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(identity.tenant_id, 1, first_key, Vec::new()),
            NOW,
        )
        .unwrap();
        let runtime = || {
            SqliteProfileCoordinationRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .load_runtime()
                .unwrap()
                .runtime_epoch
        };
        assert_eq!(runtime(), 2);
        let first_wrapped =
            SqliteLocalCryptoRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .load_tenant_roots(identity.tenant_id)
                .unwrap();

        // AEAD wrapping is randomized, but an identical semantic key set must
        // retain the authenticated ciphertext and leave the epoch unchanged.
        persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(identity.tenant_id, 1, first_key, Vec::new()),
            NOW + 1,
        )
        .unwrap();
        assert_eq!(runtime(), 2);
        assert_eq!(
            SqliteLocalCryptoRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .load_tenant_roots(identity.tenant_id)
                .unwrap(),
            first_wrapped
        );

        let rejected = persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(identity.tenant_id, 1, [0x22; KEY_LEN], Vec::new()),
            NOW + 2,
        );
        assert!(matches!(rejected, Err(StorageError::IncompatibleSchema(_))));
        assert_eq!(runtime(), 2);

        persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(
                identity.tenant_id,
                2,
                [0x23; KEY_LEN],
                vec![(1, Zeroizing::new(first_key))],
            ),
            NOW + 3,
        )
        .unwrap();
        assert_eq!(runtime(), 3);
        let LocalCryptoAvailability::Ready(reloaded) =
            load_local_crypto_context(&db_path, &DB_KEY, Some(MASTER_KEY)).unwrap()
        else {
            panic!("expected persisted key history");
        };
        assert_eq!(reloaded.sync_keys().tenant_generation, 2);
        assert_eq!(
            reloaded.sync_keys().historical_tenant_root_deks,
            vec![(1, Zeroizing::new(first_key))]
        );

        persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(identity.tenant_id, 2, [0x23; KEY_LEN], Vec::new()),
            NOW + 4,
        )
        .unwrap();
        assert_eq!(runtime(), 4);
        let LocalCryptoAvailability::Ready(reloaded) =
            load_local_crypto_context(&db_path, &DB_KEY, Some(MASTER_KEY)).unwrap()
        else {
            panic!("expected active key after retired generation pruning");
        };
        assert!(reloaded.sync_keys().historical_tenant_root_deks.is_empty());
        assert_eq!(
            SqliteLocalCryptoRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .load_tenant_roots(identity.tenant_id)
                .unwrap()
                .len(),
            1
        );

        persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            sync_keys(identity.tenant_id, 2, [0x23; KEY_LEN], Vec::new()),
            NOW + 5,
        )
        .unwrap();
        assert_eq!(runtime(), 4);
    }

    #[test]
    fn persisted_context_reopens_without_remote_session() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("client.sqlite3");
        let list = new_list(
            "Inbox".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            NOW,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
        let task = new_task(
            list.id,
            None,
            "before".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            NOW,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteTaskRepository::new(connection)
            .insert(task.clone())
            .unwrap();
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        persist_account_crypto_context(
            &db_path,
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id,
            },
            &account_keys(),
            NOW,
        )
        .unwrap();

        let loaded = load_local_crypto_context(&db_path, &DB_KEY, Some(MASTER_KEY)).unwrap();
        let LocalCryptoAvailability::Ready(context) = loaded else {
            panic!("expected ready local crypto context");
        };
        assert_eq!(context.tenant_id(), tenant_id);
        assert_eq!(context.user_id(), user_id);
        assert_eq!(context.device_id(), device_id);
        assert!(context.sync_keys().tenant_root_dek.is_some());

        crate::SqliteMutationService::new(&db_path, DB_KEY)
            .update_task(
                crate::UpdateTaskInput {
                    task_id: task.id,
                    title: "after restart".to_string(),
                    note: String::new(),
                    priority: 0,
                    due: None,
                    scheduled_at: None,
                    estimated_minutes: None,
                    now_ms: NOW + 1,
                },
                &context.mutation_context(),
            )
            .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        assert_eq!(
            SqliteTaskRepository::new(connection)
                .get(task.id)
                .unwrap()
                .content
                .title,
            "after restart"
        );
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        assert_eq!(
            SqliteSyncStateRepository::new(connection)
                .list_outbox_heads(10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn bound_profile_without_master_key_is_not_anonymous() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("client.sqlite3");
        let list = new_list(
            "Inbox".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            NOW,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
        persist_account_crypto_context(
            &db_path,
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id: Uuid::now_v7(),
                user_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
            },
            &account_keys(),
            NOW,
        )
        .unwrap();

        assert!(matches!(
            load_local_crypto_context(&db_path, &DB_KEY, None).unwrap(),
            LocalCryptoAvailability::AccountBoundUnavailable(
                LocalCryptoUnavailable::MissingMasterKey
            )
        ));
    }

    #[test]
    fn corrupt_cached_bundle_is_typed_unavailable() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("client.sqlite3");
        let list = new_list(
            "Inbox".to_string(),
            "7fffffffffffffffffffffffffffffff".to_string(),
            NOW,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        SqliteListRepository::new(connection)
            .insert(list.clone())
            .unwrap();
        persist_account_crypto_context(
            &db_path,
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id: Uuid::now_v7(),
                user_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
            },
            &account_keys(),
            NOW,
        )
        .unwrap();
        let connection = open_encrypted(&db_path, &DB_KEY).unwrap();
        connection
            .execute(
                "UPDATE local_tenant_root_key_cache SET wrapped_tenant_root_dek = x'00'",
                [],
            )
            .unwrap();

        assert!(matches!(
            load_local_crypto_context(&db_path, &DB_KEY, Some(MASTER_KEY)).unwrap(),
            LocalCryptoAvailability::AccountBoundUnavailable(
                LocalCryptoUnavailable::CorruptKeyCache
            )
        ));
    }
}
