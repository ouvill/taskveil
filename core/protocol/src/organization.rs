//! Organization wire DTOs shared by clients and the API server.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationInviteRequest {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationMemberResponse {
    pub member_user_id: Uuid,
    pub verification_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSafetyResponse {
    pub owner_user_id: Uuid,
    pub member_user_id: Uuid,
    pub owner_root_public: String,
    pub member_root_public: String,
    pub digest: String,
    pub decimal: String,
    pub qr_payload: String,
    pub verification_state: String,
    pub owner_confirmed: bool,
    pub member_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSafetyConfirmRequest {
    pub member_user_id: Uuid,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationDeviceDto {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub account_root_public: String,
    pub certificate: String,
    pub certificate_fingerprint: String,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationDeviceRosterDto {
    pub user_id: Uuid,
    pub account_root_public: String,
    pub revision: u64,
    pub devices: Vec<OrganizationDeviceDto>,
    pub signed_revocations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationDeviceRevocationRequest {
    pub signed_revocation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipientPackageRequest {
    pub device_id: Uuid,
    pub package: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipientPackageResponse {
    pub package: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn device_roster_preserves_wire_shape_and_rejects_unknown_fields() {
        let value = json!({
            "user_id": "018f0000-0000-7000-8000-000000000001",
            "account_root_public": "root",
            "revision": 7,
            "devices": [{
                "user_id": "018f0000-0000-7000-8000-000000000001",
                "device_id": "018f0000-0000-7000-8000-000000000002",
                "account_root_public": "root",
                "certificate": "certificate",
                "certificate_fingerprint": "fingerprint",
                "revoked": false
            }],
            "signed_revocations": []
        });
        let decoded: OrganizationDeviceRosterDto = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);

        let mut unknown = value;
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<OrganizationDeviceRosterDto>(unknown).is_err());
    }
}
