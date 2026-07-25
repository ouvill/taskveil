use crate::*;

/// SQLCipher-backed local profile binding and wrapped Tenant Root Key cache.
pub struct SqliteLocalCryptoRepository {
    connection: Connection,
}

impl SqliteLocalCryptoRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl LocalCryptoRepository for SqliteLocalCryptoRepository {
    fn load_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError> {
        load_local_profile_binding_on(&self.connection)
    }

    fn bind_tenant_root(
        &mut self,
        binding: LocalProfileBinding,
        tenant_root: &LocalTenantRootKeyBundle,
    ) -> Result<(), StorageError> {
        if tenant_root.tenant_id != binding.tenant_id {
            return Err(StorageError::LocalProfileTenantMismatch {
                bound_tenant_id: binding.tenant_id,
                requested_tenant_id: tenant_root.tenant_id,
            });
        }
        if tenant_root.generation == 0 || tenant_root.wrapped_tenant_root_dek.is_empty() {
            return Err(StorageError::IncompatibleSchema(
                "invalid local Tenant Root DEK cache entry".to_string(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_local_profile_binding_on(&transaction)? {
            if existing.tenant_id != binding.tenant_id {
                return Err(StorageError::LocalProfileTenantMismatch {
                    bound_tenant_id: existing.tenant_id,
                    requested_tenant_id: binding.tenant_id,
                });
            }
            if existing.user_id != binding.user_id {
                return Err(StorageError::LocalProfileUserMismatch {
                    bound_user_id: existing.user_id,
                    requested_user_id: binding.user_id,
                });
            }
        }

        transaction.execute(
            "INSERT INTO local_profile_binding (
                 singleton, tenant_id, user_id, device_id, bound_at, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(singleton) DO UPDATE SET
                 device_id = excluded.device_id,
                 updated_at = excluded.updated_at",
            params![
                binding.tenant_id.to_string(),
                binding.user_id.to_string(),
                binding.device_id.to_string(),
                binding.bound_at,
                binding.updated_at,
            ],
        )?;
        transaction.execute("DELETE FROM local_tenant_root_key_cache", [])?;
        transaction.execute(
            "INSERT INTO local_tenant_root_key_cache (
                tenant_id, generation, wrapped_tenant_root_dek, updated_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                tenant_root.tenant_id.to_string(),
                i64::try_from(tenant_root.generation).map_err(|_| {
                    StorageError::IncompatibleSchema("invalid tenant key generation".to_string())
                })?,
                tenant_root.wrapped_tenant_root_dek,
                tenant_root.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn load_tenant_root(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<LocalTenantRootKeyBundle>, StorageError> {
        if let Some(binding) = load_local_profile_binding_on(&self.connection)? {
            if binding.tenant_id != tenant_id {
                return Err(StorageError::LocalProfileTenantMismatch {
                    bound_tenant_id: binding.tenant_id,
                    requested_tenant_id: tenant_id,
                });
            }
        }
        self.connection
            .query_row(
                "SELECT tenant_id, generation, wrapped_tenant_root_dek, updated_at
                 FROM local_tenant_root_key_cache WHERE tenant_id = ?1",
                [tenant_id.to_string()],
                |row| {
                    let tenant_id = row.get::<_, String>(0)?.parse::<Uuid>().map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(LocalTenantRootKeyBundle {
                        tenant_id,
                        generation: u64::try_from(row.get::<_, i64>(1)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        wrapped_tenant_root_dek: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }
}

fn load_local_profile_binding_on(
    connection: &Connection,
) -> Result<Option<LocalProfileBinding>, StorageError> {
    let row = connection
        .query_row(
            "SELECT tenant_id, user_id, device_id, bound_at, updated_at
             FROM local_profile_binding
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(tenant_id, user_id, device_id, bound_at, updated_at)| {
        Ok(LocalProfileBinding {
            tenant_id: Uuid::parse_str(&tenant_id)?,
            user_id: Uuid::parse_str(&user_id)?,
            device_id: Uuid::parse_str(&device_id)?,
            bound_at,
            updated_at,
        })
    })
    .transpose()
}
