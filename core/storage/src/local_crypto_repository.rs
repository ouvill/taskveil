use crate::profile_coordination::bump_runtime_epoch_on;
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

    pub fn load_tenant_roots(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<LocalTenantRootKeyBundle>, StorageError> {
        if let Some(binding) = load_local_profile_binding_on(&self.connection)? {
            if binding.tenant_id != tenant_id {
                return Err(StorageError::LocalProfileTenantMismatch {
                    bound_tenant_id: binding.tenant_id,
                    requested_tenant_id: tenant_id,
                });
            }
        }
        load_local_tenant_roots_on(&self.connection, tenant_id)
    }
}

impl LocalCryptoRepository for SqliteLocalCryptoRepository {
    fn load_binding(&self) -> Result<Option<LocalProfileBinding>, StorageError> {
        load_local_profile_binding_on(&self.connection)
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
        Ok(self.load_tenant_roots(tenant_id)?.pop())
    }
}

pub(crate) fn bind_tenant_roots_on(
    connection: &Connection,
    binding: LocalProfileBinding,
    tenant_roots: &[LocalTenantRootKeyBundle],
) -> Result<bool, StorageError> {
    if tenant_roots.is_empty() {
        return Err(StorageError::IncompatibleSchema(
            "local Tenant Root DEK cache must contain an active generation".to_string(),
        ));
    }
    let mut previous_generation = None;
    for tenant_root in tenant_roots {
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
        if previous_generation.is_some_and(|previous| previous >= tenant_root.generation) {
            return Err(StorageError::IncompatibleSchema(
                "local Tenant Root DEK generations must be unique and ascending".to_string(),
            ));
        }
        previous_generation = Some(tenant_root.generation);
    }
    let existing_binding = load_local_profile_binding_on(connection)?;
    if let Some(existing) = &existing_binding {
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
    let existing_tenant_roots = load_local_tenant_roots_on(connection, binding.tenant_id)?;
    let runtime_changed = existing_binding
        .as_ref()
        .is_none_or(|existing| existing.device_id != binding.device_id)
        || existing_tenant_roots != tenant_roots;

    if !runtime_changed {
        return Ok(false);
    }

    connection.execute(
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
    connection.execute("DELETE FROM local_tenant_root_key_cache", [])?;
    for tenant_root in tenant_roots {
        connection.execute(
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
    }
    bump_runtime_epoch_on(connection, binding.updated_at)?;
    Ok(true)
}

pub(crate) fn load_local_tenant_roots_on(
    connection: &Connection,
    tenant_id: Uuid,
) -> Result<Vec<LocalTenantRootKeyBundle>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT tenant_id, generation, wrapped_tenant_root_dek, updated_at
             FROM local_tenant_root_key_cache
             WHERE tenant_id = ?1
             ORDER BY generation",
    )?;
    let roots = statement
        .query_map([tenant_id.to_string()], |row| {
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
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    Ok(roots)
}

pub(crate) fn load_local_profile_binding_on(
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
