use std::fmt;

use taskveil_crypto::key_hierarchy::{INITIAL_KEY_GENERATION, KEY_LEN};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::account::AccountKeyMaterial;

pub const SYNC_CURSOR_NAME: &str = "main";
pub const SYNC_LOCAL_HLC_SETTING_KEY: &str = "sync_local_hlc";
pub const SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY: &str = "sync_full_resync_page_token";
pub const SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY: &str = "sync_full_resync_completion_token";
pub const SYNC_UPGRADE_REQUIRED_SETTING_KEY: &str = "sync_upgrade_required_v2";
pub const KEY_ROTATION_PENDING_SETTING_KEY: &str = "key_rotation_pending_generation";
pub const TASKS_COLLECTION: &str = "tasks";
pub const LISTS_COLLECTION: &str = "lists";
pub const TEMPLATES_COLLECTION: &str = "templates";
pub const TASK_SERIES_COLLECTION: &str = "task_series";
pub const TIMER_SESSIONS_COLLECTION: &str = "timer_sessions";

#[derive(Clone, PartialEq, Eq)]
pub struct LocalSyncKeys {
    pub tenant_id: Uuid,
    pub tenant_root_dek: Option<Zeroizing<[u8; KEY_LEN]>>,
    pub tenant_generation: u64,
    pub historical_tenant_root_deks: Vec<(u64, Zeroizing<[u8; KEY_LEN]>)>,
}

impl Default for LocalSyncKeys {
    fn default() -> Self {
        Self {
            tenant_id: Uuid::nil(),
            tenant_root_dek: None,
            tenant_generation: INITIAL_KEY_GENERATION,
            historical_tenant_root_deks: Vec::new(),
        }
    }
}

impl fmt::Debug for LocalSyncKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalSyncKeys")
            .field("has_tenant_root_dek", &self.tenant_root_dek.is_some())
            .field("tenant_generation", &self.tenant_generation)
            .field(
                "historical_generation_count",
                &self.historical_tenant_root_deks.len(),
            )
            .finish()
    }
}

impl LocalSyncKeys {
    pub fn from_account_keys(tenant_id: Uuid, keys: &AccountKeyMaterial) -> Self {
        Self {
            tenant_id,
            tenant_root_dek: Some(keys.tenant_root_dek.clone()),
            tenant_generation: keys.tenant_generation,
            historical_tenant_root_deks: Vec::new(),
        }
    }

    pub fn validate_for_write(&self) -> Result<(), &'static str> {
        if self.tenant_id.is_nil() || self.tenant_generation == 0 || self.tenant_root_dek.is_none()
        {
            return Err("invalid active key generation");
        }
        Ok(())
    }
}

pub fn tenant_root_dek(keys: &LocalSyncKeys) -> Option<&[u8; KEY_LEN]> {
    keys.tenant_root_dek.as_deref()
}

pub fn tenant_root_dek_for_generation(
    keys: &LocalSyncKeys,
    generation: u64,
) -> Option<&[u8; KEY_LEN]> {
    if keys.tenant_generation == generation {
        return tenant_root_dek(keys);
    }
    keys.historical_tenant_root_deks
        .iter()
        .find(|(candidate_generation, _)| *candidate_generation == generation)
        .map(|(_, dek)| &**dek)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_sync_keys_debug_redacts_key_material() {
        let keys = LocalSyncKeys {
            tenant_id: Uuid::now_v7(),
            tenant_root_dek: Some(Zeroizing::new([0xa5; KEY_LEN])),
            tenant_generation: INITIAL_KEY_GENERATION,
            historical_tenant_root_deks: Vec::new(),
        };

        let debug = format!("{keys:?}");

        assert_eq!(
            debug,
            "LocalSyncKeys { has_tenant_root_dek: true, tenant_generation: 1, historical_generation_count: 0 }"
        );
    }
}
