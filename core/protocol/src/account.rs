//! Account and entitlement wire DTOs shared by clients and the API server.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BillingResponseDto {
    pub provider: String,
    pub provider_app_user_id: Uuid,
    pub entitlement: BillingEntitlementDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BillingEntitlementDto {
    pub lookup_key: String,
    pub status: String,
    pub sync_allowed: bool,
    pub store_product_identifier: Option<String>,
    pub expires_at: Option<i64>,
    pub grace_expires_at: Option<i64>,
    pub will_renew: Option<bool>,
    pub environment: String,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountKeyBundleDto {
    pub suite_id: u16,
    pub generation: u64,
    pub tenant_generation: u64,
    pub wrapper_revision: u64,
    pub wrapped_master_key_by_password: String,
    pub wrapped_master_key_by_recovery: String,
    pub account_root_public: String,
    pub wrapped_account_root_private: String,
    pub wrapped_tenant_root_dek: String,
    pub tenant_key_manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceEnrollmentDto {
    pub suite_id: u16,
    pub account_root_public: String,
    pub device_certificate: String,
    pub certificate_fingerprint: String,
    pub proof_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveKeyBundleDto {
    pub suite_id: u16,
    pub generation: u64,
    pub wrapped_tenant_root_dek: String,
    pub signed_manifest: String,
    #[serde(default)]
    pub migrating_generations: Vec<HistoricalKeyBundleDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoricalKeyBundleDto {
    pub generation: u64,
    pub wrapped_tenant_root_dek: String,
    pub signed_manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateKeyWrappersRequest {
    pub suite_id: u16,
    pub generation: u64,
    pub expected_wrapper_revision: u64,
    pub wrapper_revision: u64,
    pub wrapped_master_key_by_password: String,
    pub wrapped_master_key_by_recovery: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn account_key_bundle_preserves_wire_shape() {
        let value = json!({
            "suite_id": 2,
            "generation": 3,
            "tenant_generation": 4,
            "wrapper_revision": 5,
            "wrapped_master_key_by_password": "password",
            "wrapped_master_key_by_recovery": "recovery",
            "account_root_public": "public",
            "wrapped_account_root_private": "private",
            "wrapped_tenant_root_dek": "dek",
            "tenant_key_manifest": "manifest"
        });
        let decoded: AccountKeyBundleDto = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
}
