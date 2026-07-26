use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ops::Deref;
use taskveil_crypto::{
    delete_account_secret,
    key_hierarchy::{
        unwrap_account_root_private_key_with_master_key, unwrap_master_key_with_device_key,
        wrap_account_root_private_key_with_master_key, INITIAL_KEY_GENERATION, KEY_LEN,
    },
    load_account_secret,
    organization::{
        AccountRootPrivateKeys, AccountRootPublicKeys, DeviceCertificate, DeviceIdentity,
        SignedDeviceRevocation,
    },
    store_account_secret, AccountSecretKind, LocalKeyCapsuleSlot, LocalKeyCapsuleStore,
    PlatformLocalKeyCapsuleStore,
};
use taskveil_domain::Uuid;
use taskveil_storage::{
    open_encrypted, ListRepository, LocalCryptoRepository, SqliteLocalCryptoRepository,
    SqliteProfileCoordinationRepository,
};
#[cfg(test)]
use taskveil_sync::SYNC_LOCAL_HLC_METADATA_KEY;
use taskveil_sync::{
    account::{
        unwrap_active_key_bundle, unwrap_historical_key_bundles, AccountClient, AccountClientError,
        AccountKeyMaterial, AccountLoginProvisional, AccountSession, AccountTokenSet,
        BillingResponseDto, DeviceEnrollmentDto, OrganizationRosterTrust,
    },
    canonical_server_origin,
    organization::verify_organization_active_bundle,
    rebind_local_device, LocalMutationSyncStore, LocalSyncAtomicStore, LocalSyncKeys,
    LocalSyncWriteTransaction,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::{
    now_ms, AccountReadiness, CryptoRuntimeState, TaskveilClient, ACCOUNT_DEVICE_ID_METADATA_KEY,
    ACCOUNT_EMAIL_METADATA_KEY, ACCOUNT_MK_GENERATION_METADATA_KEY,
    ACCOUNT_ROOT_PUBLIC_METADATA_KEY, ACCOUNT_SESSION_EXPIRES_AT_METADATA_KEY,
    ACCOUNT_TENANT_ID_METADATA_KEY, ACCOUNT_USER_ID_METADATA_KEY,
};
#[cfg(test)]
use crate::persist_local_crypto_context;
use crate::{
    load_local_crypto_context, AccountAuthResult, AccountSessionState, BillingState, ClientError,
    LocalCryptoAvailability, LocalCryptoIdentity, LocalCryptoUnavailable, OrganizationSafetyState,
};

#[derive(Clone, Copy)]
enum AccountAuthMode {
    Register,
    Login,
}

const BILLING_ENTITLEMENT_CACHE_METADATA_KEY: &str = "billing_entitlement_cache";
const SESSION_TOKEN_SET_VERSION: u8 = 2;
const ACCESS_TOKEN_REFRESH_SKEW_MS: i64 = 60_000;
const ABSENT_CREDENTIAL_GENERATION: &str = "absent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingLoginNetworkStep {
    Certify,
    RefreshCertifiedDevice,
}

fn pending_login_network_step(now_ms: i64, access_expires_at_ms: i64) -> PendingLoginNetworkStep {
    if access_expires_at_ms <= now_ms {
        PendingLoginNetworkStep::RefreshCertifiedDevice
    } else {
        // Never apply the normal refresh skew before initial certification:
        // the server intentionally rejects refresh for provisional devices.
        PendingLoginNetworkStep::Certify
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct StoredSessionTokens {
    version: u8,
    #[serde(default)]
    credential_generation: Option<String>,
    pub(super) issuer: String,
    access_token: String,
    access_expires_at_ms: i64,
    refresh_token: String,
    refresh_expires_at_ms: i64,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct StoredPendingLogin {
    version: u8,
    #[serde(default)]
    credential_generation: Option<String>,
    issuer: String,
    email: String,
    user_id: String,
    tenant_id: String,
    device_id: String,
    access_token: String,
    access_expires_at_ms: i64,
    refresh_token: String,
    refresh_expires_at_ms: i64,
    challenge_expires_at_ms: i64,
    local_wrapped_master_key: Vec<u8>,
    generation: u64,
    tenant_generation: u64,
    master_key: Vec<u8>,
    account_root_private: Vec<u8>,
    account_root_public: Vec<u8>,
    tenant_root_dek: Vec<u8>,
    device_identity: Vec<u8>,
    enrollment_suite_id: u16,
    enrollment_account_root_public: String,
    enrollment_device_certificate: String,
    enrollment_certificate_fingerprint: String,
    enrollment_proof_signature: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum StoredSessionCredential {
    Active(StoredSessionTokens),
    PendingDeviceCertification(Box<StoredPendingLogin>),
}

impl StoredSessionTokens {
    fn from_account_tokens(issuer: &str, tokens: &AccountTokenSet) -> Self {
        Self {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: issuer.to_string(),
            access_token: tokens.access_token.to_string(),
            access_expires_at_ms: tokens.access_expires_at_ms,
            refresh_token: tokens.refresh_token.to_string(),
            refresh_expires_at_ms: tokens.refresh_expires_at_ms,
        }
    }

    fn validate(&self) -> Result<(), ClientError> {
        if self.version != SESSION_TOKEN_SET_VERSION
            || self
                .credential_generation
                .as_deref()
                .is_some_and(|generation| generation.parse::<Uuid>().is_err())
            || canonical_server_origin(&self.issuer).as_deref() != Ok(self.issuer.as_str())
            || self.access_token.is_empty()
            || self.refresh_token.is_empty()
            || self.access_expires_at_ms <= 0
            || self.refresh_expires_at_ms <= 0
        {
            return Err(ClientError::IncompleteAccountState);
        }
        Ok(())
    }
}

impl StoredPendingLogin {
    fn from_provisional(
        issuer: &str,
        provisional: &AccountLoginProvisional,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: issuer.to_string(),
            email: provisional.session.email.clone(),
            user_id: provisional.session.user_id.clone(),
            tenant_id: provisional.session.tenant_id.clone(),
            device_id: provisional.session.device_id.clone(),
            access_token: provisional.session.tokens.access_token.to_string(),
            access_expires_at_ms: provisional.session.tokens.access_expires_at_ms,
            refresh_token: provisional.session.tokens.refresh_token.to_string(),
            refresh_expires_at_ms: provisional.session.tokens.refresh_expires_at_ms,
            challenge_expires_at_ms: provisional.challenge_expires_at_ms,
            local_wrapped_master_key: provisional.local_wrapped_master_key.clone(),
            generation: provisional.keys.generation,
            tenant_generation: provisional.keys.tenant_generation,
            master_key: provisional.keys.master_key.to_vec(),
            account_root_private: provisional.keys.account_root_private.encode().to_vec(),
            account_root_public: provisional
                .keys
                .account_root_public
                .encode()
                .map_err(|_| ClientError::AccountBoundUnavailable)?,
            tenant_root_dek: provisional.keys.tenant_root_dek.to_vec(),
            device_identity: provisional
                .device_identity
                .encode()
                .map_err(|_| ClientError::AccountRequest)?
                .to_vec(),
            enrollment_suite_id: provisional.enrollment.suite_id,
            enrollment_account_root_public: provisional.enrollment.account_root_public.clone(),
            enrollment_device_certificate: provisional.enrollment.device_certificate.clone(),
            enrollment_certificate_fingerprint: provisional
                .enrollment
                .certificate_fingerprint
                .clone(),
            enrollment_proof_signature: provisional.enrollment.proof_signature.clone(),
        })
    }

    fn validate(&self) -> Result<(), ClientError> {
        if self.version != SESSION_TOKEN_SET_VERSION
            || self
                .credential_generation
                .as_deref()
                .is_some_and(|generation| generation.parse::<Uuid>().is_err())
            || canonical_server_origin(&self.issuer).as_deref() != Ok(self.issuer.as_str())
            || self.email.is_empty()
            || self.access_token.is_empty()
            || self.refresh_token.is_empty()
            || self.access_expires_at_ms <= 0
            || self.refresh_expires_at_ms <= 0
            || self.challenge_expires_at_ms <= 0
            || self.generation == 0
            || self.tenant_generation == 0
            || self.master_key.len() != KEY_LEN
            || self.account_root_private.len() != 64
            || self.tenant_root_dek.len() != KEY_LEN
            || self.device_identity.is_empty()
        {
            return Err(ClientError::IncompleteAccountState);
        }
        parse_uuid(&self.user_id)?;
        parse_uuid(&self.tenant_id)?;
        parse_uuid(&self.device_id)?;
        Ok(())
    }

    fn to_provisional(&self) -> Result<AccountLoginProvisional, ClientError> {
        self.validate()?;
        let master_key: [u8; KEY_LEN] = self
            .master_key
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        let tenant_root_dek: [u8; KEY_LEN] = self
            .tenant_root_dek
            .as_slice()
            .try_into()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        Ok(AccountLoginProvisional {
            session: AccountSession {
                user_id: self.user_id.clone(),
                tenant_id: self.tenant_id.clone(),
                device_id: self.device_id.clone(),
                email: self.email.clone(),
                tokens: AccountTokenSet {
                    access_token: Zeroizing::new(self.access_token.clone()),
                    access_expires_at_ms: self.access_expires_at_ms,
                    refresh_token: Zeroizing::new(self.refresh_token.clone()),
                    refresh_expires_at_ms: self.refresh_expires_at_ms,
                },
            },
            local_wrapped_master_key: self.local_wrapped_master_key.clone(),
            keys: AccountKeyMaterial {
                generation: self.generation,
                tenant_generation: self.tenant_generation,
                master_key: Zeroizing::new(master_key),
                account_root_private: AccountRootPrivateKeys::decode(&self.account_root_private)
                    .map_err(|_| ClientError::IncompleteAccountState)?,
                account_root_public: AccountRootPublicKeys::decode(&self.account_root_public)
                    .map_err(|_| ClientError::IncompleteAccountState)?,
                tenant_root_dek: Zeroizing::new(tenant_root_dek),
            },
            device_identity: DeviceIdentity::decode(&self.device_identity)
                .map_err(|_| ClientError::IncompleteAccountState)?,
            enrollment: DeviceEnrollmentDto {
                suite_id: self.enrollment_suite_id,
                account_root_public: self.enrollment_account_root_public.clone(),
                device_certificate: self.enrollment_device_certificate.clone(),
                certificate_fingerprint: self.enrollment_certificate_fingerprint.clone(),
                proof_signature: self.enrollment_proof_signature.clone(),
            },
            challenge_expires_at_ms: self.challenge_expires_at_ms,
        })
    }
}

pub(super) struct OriginBoundAccessToken {
    pub(super) issuer: String,
    token: Zeroizing<String>,
}

impl Deref for OriginBoundAccessToken {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.token.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OrganizationTrustPin {
    owner_root_public: String,
    member_root_public: String,
    digest: String,
    locally_confirmed: bool,
    minimum_generation: u64,
    required_generation: u64,
    owner_roster_revision: u64,
    owner_roster_head_hash: String,
    member_roster_revision: u64,
    member_roster_head_hash: String,
}

impl OrganizationTrustPin {
    fn candidate(response: &taskveil_sync::organization::OrganizationSafetyResponse) -> Self {
        Self {
            owner_root_public: response.owner_root_public.clone(),
            member_root_public: response.member_root_public.clone(),
            digest: response.digest.clone(),
            locally_confirmed: false,
            minimum_generation: 1,
            required_generation: 0,
            owner_roster_revision: 0,
            owner_roster_head_hash: STANDARD.encode([0u8; 32]),
            member_roster_revision: 0,
            member_roster_head_hash: STANDARD.encode([0u8; 32]),
        }
    }

    fn matches(&self, response: &taskveil_sync::organization::OrganizationSafetyResponse) -> bool {
        self.owner_root_public == response.owner_root_public
            && self.member_root_public == response.member_root_public
            && self.digest == response.digest
    }

    fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.owner_root_public,
            self.member_root_public,
            self.digest,
            u8::from(self.locally_confirmed),
            self.minimum_generation,
            self.required_generation,
            self.owner_roster_revision,
            self.owner_roster_head_hash,
            self.member_roster_revision,
            self.member_roster_head_hash
        )
    }

    fn decode(value: &str) -> Option<Self> {
        let mut fields = value.split('|');
        let result = Self {
            owner_root_public: fields.next()?.to_string(),
            member_root_public: fields.next()?.to_string(),
            digest: fields.next()?.to_string(),
            locally_confirmed: match fields.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            },
            minimum_generation: fields.next()?.parse().ok()?,
            required_generation: fields.next()?.parse().ok()?,
            owner_roster_revision: fields.next()?.parse().ok()?,
            owner_roster_head_hash: fields.next()?.to_string(),
            member_roster_revision: fields.next()?.parse().ok()?,
            member_roster_head_hash: fields.next()?.to_string(),
        };
        if fields.next().is_some()
            || result.owner_root_public.is_empty()
            || result.member_root_public.is_empty()
            || result.digest.is_empty()
            || result.minimum_generation == 0
            || (result.required_generation != 0
                && result.required_generation <= result.minimum_generation)
            || STANDARD.decode(&result.owner_roster_head_hash).ok()?.len() != 32
            || STANDARD.decode(&result.member_roster_head_hash).ok()?.len() != 32
        {
            return None;
        }
        Some(result)
    }
}

fn organization_safety_state(
    mut response: taskveil_sync::organization::OrganizationSafetyResponse,
    locally_verified: bool,
) -> OrganizationSafetyState {
    if !locally_verified {
        response.verification_state = "unverified".to_string();
    }
    OrganizationSafetyState {
        owner_user_id: response.owner_user_id.to_string(),
        member_user_id: response.member_user_id.to_string(),
        digest: response.digest,
        decimal: response.decimal,
        qr_payload: response.qr_payload,
        verification_state: response.verification_state,
        owner_confirmed: response.owner_confirmed,
        member_confirmed: response.member_confirmed,
    }
}

fn decode_trust_hash(value: &str) -> Result<[u8; 32], ClientError> {
    STANDARD
        .decode(value)
        .map_err(|_| ClientError::AccountRequest)?
        .try_into()
        .map_err(|_| ClientError::AccountRequest)
}

fn decode_trust_root(value: &str) -> Result<AccountRootPublicKeys, ClientError> {
    AccountRootPublicKeys::decode(
        &STANDARD
            .decode(value)
            .map_err(|_| ClientError::AccountRequest)?,
    )
    .map_err(|_| ClientError::AccountRequest)
}

impl TaskveilClient {
    pub async fn organization_safety_number(
        &self,
        tenant_id: String,
        member_user_id: String,
    ) -> Result<OrganizationSafetyState, ClientError> {
        let _operation = self.begin_operation()?;
        self.ensure_account_runtime_restored()?;
        let tenant_id = parse_uuid(&tenant_id)?;
        let member_user_id = parse_uuid(&member_user_id)?;
        let session_token = self.access_token(false).await?;
        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let response = client
            .organization_safety_number(tenant_id, member_user_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&response)?;
        let locally_verified = self
            .load_organization_trust_pin(tenant_id, member_user_id)?
            .is_some_and(|pin| {
                pin.locally_confirmed
                    && pin.matches(&response)
                    && response.verification_state == "verified"
            });
        Ok(organization_safety_state(response, locally_verified))
    }

    pub async fn confirm_organization_safety_number(
        &self,
        tenant_id: String,
        member_user_id: String,
        digest: String,
    ) -> Result<OrganizationSafetyState, ClientError> {
        let _operation = self.begin_operation()?;
        self.ensure_account_runtime_restored()?;
        let tenant_id = parse_uuid(&tenant_id)?;
        let member_user_id = parse_uuid(&member_user_id)?;
        let session_token = self.access_token(false).await?;
        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let current = client
            .organization_safety_number(tenant_id, member_user_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&current)?;
        if current.digest != digest {
            return Err(ClientError::AccountRequest);
        }
        let response = client
            .confirm_organization_safety_number(tenant_id, member_user_id, digest, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&response)?;
        if !OrganizationTrustPin::candidate(&current).matches(&response) {
            return Err(ClientError::AccountRequest);
        }
        let mut pin = OrganizationTrustPin::candidate(&response);
        pin.locally_confirmed = true;
        self.store_organization_trust_pin(tenant_id, member_user_id, &pin)?;
        let locally_verified = response.verification_state == "verified";
        Ok(organization_safety_state(response, locally_verified))
    }

    pub async fn verify_organization_device_rosters(
        &self,
        tenant_id: String,
        member_user_id: String,
    ) -> Result<(), ClientError> {
        let _operation = self.begin_operation()?;
        self.ensure_account_runtime_restored()?;
        let tenant_id = parse_uuid(&tenant_id)?;
        let member_user_id = parse_uuid(&member_user_id)?;
        let session_token = self.access_token(false).await?;
        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let safety = client
            .organization_safety_number(tenant_id, member_user_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&safety)?;
        let mut pin = self
            .load_organization_trust_pin(tenant_id, member_user_id)?
            .filter(|pin| {
                pin.locally_confirmed
                    && pin.matches(&safety)
                    && safety.verification_state == "verified"
            })
            .ok_or(ClientError::AccountRequest)?;
        let owner_root = decode_trust_root(&pin.owner_root_public)?;
        let member_root = decode_trust_root(&pin.member_root_public)?;
        if member_root.user_id != member_user_id {
            return Err(ClientError::AccountRequest);
        }
        let owner = client
            .organization_owner_devices(
                tenant_id,
                member_user_id,
                OrganizationRosterTrust {
                    user_id: owner_root.user_id,
                    root_public: &pin.owner_root_public,
                    minimum_revision: pin.owner_roster_revision,
                    minimum_head_hash: decode_trust_hash(&pin.owner_roster_head_hash)?,
                },
                &session_token,
            )
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        let member = client
            .organization_member_devices(
                tenant_id,
                member_user_id,
                OrganizationRosterTrust {
                    user_id: member_root.user_id,
                    root_public: &pin.member_root_public,
                    minimum_revision: pin.member_roster_revision,
                    minimum_head_hash: decode_trust_hash(&pin.member_roster_head_hash)?,
                },
                &session_token,
            )
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        if owner.revision != pin.owner_roster_revision
            || owner.head_hash != decode_trust_hash(&pin.owner_roster_head_hash)?
            || member.revision != pin.member_roster_revision
            || member.head_hash != decode_trust_hash(&pin.member_roster_head_hash)?
        {
            pin.required_generation = pin
                .minimum_generation
                .checked_add(1)
                .ok_or(ClientError::AccountRequest)?;
        }
        pin.owner_roster_revision = owner.revision;
        pin.owner_roster_head_hash = STANDARD.encode(owner.head_hash);
        pin.member_roster_revision = member.revision;
        pin.member_roster_head_hash = STANDARD.encode(member.head_hash);
        self.store_organization_trust_pin(tenant_id, member_user_id, &pin)
    }

    pub async fn verify_organization_active_key_bundle(
        &self,
        tenant_id: String,
        member_user_id: String,
    ) -> Result<u64, ClientError> {
        let _operation = self.begin_operation()?;
        self.ensure_account_runtime_restored()?;
        let tenant_id = parse_uuid(&tenant_id)?;
        let member_user_id = parse_uuid(&member_user_id)?;
        let session_token = self.access_token(false).await?;
        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let safety = client
            .organization_safety_number(tenant_id, member_user_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&safety)?;
        let mut pin = self
            .load_organization_trust_pin(tenant_id, member_user_id)?
            .filter(|pin| {
                pin.locally_confirmed
                    && pin.matches(&safety)
                    && safety.verification_state == "verified"
            })
            .ok_or(ClientError::AccountRequest)?;
        let owner_root = decode_trust_root(&pin.owner_root_public)?;
        let member_root = decode_trust_root(&pin.member_root_public)?;
        if member_root.user_id != member_user_id {
            return Err(ClientError::AccountRequest);
        }
        let device_identity = DeviceIdentity::decode(
            &load_account_secret(&self.db_dir, AccountSecretKind::DeviceIdentity)
                .map_err(ClientError::KeyStore)?
                .ok_or(ClientError::AccountRequest)?,
        )
        .map_err(|_| ClientError::AccountRequest)?;
        let owner_roster = client
            .organization_owner_devices(
                tenant_id,
                member_user_id,
                OrganizationRosterTrust {
                    user_id: owner_root.user_id,
                    root_public: &pin.owner_root_public,
                    minimum_revision: pin.owner_roster_revision,
                    minimum_head_hash: decode_trust_hash(&pin.owner_roster_head_hash)?,
                },
                &session_token,
            )
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        let member_roster = client
            .organization_member_devices(
                tenant_id,
                member_user_id,
                OrganizationRosterTrust {
                    user_id: member_root.user_id,
                    root_public: &pin.member_root_public,
                    minimum_revision: pin.member_roster_revision,
                    minimum_head_hash: decode_trust_hash(&pin.member_roster_head_hash)?,
                },
                &session_token,
            )
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        if owner_roster.revision != pin.owner_roster_revision
            || owner_roster.head_hash != decode_trust_hash(&pin.owner_roster_head_hash)?
            || member_roster.revision != pin.member_roster_revision
            || member_roster.head_hash != decode_trust_hash(&pin.member_roster_head_hash)?
        {
            pin.required_generation = pin
                .minimum_generation
                .checked_add(1)
                .ok_or(ClientError::AccountRequest)?;
        }
        let expected_recipients = owner_roster
            .devices
            .iter()
            .chain(member_roster.devices.iter())
            .map(|device| {
                DeviceCertificate::decode(
                    &STANDARD
                        .decode(&device.certificate)
                        .map_err(|_| ClientError::AccountRequest)?,
                )
                .and_then(|certificate| certificate.recipient_key_fingerprint())
                .map_err(|_| ClientError::AccountRequest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bundle = client
            .active_key_bundle(tenant_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        verify_organization_active_bundle(
            &bundle,
            tenant_id,
            pin.required_generation.max(pin.minimum_generation),
            &owner_root,
            device_identity.certificate(),
            &expected_recipients,
        )
        .map_err(|_| ClientError::AccountRequest)?;
        pin.minimum_generation = bundle.generation;
        pin.required_generation = 0;
        pin.owner_roster_revision = owner_roster.revision;
        pin.owner_roster_head_hash = STANDARD.encode(owner_roster.head_hash);
        pin.member_roster_revision = member_roster.revision;
        pin.member_roster_head_hash = STANDARD.encode(member_roster.head_hash);
        self.store_organization_trust_pin(tenant_id, member_user_id, &pin)?;
        Ok(bundle.generation)
    }

    pub async fn revoke_organization_device(
        &self,
        tenant_id: String,
        member_user_id: String,
        device_id: String,
    ) -> Result<(), ClientError> {
        let _operation = self.begin_operation()?;
        self.ensure_account_runtime_restored()?;
        let tenant_id = parse_uuid(&tenant_id)?;
        let member_user_id = parse_uuid(&member_user_id)?;
        let device_id = parse_uuid(&device_id)?;
        let local_user_id = parse_uuid(
            &self
                .non_empty_internal_metadata(ACCOUNT_USER_ID_METADATA_KEY)?
                .ok_or(ClientError::IncompleteAccountState)?,
        )?;
        let session_token = self.access_token(false).await?;
        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let safety = client
            .organization_safety_number(tenant_id, member_user_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        self.verify_local_safety_participant(&safety)?;
        let mut pin = self
            .load_organization_trust_pin(tenant_id, member_user_id)?
            .filter(|pin| {
                pin.locally_confirmed
                    && pin.matches(&safety)
                    && safety.verification_state == "verified"
            })
            .ok_or(ClientError::AccountRequest)?;
        let owner_root = decode_trust_root(&pin.owner_root_public)?;
        let member_root = decode_trust_root(&pin.member_root_public)?;
        if member_root.user_id != member_user_id {
            return Err(ClientError::AccountRequest);
        }
        let (roster, is_owner) = if safety.owner_user_id == local_user_id {
            (
                client
                    .organization_owner_devices(
                        tenant_id,
                        member_user_id,
                        OrganizationRosterTrust {
                            user_id: owner_root.user_id,
                            root_public: &pin.owner_root_public,
                            minimum_revision: pin.owner_roster_revision,
                            minimum_head_hash: decode_trust_hash(&pin.owner_roster_head_hash)?,
                        },
                        &session_token,
                    )
                    .await
                    .map_err(|_| ClientError::AccountRequest)?,
                true,
            )
        } else if safety.member_user_id == local_user_id {
            (
                client
                    .organization_member_devices(
                        tenant_id,
                        member_user_id,
                        OrganizationRosterTrust {
                            user_id: member_root.user_id,
                            root_public: &pin.member_root_public,
                            minimum_revision: pin.member_roster_revision,
                            minimum_head_hash: decode_trust_hash(&pin.member_roster_head_hash)?,
                        },
                        &session_token,
                    )
                    .await
                    .map_err(|_| ClientError::AccountRequest)?,
                false,
            )
        } else {
            return Err(ClientError::AccountRequest);
        };
        let device = roster
            .devices
            .iter()
            .find(|device| device.device_id == device_id && device.user_id == local_user_id)
            .ok_or(ClientError::AccountRequest)?;
        let certificate_fingerprint: [u8; 48] = STANDARD
            .decode(&device.certificate_fingerprint)
            .map_err(|_| ClientError::AccountRequest)?
            .try_into()
            .map_err(|_| ClientError::AccountRequest)?;
        let (root_private, root_public) = self.load_account_root_keys()?;
        let next_revision = roster
            .revision
            .checked_add(1)
            .ok_or(ClientError::AccountRequest)?;
        let statement = SignedDeviceRevocation::sign(
            &root_private,
            &root_public,
            device_id,
            certificate_fingerprint,
            next_revision,
            now_ms()?,
            roster.head_hash,
        )
        .map_err(|_| ClientError::AccountRequest)?;
        client
            .revoke_organization_device(tenant_id, device_id, &statement, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        pin.required_generation = pin
            .minimum_generation
            .checked_add(1)
            .ok_or(ClientError::AccountRequest)?;
        if is_owner {
            pin.owner_roster_revision = statement.revision;
            pin.owner_roster_head_hash = STANDARD.encode(
                statement
                    .authenticated_hash()
                    .map_err(|_| ClientError::AccountRequest)?,
            );
        } else {
            pin.member_roster_revision = statement.revision;
            pin.member_roster_head_hash = STANDARD.encode(
                statement
                    .authenticated_hash()
                    .map_err(|_| ClientError::AccountRequest)?,
            );
        }
        self.store_organization_trust_pin(tenant_id, member_user_id, &pin)
    }

    pub fn account_session_state(&self) -> Result<AccountSessionState, ClientError> {
        let _operation = self.begin_operation()?;
        match self.resolve_account_readiness()? {
            AccountReadiness::LoggedOut => Ok(AccountSessionState::logged_out()),
            AccountReadiness::Ready => self
                .account_state()?
                .session
                .clone()
                .ok_or(ClientError::CredentialUnavailable),
            AccountReadiness::CredentialUnavailable => Err(ClientError::CredentialUnavailable),
            AccountReadiness::AccountBoundUnavailable => Err(ClientError::AccountBoundUnavailable),
        }
    }

    pub async fn account_register(
        &self,
        email: String,
        password: String,
        server_url: Option<String>,
        device_name: Option<String>,
    ) -> Result<AccountAuthResult, ClientError> {
        self.account_auth(
            email,
            password,
            server_url,
            device_name,
            AccountAuthMode::Register,
        )
        .await
    }

    pub async fn account_login(
        &self,
        email: String,
        password: String,
        server_url: Option<String>,
        device_name: Option<String>,
    ) -> Result<AccountAuthResult, ClientError> {
        self.account_auth(
            email,
            password,
            server_url,
            device_name,
            AccountAuthMode::Login,
        )
        .await
    }

    pub async fn account_logout(&self) -> Result<(), ClientError> {
        let _operation = self.begin_exclusive_operation()?;
        let _session_lock = acquire_session_token_set_lock(&self.db_dir)?;
        let credential = load_session_credential(&self.db_dir)?;
        if let Some((issuer, refresh_token)) =
            credential.as_ref().map(|credential| match credential {
                StoredSessionCredential::Active(tokens) => {
                    (tokens.issuer.as_str(), tokens.refresh_token.as_str())
                }
                StoredSessionCredential::PendingDeviceCertification(pending) => {
                    (pending.issuer.as_str(), pending.refresh_token.as_str())
                }
            })
        {
            let client = AccountClient::new(issuer).map_err(|_| ClientError::AccountRequest)?;
            client
                .logout(refresh_token)
                .await
                .map_err(map_account_client_error)?;
        }
        self.invalidate_remote_session_locked()?;
        // Logout revokes only the remote session. The account binding, wrapped
        // master key, and verified local Tenant Root DEK cache deliberately
        // survive so offline mutation remains available.
        Ok(())
    }

    pub async fn billing_bootstrap(&self) -> Result<BillingState, ClientError> {
        let _operation = self.begin_operation()?;
        self.fetch_billing(false).await
    }

    pub async fn refresh_billing(&self) -> Result<BillingState, ClientError> {
        let _operation = self.begin_operation()?;
        self.fetch_billing(true).await
    }

    pub fn cached_billing(&self) -> Result<Option<BillingState>, ClientError> {
        let _operation = self.begin_operation()?;
        self.internal_metadata(BILLING_ENTITLEMENT_CACHE_METADATA_KEY)?
            .map(|value| serde_json::from_str(&value).map_err(|_| ClientError::AccountRequest))
            .transpose()
    }

    async fn fetch_billing(&self, refresh: bool) -> Result<BillingState, ClientError> {
        self.ensure_account_runtime_restored()?;
        let session = self
            .account_state()?
            .session
            .clone()
            .filter(|session| session.logged_in)
            .ok_or(ClientError::AccountRequest)?;
        let tenant_id = parse_uuid(
            session
                .tenant_id
                .as_deref()
                .ok_or(ClientError::AccountRequest)?,
        )?;
        let token = self.access_token(false).await?;
        let client = AccountClient::new(&token.issuer).map_err(|_| ClientError::AccountRequest)?;
        let response = if refresh {
            client.refresh_billing(tenant_id, &token).await
        } else {
            client.billing(tenant_id, &token).await
        }
        .map_err(map_account_client_error)?;
        let state = billing_state(response);
        self.set_internal_metadata_value(
            BILLING_ENTITLEMENT_CACHE_METADATA_KEY,
            &serde_json::to_string(&state).map_err(|_| ClientError::AccountRequest)?,
        )?;
        Ok(state)
    }

    async fn account_auth(
        &self,
        email: String,
        password: String,
        server_url: Option<String>,
        device_name: Option<String>,
        mode: AccountAuthMode,
    ) -> Result<AccountAuthResult, ClientError> {
        let _operation = self.begin_exclusive_operation()?;
        let _session_lock = acquire_session_token_set_lock(&self.db_dir)?;
        let requested_server_url = match server_url {
            Some(server_url) => server_url,
            None => self.sync_server_url_unlocked()?,
        };
        let server_url = canonical_server_origin(&requested_server_url)
            .map_err(|_| ClientError::AccountRequest)?;
        if let Some(pending) = load_pending_login(&self.db_dir)? {
            if !matches!(mode, AccountAuthMode::Login)
                || pending.issuer != server_url
                || !pending.email.eq_ignore_ascii_case(email.trim())
            {
                return Err(ClientError::AccountRequest);
            }
            let client =
                AccountClient::new(&pending.issuer).map_err(|_| ClientError::AccountRequest)?;
            return self.resume_pending_login_locked(client, pending).await;
        }
        if load_session_tokens(&self.db_dir)?.is_some() {
            // Re-authentication must not silently abandon an existing remote
            // family. The caller must complete remote-first logout first.
            return Err(ClientError::AccountRequest);
        }
        let device_key = Zeroizing::new(*self.active_capsule()?.device_key());
        let client = AccountClient::new(&server_url).map_err(|_| ClientError::AccountRequest)?;
        let password = Zeroizing::new(password);

        match mode {
            AccountAuthMode::Register => {
                self.ensure_profile_is_unbound_for_registration()?;
                let outcome = client
                    .register(&email, &password, device_name.as_deref(), &device_key)
                    .await
                    .map_err(|_| ClientError::AccountRequest)?;
                let session = account_session_state(
                    outcome.session.email.clone(),
                    outcome.session.user_id.clone(),
                    outcome.session.tenant_id.clone(),
                    outcome.session.device_id.clone(),
                );
                let encoded_identity = outcome
                    .device_identity
                    .encode()
                    .map_err(|_| ClientError::AccountRequest)?;
                store_account_secret(
                    &self.db_dir,
                    AccountSecretKind::DeviceIdentity,
                    &encoded_identity,
                )
                .map_err(ClientError::KeyStore)?;
                let crypto = self.persist_account_state_locked(
                    &server_url,
                    &session,
                    &outcome.session.tokens,
                    &outcome.local_wrapped_master_key,
                    &outcome.keys,
                )?;
                self.set_internal_metadata_value(super::SYNC_SERVER_URL_METADATA_KEY, &server_url)?;
                self.replace_account_runtime(Some(session.clone()), crypto)?;
                // A new profile has no initial-backfill cursor. Do not delete a
                // durable cursor here: same-profile authentication must be
                // idempotent and must never replay a completed backfill.
                Ok(AccountAuthResult {
                    session,
                    recovery_key: Some(outcome.recovery_key.to_string()),
                })
            }
            AccountAuthMode::Login => {
                let provisional = client
                    .begin_login(&email, &password, device_name.as_deref(), &device_key)
                    .await
                    .map_err(|_| ClientError::AccountRequest)?;
                let pending = StoredPendingLogin::from_provisional(&server_url, &provisional)?;
                store_pending_login(&self.db_dir, pending)?;
                self.resume_pending_login_locked(
                    client,
                    load_pending_login(&self.db_dir)?.ok_or(ClientError::IncompleteAccountState)?,
                )
                .await
            }
        }
    }

    async fn resume_pending_login_locked(
        &self,
        client: AccountClient,
        pending: StoredPendingLogin,
    ) -> Result<AccountAuthResult, ClientError> {
        let issuer = pending.issuer.clone();
        let mut provisional = pending.to_provisional()?;
        let tenant_id = parse_uuid(&provisional.session.tenant_id)?;
        let user_id = parse_uuid(&provisional.session.user_id)?;
        if let Err(identity_error) = self.validate_existing_profile_identity(tenant_id, user_id) {
            client
                .logout(&provisional.session.tokens.refresh_token)
                .await
                .map_err(map_account_client_error)?;
            delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
                .map_err(ClientError::KeyStore)?;
            return Err(identity_error);
        }

        let now = now_ms()?;
        if provisional.session.tokens.refresh_expires_at_ms <= now {
            delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
                .map_err(ClientError::KeyStore)?;
            return Err(ClientError::AccountRequest);
        }
        if pending_login_network_step(now, provisional.session.tokens.access_expires_at_ms)
            == PendingLoginNetworkStep::RefreshCertifiedDevice
        {
            match client
                .refresh(&provisional.session.tokens.refresh_token)
                .await
            {
                Ok(tokens) => {
                    // The server refresh endpoint accepts only certified
                    // devices, so success proves that certification completed
                    // before the previous process stopped.
                    provisional.session.tokens = tokens;
                    store_pending_login(
                        &self.db_dir,
                        StoredPendingLogin::from_provisional(&issuer, &provisional)?,
                    )?;
                }
                Err(AccountClientError::InvalidGrant)
                    if provisional.challenge_expires_at_ms <= now =>
                {
                    client
                        .logout(&provisional.session.tokens.refresh_token)
                        .await
                        .map_err(map_account_client_error)?;
                    delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
                        .map_err(ClientError::KeyStore)?;
                    return Err(ClientError::AccountRequest);
                }
                Err(error) => return Err(map_account_client_error(error)),
            }
        }

        if let Err(error) = client.certify_login(&provisional).await {
            if provisional.challenge_expires_at_ms <= now {
                client
                    .logout(&provisional.session.tokens.refresh_token)
                    .await
                    .map_err(map_account_client_error)?;
                delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
                    .map_err(ClientError::KeyStore)?;
            }
            return Err(map_account_client_error(error));
        }
        self.ensure_key_material_covers_local_lists(
            &issuer,
            tenant_id,
            &provisional.session.tokens.access_token,
            &mut provisional.keys,
        )
        .await?;
        let session = account_session_state(
            provisional.session.email.clone(),
            provisional.session.user_id.clone(),
            provisional.session.tenant_id.clone(),
            provisional.session.device_id.clone(),
        );
        let encoded_identity = provisional
            .device_identity
            .encode()
            .map_err(|_| ClientError::AccountRequest)?;
        store_account_secret(
            &self.db_dir,
            AccountSecretKind::DeviceIdentity,
            &encoded_identity,
        )
        .map_err(ClientError::KeyStore)?;
        let crypto = self.persist_account_state_locked(
            &issuer,
            &session,
            &provisional.session.tokens,
            &provisional.local_wrapped_master_key,
            &provisional.keys,
        )?;
        self.set_internal_metadata_value(super::SYNC_SERVER_URL_METADATA_KEY, &issuer)?;
        self.replace_account_runtime(Some(session.clone()), crypto)?;
        Ok(AccountAuthResult {
            session,
            recovery_key: None,
        })
    }

    pub(crate) fn ensure_account_runtime_restored(&self) -> Result<(), ClientError> {
        self.resolve_account_readiness().map(|_| ())
    }

    pub(crate) fn ensure_local_crypto_runtime_restored(&self) -> Result<(), ClientError> {
        if !matches!(self.account_state()?.crypto, CryptoRuntimeState::Unloaded) {
            return Ok(());
        }
        let resolved_crypto = self.load_crypto_runtime()?;
        let mut account = self.account_state()?;
        if matches!(account.crypto, CryptoRuntimeState::Unloaded) {
            account.crypto = resolved_crypto;
        }
        Ok(())
    }

    pub(super) fn resolve_account_readiness(&self) -> Result<AccountReadiness, ClientError> {
        let _session_lock = acquire_session_token_set_lock(&self.db_dir)?;
        let mut credential = load_session_credential(&self.db_dir)?;
        let expired = match credential.as_ref() {
            Some(StoredSessionCredential::Active(tokens)) => {
                tokens.refresh_expires_at_ms <= now_ms()?
            }
            Some(StoredSessionCredential::PendingDeviceCertification(_)) | None => false,
        };
        if expired {
            delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
                .map_err(ClientError::KeyStore)?;
            credential = None;
        }
        let credential_generation = stored_credential_generation(credential.as_ref())?;
        {
            let account = self.account_state()?;
            if account.session_restored
                && account.loaded_credential_generation.as_deref()
                    == Some(credential_generation.as_str())
                && !matches!(account.crypto, CryptoRuntimeState::Unloaded)
            {
                return Ok(classify_account_readiness(
                    credential_kind(credential.as_ref()),
                    crypto_readiness(&account.crypto),
                    account.session.is_some(),
                ));
            }
        }

        self.ensure_local_crypto_runtime_restored()?;
        let crypto_readiness = crypto_readiness(&self.account_state()?.crypto);
        let restored_session = match credential.as_ref() {
            Some(StoredSessionCredential::Active(_)) => {
                let email = self
                    .non_empty_internal_metadata(ACCOUNT_EMAIL_METADATA_KEY)?
                    .ok_or(ClientError::IncompleteAccountState)?;
                let user_id = self
                    .non_empty_internal_metadata(ACCOUNT_USER_ID_METADATA_KEY)?
                    .ok_or(ClientError::IncompleteAccountState)?;
                let tenant_id = self
                    .non_empty_internal_metadata(ACCOUNT_TENANT_ID_METADATA_KEY)?
                    .ok_or(ClientError::IncompleteAccountState)?;
                let device_id = self
                    .non_empty_internal_metadata(ACCOUNT_DEVICE_ID_METADATA_KEY)?
                    .ok_or(ClientError::IncompleteAccountState)?;
                let session_identity = LocalCryptoIdentity {
                    user_id: parse_uuid(&user_id)?,
                    tenant_id: parse_uuid(&tenant_id)?,
                    device_id: parse_uuid(&device_id)?,
                };
                match crypto_readiness {
                    CryptoReadiness::Ready(crypto_identity) => {
                        if session_identity != crypto_identity {
                            return Err(ClientError::ProfileIdentityMismatch);
                        }
                        Some(account_session_state(email, user_id, tenant_id, device_id))
                    }
                    CryptoReadiness::Anonymous | CryptoReadiness::Unavailable => None,
                }
            }
            Some(StoredSessionCredential::PendingDeviceCertification(_)) | None => None,
        };
        let readiness = classify_account_readiness(
            credential_kind(credential.as_ref()),
            crypto_readiness,
            restored_session.is_some(),
        );

        let mut account = self.account_state()?;
        account.session = restored_session;
        account.loaded_credential_generation = Some(credential_generation);
        account.session_restored = true;
        Ok(readiness)
    }

    fn load_crypto_runtime(&self) -> Result<CryptoRuntimeState, ClientError> {
        let active_capsule = self.active_capsule()?;
        let master_key = match active_capsule.wrapped_master_key() {
            Some(local_wrapped_master_key) => {
                let user_id = self
                    .non_empty_internal_metadata(ACCOUNT_USER_ID_METADATA_KEY)?
                    .ok_or(ClientError::IncompleteAccountState)
                    .and_then(|value| parse_uuid(&value))?;
                let device_key = Zeroizing::new(*active_capsule.device_key());
                unwrap_master_key_with_device_key(
                    user_id,
                    INITIAL_KEY_GENERATION,
                    local_wrapped_master_key,
                    &device_key,
                )
                .ok()
            }
            None => None,
        };
        Ok(
            match load_local_crypto_context(&self.db_path, &self.db_key(), master_key)? {
                LocalCryptoAvailability::Ready(crypto) => CryptoRuntimeState::Ready(crypto),
                LocalCryptoAvailability::AccountBoundUnavailable(reason) => {
                    CryptoRuntimeState::Unavailable(reason)
                }
                LocalCryptoAvailability::Anonymous if self.has_legacy_account_binding()? => {
                    CryptoRuntimeState::Unavailable(LocalCryptoUnavailable::MissingMasterKey)
                }
                LocalCryptoAvailability::Anonymous => CryptoRuntimeState::Anonymous,
            },
        )
    }

    pub(super) async fn access_token(
        &self,
        force_refresh: bool,
    ) -> Result<OriginBoundAccessToken, ClientError> {
        self.access_token_with_sync_gate(force_refresh, None).await
    }

    pub(super) async fn access_token_for_sync(
        &self,
        force_refresh: bool,
        network_gate: &mut crate::SqliteSyncStore,
    ) -> Result<OriginBoundAccessToken, ClientError> {
        self.access_token_with_sync_gate(force_refresh, Some(network_gate))
            .await
    }

    async fn access_token_with_sync_gate(
        &self,
        force_refresh: bool,
        mut network_gate: Option<&mut crate::SqliteSyncStore>,
    ) -> Result<OriginBoundAccessToken, ClientError> {
        self.ensure_account_runtime_restored()?;
        let _session_lock = acquire_session_token_set_lock(&self.db_dir)?;
        let mut tokens =
            load_session_tokens(&self.db_dir)?.ok_or(ClientError::CredentialUnavailable)?;
        let now = now_ms()?;
        if tokens.refresh_expires_at_ms <= now {
            self.invalidate_remote_session_locked()?;
            return Err(ClientError::CredentialUnavailable);
        }
        if force_refresh
            || tokens.access_expires_at_ms <= now.saturating_add(ACCESS_TOKEN_REFRESH_SKEW_MS)
        {
            let client =
                AccountClient::new(&tokens.issuer).map_err(|_| ClientError::AccountRequest)?;
            if let Some(gate) = network_gate.as_mut() {
                gate.preflight_network_request()
                    .map_err(super::sync::map_sync_run_error)?;
            }
            let refreshed = match client.refresh(&tokens.refresh_token).await {
                Ok(refreshed) => refreshed,
                Err(AccountClientError::InvalidGrant) => {
                    self.invalidate_remote_session_locked()?;
                    return Err(ClientError::CredentialUnavailable);
                }
                Err(error) => return Err(map_account_client_error(error)),
            };
            tokens = StoredSessionTokens::from_account_tokens(&tokens.issuer, &refreshed);
            store_session_tokens(&self.db_dir, &tokens)?;
        }
        Ok(OriginBoundAccessToken {
            issuer: tokens.issuer.clone(),
            token: Zeroizing::new(tokens.access_token.clone()),
        })
    }

    pub(super) fn current_access_token(
        &self,
    ) -> Result<Option<OriginBoundAccessToken>, ClientError> {
        let _session_lock = acquire_session_token_set_lock(&self.db_dir)?;
        let Some(tokens) = load_session_tokens(&self.db_dir)? else {
            return Ok(None);
        };
        Ok(Some(OriginBoundAccessToken {
            issuer: tokens.issuer.clone(),
            token: Zeroizing::new(tokens.access_token.clone()),
        }))
    }

    fn invalidate_remote_session_locked(&self) -> Result<(), ClientError> {
        delete_account_secret(&self.db_dir, AccountSecretKind::SessionTokens)
            .map_err(ClientError::KeyStore)?;
        let mut account = self.account_state()?;
        account.session = None;
        account.session_restored = true;
        account.loaded_credential_generation = Some(ABSENT_CREDENTIAL_GENERATION.to_string());
        Ok(())
    }

    pub(super) async fn refresh_tenant_keys_for_sync(
        &self,
        lease: taskveil_storage::SyncLease,
    ) -> Result<LocalSyncKeys, ClientError> {
        self.ensure_account_runtime_restored()?;
        let mut network_gate = crate::SqliteSyncStore::new_secret_with_lease(
            self.db_path.clone(),
            self.db_key(),
            lease.clone(),
        );
        let session_token = self.access_token_for_sync(false, &mut network_gate).await?;
        let (tenant_id, user_id, device_id, master_key) = {
            let account = self.account_state()?;
            let Some(_session) = account.session.as_ref().filter(|session| session.logged_in)
            else {
                return Err(ClientError::AccountRequest);
            };
            let CryptoRuntimeState::Ready(crypto) = &account.crypto else {
                return Err(ClientError::AccountBoundUnavailable);
            };
            (
                crypto.tenant_id(),
                crypto.user_id(),
                crypto.device_id(),
                Zeroizing::new(*crypto.master_key()),
            )
        };

        let client =
            AccountClient::new(&session_token.issuer).map_err(|_| ClientError::AccountRequest)?;
        network_gate
            .preflight_network_request()
            .map_err(super::sync::map_sync_run_error)?;
        let bundle = client
            .active_key_bundle(tenant_id, &session_token)
            .await
            .map_err(|_| ClientError::AccountRequest)?;
        let tenant_root_dek = unwrap_active_key_bundle(tenant_id, &bundle, &master_key)
            .map_err(|_| ClientError::AccountBoundUnavailable)?;
        let historical =
            unwrap_historical_key_bundles(tenant_id, &bundle.migrating_generations, &master_key)
                .map_err(|_| ClientError::AccountBoundUnavailable)?;
        let remote_keys = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(tenant_root_dek),
            tenant_generation: bundle.generation,
            historical_tenant_root_deks: historical
                .into_iter()
                .map(|historical| (historical.generation, historical.tenant_root_dek))
                .collect(),
        };
        let local_keys = {
            let account = self.account_state()?;
            let CryptoRuntimeState::Ready(crypto) = &account.crypto else {
                return Err(ClientError::AccountBoundUnavailable);
            };
            crypto.sync_keys().clone()
        };
        if remote_keys.tenant_generation < local_keys.tenant_generation {
            return Err(ClientError::AccountBoundUnavailable);
        }
        self.commit_tenant_key_cutover(
            lease,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id,
            },
            &master_key,
            remote_keys,
        )
    }

    fn commit_tenant_key_cutover(
        &self,
        lease: taskveil_storage::SyncLease,
        identity: LocalCryptoIdentity,
        master_key: &[u8; KEY_LEN],
        sync_keys: LocalSyncKeys,
    ) -> Result<LocalSyncKeys, ClientError> {
        // Remote fetch and unwrap happen before this fenced local cutover.
        // Do not reacquire a profile guard while holding the sync lease: the
        // lease-fenced transaction and epoch-CAS publication preserve the
        // documented profile -> lease -> transaction lock order.
        let lease_epoch = lease.runtime_epoch;
        let previous_generation = {
            let account = self.account_state()?;
            let CryptoRuntimeState::Ready(crypto) = &account.crypto else {
                return Err(ClientError::AccountBoundUnavailable);
            };
            if crypto.tenant_id() != identity.tenant_id
                || crypto.user_id() != identity.user_id
                || crypto.device_id() != identity.device_id
            {
                return Err(ClientError::LeaseLost);
            }
            crypto.sync_keys().tenant_generation
        };
        if sync_keys.tenant_generation < previous_generation {
            return Err(ClientError::LeaseLost);
        }
        let mut cutover_store = crate::SqliteSyncStore::new_secret_with_lease(
            self.db_path.clone(),
            self.db_key(),
            lease,
        );
        let mut transaction = cutover_store
            .begin_write_transaction()
            .map_err(super::sync::map_sync_run_error)?;
        if sync_keys.tenant_generation > previous_generation {
            let snapshot = transaction
                .rotation_backfill_snapshot()
                .map_err(super::sync::map_sync_run_error)?;
            let mut clock = || now_ms().map_err(|error| error.to_string());
            taskveil_sync::enqueue_rotation_backfill(
                &mut transaction,
                &sync_keys,
                &identity.device_id.to_string(),
                taskveil_sync::BackfillRecords {
                    lists: &snapshot.lists,
                    templates: &snapshot.templates,
                    task_series: &snapshot.schedules,
                    tasks: &snapshot.tasks,
                    timer_sessions: &snapshot.timer_sessions,
                },
                &mut clock,
            )
            .map_err(super::sync::map_sync_run_error)?;
        }
        let cutover_now = now_ms()?;
        let crypto = transaction
            .persist_local_crypto_context(identity, master_key, sync_keys.clone(), cutover_now)
            .map_err(super::sync::map_sync_run_error)?;
        transaction
            .set_setting(
                taskveil_sync::KEY_ROTATION_PENDING_METADATA_KEY,
                "0",
                cutover_now,
            )
            .map_err(super::sync::map_sync_run_error)?;
        let expected_runtime_epoch = if transaction.has_runtime_cutover() {
            lease_epoch.checked_add(1).ok_or(ClientError::LeaseLost)?
        } else {
            lease_epoch
        };
        transaction
            .commit()
            .map_err(super::sync::map_sync_run_error)?;
        let coordination = SqliteProfileCoordinationRepository::new(open_encrypted(
            &self.db_path,
            &self.db_key(),
        )?);
        // Serialize in-memory account publication, then reject a stale
        // post-commit publisher if another profile transition won either the
        // durable epoch or this instance's atomic epoch in the meantime.
        let mut account = self.account_state()?;
        let runtime = coordination.load_runtime()?;
        if runtime.runtime_epoch != expected_runtime_epoch {
            return Err(ClientError::LeaseLost);
        }
        self.publish_runtime_epoch_if_current(lease_epoch, expected_runtime_epoch)?;
        account.crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        Ok(sync_keys)
    }

    pub(super) fn local_lists_including_archived(
        &self,
    ) -> Result<Vec<taskveil_domain::List>, ClientError> {
        self.with_list_repository(|repository| {
            let mut lists = repository.list_all()?;
            lists.extend(repository.list_archived()?);
            Ok(lists)
        })
    }

    async fn ensure_key_material_covers_local_lists(
        &self,
        _server_url: &str,
        _tenant_id: Uuid,
        _session_token: &str,
        keys: &mut AccountKeyMaterial,
    ) -> Result<(), ClientError> {
        match load_local_crypto_context(&self.db_path, &self.db_key(), Some(*keys.master_key))? {
            LocalCryptoAvailability::Ready(local) => {
                keys.tenant_generation = local.sync_keys().tenant_generation;
                keys.tenant_root_dek = local
                    .sync_keys()
                    .tenant_root_dek
                    .clone()
                    .ok_or(ClientError::AccountBoundUnavailable)?;
                return Ok(());
            }
            LocalCryptoAvailability::AccountBoundUnavailable(_) => {
                return Err(ClientError::AccountBoundUnavailable);
            }
            LocalCryptoAvailability::Anonymous => {}
        }
        Ok(())
    }

    fn ensure_profile_is_unbound_for_registration(&self) -> Result<(), ClientError> {
        if self.has_profile_binding()? || self.has_legacy_account_binding()? {
            return Err(ClientError::ProfileAlreadyBound);
        }
        Ok(())
    }

    fn validate_existing_profile_identity(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), ClientError> {
        let connection = open_encrypted(&self.db_path, &self.db_key())?;
        if let Some(binding) = SqliteLocalCryptoRepository::new(connection).load_binding()? {
            if binding.tenant_id != tenant_id || binding.user_id != user_id {
                return Err(ClientError::ProfileIdentityMismatch);
            }
        } else if self.has_legacy_account_binding()? {
            let legacy_tenant = self
                .non_empty_internal_metadata(ACCOUNT_TENANT_ID_METADATA_KEY)?
                .ok_or(ClientError::IncompleteAccountState)?;
            let legacy_user = self
                .non_empty_internal_metadata(ACCOUNT_USER_ID_METADATA_KEY)?
                .ok_or(ClientError::IncompleteAccountState)?;
            if parse_uuid(&legacy_tenant)? != tenant_id || parse_uuid(&legacy_user)? != user_id {
                return Err(ClientError::ProfileIdentityMismatch);
            }
        }
        Ok(())
    }

    fn verify_local_safety_participant(
        &self,
        response: &taskveil_sync::organization::OrganizationSafetyResponse,
    ) -> Result<(), ClientError> {
        let local_user_id = parse_uuid(
            &self
                .non_empty_internal_metadata(ACCOUNT_USER_ID_METADATA_KEY)?
                .ok_or(ClientError::IncompleteAccountState)?,
        )?;
        let local_root = self
            .non_empty_internal_metadata(ACCOUNT_ROOT_PUBLIC_METADATA_KEY)?
            .ok_or(ClientError::IncompleteAccountState)?;
        let expected_local_root = if response.owner_user_id == local_user_id {
            &response.owner_root_public
        } else if response.member_user_id == local_user_id {
            &response.member_root_public
        } else {
            return Err(ClientError::AccountRequest);
        };
        if expected_local_root != &local_root {
            return Err(ClientError::AccountRequest);
        }
        let decoded = AccountRootPublicKeys::decode(
            &STANDARD
                .decode(local_root)
                .map_err(|_| ClientError::AccountRequest)?,
        )
        .map_err(|_| ClientError::AccountRequest)?;
        if decoded.user_id != local_user_id {
            return Err(ClientError::AccountRequest);
        }
        Ok(())
    }

    fn organization_trust_pin_key(tenant_id: Uuid, member_user_id: Uuid) -> String {
        format!("organization_trust:{tenant_id}:{member_user_id}")
    }

    fn load_organization_trust_pin(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
    ) -> Result<Option<OrganizationTrustPin>, ClientError> {
        let Some(value) =
            self.internal_metadata(&Self::organization_trust_pin_key(tenant_id, member_user_id))?
        else {
            return Ok(None);
        };
        OrganizationTrustPin::decode(&value)
            .map(Some)
            .ok_or(ClientError::AccountRequest)
    }

    fn store_organization_trust_pin(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        pin: &OrganizationTrustPin,
    ) -> Result<(), ClientError> {
        self.set_internal_metadata_value(
            &Self::organization_trust_pin_key(tenant_id, member_user_id),
            &pin.encode(),
        )
    }

    fn load_account_root_keys(
        &self,
    ) -> Result<(AccountRootPrivateKeys, AccountRootPublicKeys), ClientError> {
        let (user_id, master_key) = {
            let state = self.account_state()?;
            let CryptoRuntimeState::Ready(crypto) = &state.crypto else {
                return Err(ClientError::AccountBoundUnavailable);
            };
            (crypto.user_id(), Zeroizing::new(*crypto.master_key()))
        };
        let generation = self
            .non_empty_internal_metadata(ACCOUNT_MK_GENERATION_METADATA_KEY)?
            .ok_or(ClientError::IncompleteAccountState)?
            .parse::<u64>()
            .map_err(|_| ClientError::IncompleteAccountState)?;
        let wrapped = load_account_secret(&self.db_dir, AccountSecretKind::WrappedAccountRoot)
            .map_err(ClientError::KeyStore)?
            .ok_or(ClientError::IncompleteAccountState)?;
        let private_bytes = unwrap_account_root_private_key_with_master_key(
            user_id,
            generation,
            &wrapped,
            &master_key,
        )
        .map_err(|_| ClientError::AccountBoundUnavailable)?;
        let private = AccountRootPrivateKeys::decode(&*private_bytes)
            .map_err(|_| ClientError::AccountBoundUnavailable)?;
        let public = AccountRootPublicKeys::decode(
            &STANDARD
                .decode(
                    self.non_empty_internal_metadata(ACCOUNT_ROOT_PUBLIC_METADATA_KEY)?
                        .ok_or(ClientError::IncompleteAccountState)?,
                )
                .map_err(|_| ClientError::IncompleteAccountState)?,
        )
        .map_err(|_| ClientError::IncompleteAccountState)?;
        if private
            .public_keys(user_id)
            .map_err(|_| ClientError::AccountBoundUnavailable)?
            != public
        {
            return Err(ClientError::AccountBoundUnavailable);
        }
        Ok((private, public))
    }

    fn persist_account_state_locked(
        &self,
        issuer: &str,
        session: &AccountSessionState,
        tokens: &AccountTokenSet,
        local_wrapped_master_key: &[u8],
        keys: &AccountKeyMaterial,
    ) -> Result<crate::LocalCryptoContext, ClientError> {
        let identity = LocalCryptoIdentity {
            tenant_id: parse_session_id(session.tenant_id.as_deref())?,
            user_id: parse_session_id(session.user_id.as_deref())?,
            device_id: parse_session_id(session.device_id.as_deref())?,
        };
        let persistence_now = now_ms()?;
        let db_key = self.db_key();
        let mut transaction =
            crate::SqliteSyncStore::begin_profile_cutover_transaction(&self.db_path, &db_key)
                .map_err(|_| ClientError::Sync)?;
        let mut fixed_now = || Ok(persistence_now);
        rebind_local_device(
            &mut transaction,
            &identity.device_id.to_string(),
            &mut fixed_now,
        )
        .map_err(|_| ClientError::Sync)?;
        transaction
            .set_setting(
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                &identity.device_id.to_string(),
                persistence_now,
            )
            .map_err(|_| ClientError::Sync)?;
        let crypto = transaction
            .persist_local_crypto_context(
                identity,
                &keys.master_key,
                LocalSyncKeys::from_account_keys(identity.tenant_id, keys),
                persistence_now,
            )
            .map_err(|_| ClientError::Sync)?;
        transaction.commit().map_err(|_| ClientError::Sync)?;
        self.store_active_wrapped_master_key(local_wrapped_master_key.to_vec())?;
        self.set_internal_metadata_value(
            ACCOUNT_EMAIL_METADATA_KEY,
            session.email.as_deref().unwrap_or_default(),
        )?;
        self.set_internal_metadata_value(
            ACCOUNT_USER_ID_METADATA_KEY,
            session.user_id.as_deref().unwrap_or_default(),
        )?;
        self.set_internal_metadata_value(
            ACCOUNT_TENANT_ID_METADATA_KEY,
            session.tenant_id.as_deref().unwrap_or_default(),
        )?;
        self.set_internal_metadata_value(
            ACCOUNT_DEVICE_ID_METADATA_KEY,
            session.device_id.as_deref().unwrap_or_default(),
        )?;
        self.set_internal_metadata_value(
            ACCOUNT_ROOT_PUBLIC_METADATA_KEY,
            &STANDARD.encode(
                keys.account_root_public
                    .encode()
                    .map_err(|_| ClientError::AccountBoundUnavailable)?,
            ),
        )?;
        self.set_internal_metadata_value(
            ACCOUNT_MK_GENERATION_METADATA_KEY,
            &keys.generation.to_string(),
        )?;
        let root_private = keys.account_root_private.encode();
        let wrapped_root = wrap_account_root_private_key_with_master_key(
            identity.user_id,
            keys.generation,
            &root_private,
            &keys.master_key,
        )
        .map_err(|_| ClientError::AccountBoundUnavailable)?;
        store_account_secret(
            &self.db_dir,
            AccountSecretKind::WrappedAccountRoot,
            &wrapped_root,
        )
        .map_err(ClientError::KeyStore)?;
        // Publishing the active credential is the final durable step. Until
        // this succeeds, the pending-login payload remains available for an
        // idempotent certification/finalization retry after a crash.
        store_session_tokens(
            &self.db_dir,
            &StoredSessionTokens::from_account_tokens(issuer, tokens),
        )?;
        Ok(crypto)
    }

    #[cfg(test)]
    fn rebind_sync_device_locked(
        &self,
        device_id: Uuid,
        persistence_now: i64,
    ) -> Result<(), ClientError> {
        let db_key = self.db_key();
        let mut transaction =
            crate::SqliteSyncStore::begin_profile_cutover_transaction(&self.db_path, &db_key)
                .map_err(|_| ClientError::Sync)?;
        let old_clock = transaction
            .get_setting(SYNC_LOCAL_HLC_METADATA_KEY)
            .map_err(|_| ClientError::Sync)?;
        let old_device = transaction
            .get_setting(ACCOUNT_DEVICE_ID_METADATA_KEY)
            .map_err(|_| ClientError::Sync)?;
        let mut fixed_now = || Ok(persistence_now);
        rebind_local_device(&mut transaction, &device_id.to_string(), &mut fixed_now)
            .map_err(|_| ClientError::Sync)?;
        let new_clock = transaction
            .get_setting(SYNC_LOCAL_HLC_METADATA_KEY)
            .map_err(|_| ClientError::Sync)?;
        transaction
            .set_setting(
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                &device_id.to_string(),
                persistence_now,
            )
            .map_err(|_| ClientError::Sync)?;
        if old_clock != new_clock || old_device.as_deref() != Some(device_id.to_string().as_str()) {
            transaction
                .bump_runtime_epoch(persistence_now)
                .map_err(|_| ClientError::Sync)?;
        }
        transaction.commit().map_err(|_| ClientError::Sync)
    }

    fn replace_account_runtime(
        &self,
        session: Option<AccountSessionState>,
        crypto: crate::LocalCryptoContext,
    ) -> Result<(), ClientError> {
        let mut state = self.account_state()?;
        state.session = session;
        state.session_restored = true;
        // The protected credential was published immediately before this
        // runtime update. Force the next session-dependent operation to
        // observe the exact generation committed by the platform store.
        state.loaded_credential_generation = None;
        state.crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        Ok(())
    }

    fn has_legacy_account_binding(&self) -> Result<bool, ClientError> {
        for key in [
            ACCOUNT_EMAIL_METADATA_KEY,
            ACCOUNT_USER_ID_METADATA_KEY,
            ACCOUNT_TENANT_ID_METADATA_KEY,
            ACCOUNT_DEVICE_ID_METADATA_KEY,
            ACCOUNT_SESSION_EXPIRES_AT_METADATA_KEY,
        ] {
            if self.non_empty_internal_metadata(key)?.is_some() {
                return Ok(true);
            }
        }
        if self.active_capsule()?.wrapped_master_key().is_some() {
            return Ok(true);
        }
        Ok(false)
    }

    fn active_capsule(&self) -> Result<taskveil_crypto::LocalKeyCapsule, ClientError> {
        PlatformLocalKeyCapsuleStore::new(&self.db_dir)
            .load(LocalKeyCapsuleSlot::Active)
            .map_err(ClientError::KeyStore)?
            .ok_or(ClientError::LocalKeyState)
    }

    fn store_active_wrapped_master_key(
        &self,
        wrapped_master_key: Vec<u8>,
    ) -> Result<(), ClientError> {
        let mut store = PlatformLocalKeyCapsuleStore::new(&self.db_dir);
        let active = store
            .load(LocalKeyCapsuleSlot::Active)
            .map_err(ClientError::KeyStore)?
            .ok_or(ClientError::LocalKeyState)?;
        let updated = active
            .with_wrapped_master_key(Some(wrapped_master_key))
            .map_err(ClientError::KeyStore)?;
        store
            .store(LocalKeyCapsuleSlot::Active, &updated)
            .map_err(ClientError::KeyStore)
    }
}

fn map_account_client_error(error: AccountClientError) -> ClientError {
    match error {
        AccountClientError::EntitlementRequired => ClientError::EntitlementRequired,
        _ => ClientError::AccountRequest,
    }
}

fn billing_state(response: BillingResponseDto) -> BillingState {
    BillingState {
        provider: response.provider,
        provider_app_user_id: response.provider_app_user_id.to_string(),
        lookup_key: response.entitlement.lookup_key,
        status: response.entitlement.status,
        sync_allowed: response.entitlement.sync_allowed,
        store_product_identifier: response.entitlement.store_product_identifier,
        expires_at: response.entitlement.expires_at,
        grace_expires_at: response.entitlement.grace_expires_at,
        will_renew: response.entitlement.will_renew,
        environment: response.entitlement.environment,
        updated_at: response.entitlement.updated_at,
    }
}

fn load_session_credential(
    db_dir: &std::path::Path,
) -> Result<Option<StoredSessionCredential>, ClientError> {
    let Some(encoded) = load_account_secret(db_dir, AccountSecretKind::SessionTokens)
        .map_err(ClientError::KeyStore)?
    else {
        return Ok(None);
    };
    let encoded = Zeroizing::new(encoded);
    let credential: StoredSessionCredential =
        serde_json::from_slice(&encoded).map_err(|_| ClientError::IncompleteAccountState)?;
    match &credential {
        StoredSessionCredential::Active(tokens) => tokens.validate()?,
        StoredSessionCredential::PendingDeviceCertification(pending) => pending.validate()?,
    }
    Ok(Some(credential))
}

fn stored_credential_generation(
    credential: Option<&StoredSessionCredential>,
) -> Result<String, ClientError> {
    match credential {
        Some(StoredSessionCredential::Active(tokens)) => {
            if let Some(generation) = &tokens.credential_generation {
                return Ok(generation.clone());
            }
        }
        Some(StoredSessionCredential::PendingDeviceCertification(pending)) => {
            if let Some(generation) = &pending.credential_generation {
                return Ok(generation.clone());
            }
        }
        None => return Ok(ABSENT_CREDENTIAL_GENERATION.to_string()),
    }
    let encoded = Zeroizing::new(
        serde_json::to_vec(credential.ok_or(ClientError::IncompleteAccountState)?)
            .map_err(|_| ClientError::IncompleteAccountState)?,
    );
    let digest = Sha256::digest(&*encoded);
    Ok(format!("legacy:{}", STANDARD.encode(digest)))
}

pub(super) fn load_session_tokens(
    db_dir: &std::path::Path,
) -> Result<Option<StoredSessionTokens>, ClientError> {
    Ok(match load_session_credential(db_dir)? {
        Some(StoredSessionCredential::Active(tokens)) => Some(tokens),
        Some(StoredSessionCredential::PendingDeviceCertification(_)) | None => None,
    })
}

pub(super) fn stored_session_credential_issuer(
    db_dir: &std::path::Path,
) -> Result<Option<String>, ClientError> {
    Ok(match load_session_credential(db_dir)? {
        Some(StoredSessionCredential::Active(tokens)) => Some(tokens.issuer.clone()),
        Some(StoredSessionCredential::PendingDeviceCertification(pending)) => {
            Some(pending.issuer.clone())
        }
        None => None,
    })
}

fn load_pending_login(db_dir: &std::path::Path) -> Result<Option<StoredPendingLogin>, ClientError> {
    Ok(match load_session_credential(db_dir)? {
        Some(StoredSessionCredential::PendingDeviceCertification(pending)) => Some(*pending),
        Some(StoredSessionCredential::Active(_)) | None => None,
    })
}

fn store_session_tokens(
    db_dir: &std::path::Path,
    tokens: &StoredSessionTokens,
) -> Result<(), ClientError> {
    tokens.validate()?;
    let encoded = Zeroizing::new(
        serde_json::to_vec(&StoredSessionCredential::Active(
            StoredSessionTokens::from_account_tokens(
                &tokens.issuer,
                &AccountTokenSet {
                    access_token: Zeroizing::new(tokens.access_token.clone()),
                    access_expires_at_ms: tokens.access_expires_at_ms,
                    refresh_token: Zeroizing::new(tokens.refresh_token.clone()),
                    refresh_expires_at_ms: tokens.refresh_expires_at_ms,
                },
            ),
        ))
        .map_err(|_| ClientError::IncompleteAccountState)?,
    );
    store_account_secret(db_dir, AccountSecretKind::SessionTokens, &encoded)
        .map_err(ClientError::KeyStore)
}

fn store_pending_login(
    db_dir: &std::path::Path,
    pending: StoredPendingLogin,
) -> Result<(), ClientError> {
    pending.validate()?;
    let encoded = Zeroizing::new(
        serde_json::to_vec(&StoredSessionCredential::PendingDeviceCertification(
            Box::new(pending),
        ))
        .map_err(|_| ClientError::IncompleteAccountState)?,
    );
    store_account_secret(db_dir, AccountSecretKind::SessionTokens, &encoded)
        .map_err(ClientError::KeyStore)
}

pub(super) fn acquire_session_token_set_lock(
    db_dir: &std::path::Path,
) -> Result<crate::profile_coordination::SessionCredentialGuard, ClientError> {
    crate::profile_coordination::ProfileCoordinator::for_profile(db_dir)?.try_session()
}

fn account_session_state(
    email: String,
    user_id: String,
    tenant_id: String,
    device_id: String,
) -> AccountSessionState {
    AccountSessionState {
        logged_in: true,
        email: Some(email),
        user_id: Some(user_id),
        tenant_id: Some(tenant_id),
        device_id: Some(device_id),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialKind {
    Absent,
    Active,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CryptoReadiness {
    Anonymous,
    Ready(LocalCryptoIdentity),
    Unavailable,
}

fn credential_kind(credential: Option<&StoredSessionCredential>) -> CredentialKind {
    match credential {
        Some(StoredSessionCredential::Active(_)) => CredentialKind::Active,
        Some(StoredSessionCredential::PendingDeviceCertification(_)) => CredentialKind::Pending,
        None => CredentialKind::Absent,
    }
}

fn crypto_readiness(crypto: &CryptoRuntimeState) -> CryptoReadiness {
    match crypto {
        CryptoRuntimeState::Anonymous => CryptoReadiness::Anonymous,
        CryptoRuntimeState::Ready(crypto) => CryptoReadiness::Ready(LocalCryptoIdentity {
            tenant_id: crypto.tenant_id(),
            user_id: crypto.user_id(),
            device_id: crypto.device_id(),
        }),
        CryptoRuntimeState::Unavailable(_) | CryptoRuntimeState::Unloaded => {
            CryptoReadiness::Unavailable
        }
    }
}

fn classify_account_readiness(
    credential: CredentialKind,
    crypto: CryptoReadiness,
    has_validated_session: bool,
) -> AccountReadiness {
    if crypto == CryptoReadiness::Unavailable {
        return AccountReadiness::AccountBoundUnavailable;
    }
    match credential {
        CredentialKind::Absent => match crypto {
            CryptoReadiness::Anonymous => AccountReadiness::LoggedOut,
            CryptoReadiness::Ready(_) => AccountReadiness::CredentialUnavailable,
            CryptoReadiness::Unavailable => AccountReadiness::AccountBoundUnavailable,
        },
        CredentialKind::Pending => AccountReadiness::CredentialUnavailable,
        CredentialKind::Active
            if matches!(crypto, CryptoReadiness::Ready(_)) && has_validated_session =>
        {
            AccountReadiness::Ready
        }
        CredentialKind::Active => AccountReadiness::AccountBoundUnavailable,
    }
}

fn parse_session_id(value: Option<&str>) -> Result<Uuid, ClientError> {
    parse_uuid(value.ok_or(ClientError::IncompleteAccountState)?)
}

fn parse_uuid(value: &str) -> Result<Uuid, ClientError> {
    value
        .parse::<Uuid>()
        .map_err(|_| ClientError::IncompleteAccountState)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        process::{Command, Stdio},
        sync::Mutex,
    };

    use taskveil_domain::new_list;
    use taskveil_storage::{
        ListRepository, SqliteListRepository, SqliteProfileCoordinationRepository,
        SqliteTaskRepository, StorageError, TaskRepository,
    };
    use taskveil_sync::{
        EncryptedSyncState, Hlc, LocalSyncKeys, LocalSyncStore, NewLocalSyncOutboxEntry,
        SyncCollection, SYNC_LOCAL_HLC_METADATA_KEY,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::{profile_coordination::ProfileCoordinator, SqliteSyncStore};

    fn open_test_client(db_dir: &std::path::Path, db_key: [u8; 32]) -> TaskveilClient {
        let db_path = db_dir.join("taskveil.db");
        drop(open_encrypted(&db_path, &db_key).expect("open encrypted test database"));
        TaskveilClient {
            db_dir: db_dir.to_path_buf(),
            profile_coordinator: TaskveilClient::pinned_test_coordinator(db_dir, &db_path),
            db_path,
            db_key: Mutex::new(Zeroizing::new(db_key)),
            account: Mutex::new(super::super::AccountRuntimeState {
                session: None,
                session_restored: false,
                loaded_credential_generation: None,
                crypto: CryptoRuntimeState::Anonymous,
            }),
            sync: Mutex::new(super::super::SyncRuntimeState::default()),
            runtime_epoch: std::sync::atomic::AtomicI64::new(1),
            capsule_generation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn spawn_stale_runtime_child(
        profile: &std::path::Path,
        mode: &str,
        list_id: Uuid,
    ) -> (std::process::Child, BufReader<std::process::ChildStdout>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::account::tests::child_stale_runtime_actor",
                "--nocapture",
            ])
            .env("TASKVEIL_STALE_RUNTIME_CHILD", mode)
            .env("TASKVEIL_STALE_RUNTIME_PROFILE", profile)
            .env("TASKVEIL_STALE_RUNTIME_LIST", list_id.to_string())
            // The child is a standalone test process, so Apple does not see
            // Rust's `cfg(test)` when selecting the capsule backend.
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
            if line.contains("TASKVEIL_STALE_RUNTIME_READY") {
                break;
            }
        }
        (child, output)
    }

    #[test]
    fn child_stale_runtime_actor() {
        let Ok(mode) = std::env::var("TASKVEIL_STALE_RUNTIME_CHILD") else {
            return;
        };
        let profile =
            std::path::PathBuf::from(std::env::var_os("TASKVEIL_STALE_RUNTIME_PROFILE").unwrap());
        let list_id = std::env::var("TASKVEIL_STALE_RUNTIME_LIST")
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        const DB_KEY: [u8; 32] = [0xb6; 32];
        const MASTER_KEY: [u8; KEY_LEN] = [0x71; KEY_LEN];
        let client = open_test_client(&profile, DB_KEY);
        if mode == "ready" {
            let LocalCryptoAvailability::Ready(crypto) =
                load_local_crypto_context(client.db_path(), &DB_KEY, Some(MASTER_KEY)).unwrap()
            else {
                panic!("ready child could not restore local crypto");
            };
            let runtime = SqliteProfileCoordinationRepository::new(
                open_encrypted(client.db_path(), &DB_KEY).unwrap(),
            )
            .load_runtime()
            .unwrap();
            client
                .runtime_epoch
                .store(runtime.runtime_epoch, std::sync::atomic::Ordering::Release);
            client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(crypto);
        } else {
            assert!(matches!(
                mode.as_str(),
                "anonymous" | "root-swap" | "windows-root-handle"
            ));
        }
        println!("TASKVEIL_STALE_RUNTIME_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();

        let result = client.create_task(super::super::CreateTaskCommand {
            list_id,
            title: format!("must not commit from stale {mode}"),
            parent_task_id: None,
            due: None,
            note: None,
            priority: 0,
            scheduled_at: None,
            estimated_minutes: None,
        });
        match mode.as_str() {
            "root-swap" => assert!(matches!(result, Err(ClientError::ProfileLockUnsupported))),
            "windows-root-handle" => {
                result.expect("the pinned Windows profile remains usable while rename is denied");
            }
            _ => assert!(
                matches!(
                    result,
                    Err(ClientError::LocalKeyState | ClientError::AccountBoundUnavailable)
                ),
                "stale runtime mutation returned an unexpected result: {result:?}"
            ),
        }
    }

    #[test]
    fn versioned_session_token_set_round_trips_as_one_payload() {
        let expected = StoredSessionTokens {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: "https://sync.example.com".to_string(),
            access_token: "access-secret".to_string(),
            access_expires_at_ms: 1_800_000_000_000,
            refresh_token: "refresh-secret".to_string(),
            refresh_expires_at_ms: 1_801_000_000_000,
        };
        let encoded = Zeroizing::new(serde_json::to_vec(&expected).expect("encode token set"));
        let loaded: StoredSessionTokens =
            serde_json::from_slice(&encoded).expect("decode token set");
        loaded.validate().expect("validate token set");
        assert_eq!(loaded.version, SESSION_TOKEN_SET_VERSION);
        assert_eq!(loaded.issuer, "https://sync.example.com");
        assert_eq!(loaded.access_token, "access-secret");
        assert_eq!(loaded.refresh_token, "refresh-secret");
        assert_eq!(loaded.access_expires_at_ms, 1_800_000_000_000);
        assert_eq!(loaded.refresh_expires_at_ms, 1_801_000_000_000);

        assert!(serde_json::from_slice::<StoredSessionTokens>(b"legacy-single-token").is_err());
    }

    #[test]
    fn expired_challenge_with_still_valid_access_retries_certification_before_refresh() {
        let challenge_expires_at_ms = 10 * 60 * 1_000;
        let now_ms = challenge_expires_at_ms + 1;
        let access_expires_at_ms = 15 * 60 * 1_000;

        assert_eq!(
            pending_login_network_step(now_ms, access_expires_at_ms),
            PendingLoginNetworkStep::Certify
        );
        assert_eq!(
            pending_login_network_step(access_expires_at_ms, access_expires_at_ms),
            PendingLoginNetworkStep::RefreshCertifiedDevice
        );
    }

    #[test]
    fn active_credential_blocks_origin_change() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x31; 32]);
        let tokens = StoredSessionTokens {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: "https://sync.example.com".to_string(),
            access_token: "access-secret".to_string(),
            access_expires_at_ms: 1_900_000_000_000,
            refresh_token: "refresh-secret".to_string(),
            refresh_expires_at_ms: 1_901_000_000_000,
        };
        store_session_tokens(temp.path(), &tokens).expect("store token set");

        assert!(client
            .set_sync_server_url("https://attacker.example".to_string())
            .is_err());
        client
            .set_sync_server_url("HTTPS://SYNC.EXAMPLE.COM:443/".to_string())
            .expect("same canonical origin");
        assert_eq!(
            client.sync_server_url().unwrap(),
            "https://sync.example.com"
        );
        delete_account_secret(temp.path(), AccountSecretKind::SessionTokens)
            .expect("remove test token");
    }

    #[test]
    fn active_credential_rejects_mismatched_stored_origin_on_read() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x32; 32]);
        client
            .set_internal_metadata_value(
                super::super::SYNC_SERVER_URL_METADATA_KEY,
                "https://attacker.example",
            )
            .expect("seed migrated metadata");
        let tokens = StoredSessionTokens {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: "https://sync.example.com".to_string(),
            access_token: "access-secret".to_string(),
            access_expires_at_ms: 1_900_000_000_000,
            refresh_token: "refresh-secret".to_string(),
            refresh_expires_at_ms: 1_901_000_000_000,
        };
        store_session_tokens(temp.path(), &tokens).expect("store token set");

        let result = client.sync_server_url_unlocked();
        assert!(
            matches!(result, Err(ClientError::AccountRequest)),
            "unexpected result: {result:?}"
        );
        client
            .set_internal_metadata_value(super::super::SYNC_SERVER_URL_METADATA_KEY, " ")
            .expect("seed blank migrated metadata");
        assert_eq!(
            client.sync_server_url_unlocked().unwrap(),
            "https://sync.example.com"
        );
        delete_account_secret(temp.path(), AccountSecretKind::SessionTokens)
            .expect("remove test token");
    }

    #[test]
    fn invalid_migrated_server_url_is_rejected_on_read() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x33; 32]);
        client
            .set_internal_metadata_value(
                super::super::SYNC_SERVER_URL_METADATA_KEY,
                "https://sync.example.com/path",
            )
            .expect("seed migrated metadata");

        assert!(matches!(
            client.sync_server_url_unlocked(),
            Err(ClientError::AccountRequest)
        ));
    }

    #[tokio::test]
    async fn logout_retains_credential_until_remote_revocation_succeeds() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for status in ["500 Internal Server Error", "200 OK"] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).expect("read request");
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 2\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{{}}"
                )
                .expect("write response");
            }
        });
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x32; 32]);
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer,
                access_token: "access-secret".to_string(),
                access_expires_at_ms: 1_900_000_000_000,
                refresh_token: "refresh-secret".to_string(),
                refresh_expires_at_ms: 1_901_000_000_000,
            },
        )
        .expect("store token set");

        assert!(client.account_logout().await.is_err());
        assert!(load_session_tokens(temp.path()).unwrap().is_some());
        client
            .account_logout()
            .await
            .expect("remote revocation succeeds");
        assert!(load_session_credential(temp.path()).unwrap().is_none());
        server.join().expect("test server thread");
    }

    #[test]
    fn process_restart_restores_origin_bound_session() {
        const DB_KEY: [u8; 32] = [0x33; 32];
        const MASTER_KEY: [u8; KEY_LEN] = [0x34; KEY_LEN];
        let temp = TempDir::new().expect("temp profile");
        let first = open_test_client(temp.path(), DB_KEY);
        let identity = LocalCryptoIdentity {
            user_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            tenant_id: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            device_id: "00000000-0000-4000-8000-000000000003".parse().unwrap(),
        };
        for (key, value) in [
            (
                ACCOUNT_EMAIL_METADATA_KEY,
                "restart@example.com".to_string(),
            ),
            (ACCOUNT_USER_ID_METADATA_KEY, identity.user_id.to_string()),
            (
                ACCOUNT_TENANT_ID_METADATA_KEY,
                identity.tenant_id.to_string(),
            ),
            (
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                identity.device_id.to_string(),
            ),
        ] {
            first.set_internal_metadata_value(key, &value).unwrap();
        }
        let crypto = persist_local_crypto_context(
            first.db_path(),
            &DB_KEY,
            identity,
            &MASTER_KEY,
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x35; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer: "https://sync.example.com".to_string(),
                access_token: "access-secret".to_string(),
                access_expires_at_ms: 1_900_000_000_000,
                refresh_token: "refresh-secret".to_string(),
                refresh_expires_at_ms: 1_901_000_000_000,
            },
        )
        .unwrap();
        drop(first);

        let restarted = open_test_client(temp.path(), DB_KEY);
        let runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(restarted.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        restarted
            .runtime_epoch
            .store(runtime.runtime_epoch, std::sync::atomic::Ordering::Release);
        restarted.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        let session = restarted.account_session_state().unwrap();
        assert!(session.logged_in);
        assert_eq!(session.email.as_deref(), Some("restart@example.com"));
        assert_eq!(
            stored_session_credential_issuer(temp.path()).unwrap(),
            Some("https://sync.example.com".to_string())
        );
        delete_account_secret(temp.path(), AccountSecretKind::SessionTokens).unwrap();
    }

    #[test]
    fn live_profile_observes_authoritative_credential_replacement_and_deletion() {
        const DB_KEY: [u8; 32] = [0x35; 32];
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        let identity = LocalCryptoIdentity {
            user_id: "00000000-0000-4000-8000-000000000011".parse().unwrap(),
            tenant_id: "00000000-0000-4000-8000-000000000012".parse().unwrap(),
            device_id: "00000000-0000-4000-8000-000000000013".parse().unwrap(),
        };
        for (key, value) in [
            (
                ACCOUNT_EMAIL_METADATA_KEY,
                "converge@example.com".to_string(),
            ),
            (ACCOUNT_USER_ID_METADATA_KEY, identity.user_id.to_string()),
            (
                ACCOUNT_TENANT_ID_METADATA_KEY,
                identity.tenant_id.to_string(),
            ),
            (
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                identity.device_id.to_string(),
            ),
        ] {
            client.set_internal_metadata_value(key, &value).unwrap();
        }
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &[0x36; KEY_LEN],
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x37; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        let runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        client
            .runtime_epoch
            .store(runtime.runtime_epoch, std::sync::atomic::Ordering::Release);
        client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        let token_set = |access_token: &str| StoredSessionTokens {
            version: SESSION_TOKEN_SET_VERSION,
            credential_generation: Some(Uuid::now_v7().to_string()),
            issuer: "https://sync.example.com".to_string(),
            access_token: access_token.to_string(),
            access_expires_at_ms: 1_900_000_000_000,
            refresh_token: format!("{access_token}-refresh"),
            refresh_expires_at_ms: 1_901_000_000_000,
        };

        store_session_tokens(temp.path(), &token_set("first")).unwrap();
        assert!(client.account_session_state().unwrap().logged_in);
        assert_eq!(
            client
                .current_access_token()
                .unwrap()
                .unwrap()
                .token
                .as_str(),
            "first"
        );
        let first_generation = client
            .account_state()
            .unwrap()
            .loaded_credential_generation
            .clone();

        // This direct replacement models a refresh committed by another
        // process. The next operation must reread the protected store while
        // holding the session lock instead of using its cached token family.
        store_session_tokens(temp.path(), &token_set("second")).unwrap();
        assert_eq!(
            client
                .current_access_token()
                .unwrap()
                .unwrap()
                .token
                .as_str(),
            "second"
        );
        assert!(client.account_session_state().unwrap().logged_in);
        assert_ne!(
            client.account_state().unwrap().loaded_credential_generation,
            first_generation
        );

        delete_account_secret(temp.path(), AccountSecretKind::SessionTokens).unwrap();
        assert!(matches!(
            client.account_session_state(),
            Err(ClientError::CredentialUnavailable)
        ));
        assert!(client.current_access_token().unwrap().is_none());
    }

    #[test]
    fn account_readiness_state_matrix_is_explicit_and_fail_closed() {
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        for (credential, crypto, validated, expected) in [
            (
                CredentialKind::Absent,
                CryptoReadiness::Anonymous,
                false,
                AccountReadiness::LoggedOut,
            ),
            (
                CredentialKind::Absent,
                CryptoReadiness::Ready(identity),
                false,
                AccountReadiness::CredentialUnavailable,
            ),
            (
                CredentialKind::Absent,
                CryptoReadiness::Unavailable,
                false,
                AccountReadiness::AccountBoundUnavailable,
            ),
            (
                CredentialKind::Pending,
                CryptoReadiness::Anonymous,
                false,
                AccountReadiness::CredentialUnavailable,
            ),
            (
                CredentialKind::Pending,
                CryptoReadiness::Ready(identity),
                false,
                AccountReadiness::CredentialUnavailable,
            ),
            (
                CredentialKind::Pending,
                CryptoReadiness::Unavailable,
                false,
                AccountReadiness::AccountBoundUnavailable,
            ),
            (
                CredentialKind::Active,
                CryptoReadiness::Anonymous,
                false,
                AccountReadiness::AccountBoundUnavailable,
            ),
            (
                CredentialKind::Active,
                CryptoReadiness::Ready(identity),
                false,
                AccountReadiness::AccountBoundUnavailable,
            ),
            (
                CredentialKind::Active,
                CryptoReadiness::Unavailable,
                false,
                AccountReadiness::AccountBoundUnavailable,
            ),
            (
                CredentialKind::Active,
                CryptoReadiness::Ready(identity),
                true,
                AccountReadiness::Ready,
            ),
        ] {
            assert_eq!(
                classify_account_readiness(credential, crypto, validated),
                expected
            );
        }
    }

    #[test]
    fn corrupt_session_credential_is_not_reported_as_logged_out() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x38; 32]);
        store_account_secret(temp.path(), AccountSecretKind::SessionTokens, b"not-json").unwrap();

        let sync_status = client.sync_status();
        assert!(
            matches!(sync_status, Err(ClientError::IncompleteAccountState)),
            "unexpected sync readiness result: {sync_status:?}"
        );
        let account = client.account_state().unwrap();
        assert!(!account.session_restored);
        assert!(account.loaded_credential_generation.is_none());
        assert!(account.session.is_none());
    }

    #[test]
    fn corrupt_remote_credential_does_not_block_account_bound_local_mutation() {
        const DB_KEY: [u8; 32] = [0x41; 32];
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &[0x42; KEY_LEN],
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x43; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            101,
        )
        .unwrap();
        let runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        client
            .runtime_epoch
            .store(runtime.runtime_epoch, std::sync::atomic::Ordering::Release);
        client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        store_account_secret(temp.path(), AccountSecretKind::SessionTokens, b"not-json").unwrap();

        let sync_status = client.sync_status();
        assert!(
            matches!(sync_status, Err(ClientError::IncompleteAccountState)),
            "unexpected sync readiness result: {sync_status:?}"
        );
        let task = client
            .create_task(super::super::CreateTaskCommand {
                list_id: list.id,
                title: "offline edit survives remote credential corruption".into(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
            })
            .unwrap();
        assert_eq!(
            task.content.title,
            "offline edit survives remote credential corruption"
        );
        assert_eq!(
            SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY)
                .list_outbox_heads(10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn active_credential_with_missing_identity_is_not_reported_as_logged_out() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x40; 32]);
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer: "https://sync.example.com".to_string(),
                access_token: "access-secret".to_string(),
                access_expires_at_ms: 1_900_000_000_000,
                refresh_token: "refresh-secret".to_string(),
                refresh_expires_at_ms: 1_901_000_000_000,
            },
        )
        .unwrap();

        assert!(matches!(
            client.sync_status(),
            Err(ClientError::IncompleteAccountState)
        ));
        let account = client.account_state().unwrap();
        assert!(!account.session_restored);
        assert!(account.loaded_credential_generation.is_none());
        assert!(account.session.is_none());
    }

    #[test]
    fn mismatched_session_and_crypto_identity_is_rejected_without_partial_publish() {
        const DB_KEY: [u8; 32] = [0x39; 32];
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &[0x3a; KEY_LEN],
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x3b; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        for (key, value) in [
            (
                ACCOUNT_EMAIL_METADATA_KEY,
                "mismatch@example.com".to_string(),
            ),
            (ACCOUNT_USER_ID_METADATA_KEY, identity.user_id.to_string()),
            (
                ACCOUNT_TENANT_ID_METADATA_KEY,
                identity.tenant_id.to_string(),
            ),
            (ACCOUNT_DEVICE_ID_METADATA_KEY, Uuid::now_v7().to_string()),
        ] {
            client.set_internal_metadata_value(key, &value).unwrap();
        }
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer: "https://sync.example.com".to_string(),
                access_token: "access-secret".to_string(),
                access_expires_at_ms: 1_900_000_000_000,
                refresh_token: "refresh-secret".to_string(),
                refresh_expires_at_ms: 1_901_000_000_000,
            },
        )
        .unwrap();

        assert!(matches!(
            client.resolve_account_readiness(),
            Err(ClientError::ProfileIdentityMismatch)
        ));
        let account = client.account_state().unwrap();
        assert!(!account.session_restored);
        assert!(account.loaded_credential_generation.is_none());
        assert!(account.session.is_none());
    }

    #[test]
    fn stale_runtime_epoch_is_rejected_before_sync_readiness_uses_cached_session() {
        const DB_KEY: [u8; 32] = [0x3c; 32];
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        {
            let mut account = client.account_state().unwrap();
            account.session = Some(account_session_state(
                "stale@example.com".to_string(),
                Uuid::now_v7().to_string(),
                Uuid::now_v7().to_string(),
                Uuid::now_v7().to_string(),
            ));
            account.session_restored = true;
            account.loaded_credential_generation = Some("stale".to_string());
        }
        SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .bump_runtime_epoch(1)
        .unwrap();

        assert!(matches!(
            client.sync_status(),
            Err(ClientError::LocalKeyState)
        ));
        let account = client.account_state().unwrap();
        assert!(account.session.is_none());
        assert!(!account.session_restored);
        assert!(account.loaded_credential_generation.is_none());
    }

    #[test]
    fn session_token_set_lock_excludes_another_profile_instance() {
        let temp = TempDir::new().expect("temp profile");
        let first = acquire_session_token_set_lock(temp.path()).expect("first session lock");
        assert!(matches!(
            acquire_session_token_set_lock(temp.path()),
            Err(ClientError::Busy)
        ));
        drop(first);
        acquire_session_token_set_lock(temp.path()).expect("session lock after release");
    }

    #[test]
    fn runtime_epoch_retry_replays_once_and_only_after_pre_write_mismatch() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x36; 32]);
        let mut attempts = 0;
        let mut committed_effects = 0;

        let result = client.retry_runtime_epoch_once(|| {
            attempts += 1;
            if attempts == 1 {
                return Err(ClientError::Storage(
                    StorageError::ProfileRuntimeEpochChanged {
                        expected: 1,
                        actual: 2,
                    },
                ));
            }
            committed_effects += 1;
            Ok("committed")
        });

        assert_eq!(result.unwrap(), "committed");
        assert_eq!(attempts, 2);
        assert_eq!(committed_effects, 1);

        let mut repeated_attempts = 0;
        let repeated = client.retry_runtime_epoch_once::<()>(|| {
            repeated_attempts += 1;
            Err(ClientError::Storage(
                StorageError::ProfileRuntimeEpochChanged {
                    expected: 1,
                    actual: 2,
                },
            ))
        });
        assert!(matches!(
            repeated,
            Err(ClientError::Storage(
                StorageError::ProfileRuntimeEpochChanged { .. }
            ))
        ));
        assert_eq!(repeated_attempts, 2);
    }

    #[test]
    fn runtime_epoch_publication_rejects_a_stale_cutover() {
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), [0x37; 32]);
        let original_epoch = client.loaded_runtime_epoch();
        let newer_epoch = original_epoch.checked_add(1).unwrap();
        client.publish_runtime_epoch(newer_epoch);

        assert!(matches!(
            client.publish_runtime_epoch_if_current(original_epoch, newer_epoch),
            Err(ClientError::LeaseLost)
        ));
        assert_eq!(client.loaded_runtime_epoch(), newer_epoch);
    }

    #[tokio::test]
    async fn two_client_instances_exclude_refresh_and_logout_mutations() {
        let temp = TempDir::new().expect("temp profile");
        let first = open_test_client(temp.path(), [0x34; 32]);
        let second = open_test_client(temp.path(), [0x34; 32]);
        let first_mutation =
            acquire_session_token_set_lock(&first.db_dir).expect("first client mutation lock");

        assert!(matches!(
            second.access_token(true).await,
            Err(ClientError::Busy)
        ));
        assert!(matches!(
            second.account_logout().await,
            Err(ClientError::Busy)
        ));
        drop(first_mutation);
        acquire_session_token_set_lock(&second.db_dir).expect("second client can continue");
    }

    #[tokio::test]
    async fn sync_token_refresh_checks_lease_immediately_before_http() {
        const DB_KEY: [u8; 32] = [0x3a; 32];
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve issuer address");
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        for (key, value) in [
            (ACCOUNT_EMAIL_METADATA_KEY, "lease@example.com".to_string()),
            (ACCOUNT_USER_ID_METADATA_KEY, identity.user_id.to_string()),
            (
                ACCOUNT_TENANT_ID_METADATA_KEY,
                identity.tenant_id.to_string(),
            ),
            (
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                identity.device_id.to_string(),
            ),
        ] {
            client.set_internal_metadata_value(key, &value).unwrap();
        }
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &[0x3b; KEY_LEN],
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x3c; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        let runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        client
            .runtime_epoch
            .store(runtime.runtime_epoch, std::sync::atomic::Ordering::Release);
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer,
                access_token: "expired-access".to_string(),
                access_expires_at_ms: 1,
                refresh_token: "refresh-secret".to_string(),
                refresh_expires_at_ms: i64::MAX,
            },
        )
        .unwrap();
        let mut gate = SqliteSyncStore::new_secret(client.db_path().to_path_buf(), client.db_key());
        gate.acquire_sync_lease("sync-token-refresh", client.loaded_runtime_epoch(), 60_000)
            .unwrap();
        open_encrypted(client.db_path(), &client.db_key())
            .unwrap()
            .execute(
                "UPDATE sync_run_lease SET expires_at_ms = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();

        assert!(matches!(
            client.access_token_for_sync(true, &mut gate).await,
            Err(ClientError::LeaseLost)
        ));
        delete_account_secret(temp.path(), AccountSecretKind::SessionTokens).unwrap();
    }

    #[tokio::test]
    async fn invalid_grant_deletes_the_credential_and_requires_reauthentication() {
        const DB_KEY: [u8; 32] = [0x3d; 32];
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind refresh server");
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept refresh request");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).expect("read refresh request");
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 2\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{{}}"
            )
            .expect("write invalid-grant response");
        });
        let temp = TempDir::new().expect("temp profile");
        let client = open_test_client(temp.path(), DB_KEY);
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        for (key, value) in [
            (ACCOUNT_EMAIL_METADATA_KEY, "reauth@example.com".to_string()),
            (ACCOUNT_USER_ID_METADATA_KEY, identity.user_id.to_string()),
            (
                ACCOUNT_TENANT_ID_METADATA_KEY,
                identity.tenant_id.to_string(),
            ),
            (
                ACCOUNT_DEVICE_ID_METADATA_KEY,
                identity.device_id.to_string(),
            ),
        ] {
            client.set_internal_metadata_value(key, &value).unwrap();
        }
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &[0x3e; KEY_LEN],
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x3f; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        client.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        store_session_tokens(
            temp.path(),
            &StoredSessionTokens {
                version: SESSION_TOKEN_SET_VERSION,
                credential_generation: Some(Uuid::now_v7().to_string()),
                issuer,
                access_token: "expired-access".to_string(),
                access_expires_at_ms: 1,
                refresh_token: "invalid-refresh".to_string(),
                refresh_expires_at_ms: i64::MAX,
            },
        )
        .unwrap();

        assert!(matches!(
            client.access_token(true).await,
            Err(ClientError::CredentialUnavailable)
        ));
        assert!(load_session_credential(temp.path()).unwrap().is_none());
        server.join().expect("refresh server");
    }

    #[test]
    fn key_cutover_backfills_mutation_committed_after_remote_fetch() {
        const DB_KEY: [u8; 32] = [0x3b; 32];
        const MASTER_KEY: [u8; KEY_LEN] = [0x41; KEY_LEN];
        let temp = TempDir::new().unwrap();
        let client = open_test_client(temp.path(), DB_KEY);
        let tenant_id = Uuid::now_v7();
        let identity = LocalCryptoIdentity {
            tenant_id,
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let generation_one = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new([0x42; KEY_LEN])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        };
        let crypto = persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            identity,
            &MASTER_KEY,
            generation_one.clone(),
            100,
        )
        .unwrap();
        let runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        client.publish_runtime_epoch(runtime.runtime_epoch);
        {
            let mut account = client.account_state().unwrap();
            account.session_restored = true;
            account.crypto = CryptoRuntimeState::Ready(Box::new(crypto));
        }

        // This models a verified remote response already fetched at the
        // network barrier. The local mutation deliberately commits after that
        // fetch and before the fenced cutover transaction starts.
        let generation_two = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new([0x43; KEY_LEN])),
            tenant_generation: 2,
            historical_tenant_root_deks: vec![(1, generation_one.tenant_root_dek.clone().unwrap())],
        };
        let list = client
            .create_list("Committed after key fetch".to_string())
            .unwrap();
        let before = SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY)
            .list_all_outbox_heads(10)
            .unwrap();
        let old_head = before
            .iter()
            .find(|entry| entry.record_id == list.id)
            .unwrap();
        let old_op_id = old_head.op_id;
        let EncryptedSyncState::Live { blob, .. } = &old_head.state else {
            panic!("created list must have a live outbox head");
        };
        assert_eq!(
            taskveil_sync::parse_envelope_header(blob)
                .unwrap()
                .key_generation,
            1
        );

        let mut lease_store =
            SqliteSyncStore::new_secret(client.db_path().to_path_buf(), client.db_key());
        lease_store
            .acquire_sync_lease("key-cutover", runtime.runtime_epoch, 60_000)
            .unwrap();
        let lease = lease_store.active_lease().unwrap();
        client
            .commit_tenant_key_cutover(lease, identity, &MASTER_KEY, generation_two)
            .unwrap();

        let after = SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY)
            .list_all_outbox_heads(10)
            .unwrap();
        let rotated = after
            .iter()
            .find(|entry| entry.record_id == list.id)
            .expect("post-fetch mutation must survive key cutover");
        assert_ne!(rotated.op_id, old_op_id);
        let EncryptedSyncState::Live { blob, .. } = &rotated.state else {
            panic!("rotation backfill must keep a live head");
        };
        assert_eq!(
            taskveil_sync::parse_envelope_header(blob)
                .unwrap()
                .key_generation,
            2
        );
        let durable_runtime = SqliteProfileCoordinationRepository::new(
            open_encrypted(client.db_path(), &DB_KEY).unwrap(),
        )
        .load_runtime()
        .unwrap();
        assert_eq!(client.loaded_runtime_epoch(), durable_runtime.runtime_epoch);
        assert!(matches!(
            lease_store.preflight_network_request(),
            Err(error) if error == "sync lease lost"
        ));
    }

    #[test]
    fn billing_cache_round_trips_through_encrypted_profile_and_rejects_corruption() {
        let temp = TempDir::new().expect("temp profile");
        let db_key = [0x42; 32];
        let expected = BillingState {
            provider: "revenuecat".to_string(),
            provider_app_user_id: "00000000-0000-4000-8000-000000000001".to_string(),
            lookup_key: "pro".to_string(),
            status: "in_grace_period".to_string(),
            sync_allowed: true,
            store_product_identifier: Some("com.taskveil.app.pro.monthly".to_string()),
            expires_at: Some(1_800_000_000_000),
            grace_expires_at: Some(1_801_382_400_000),
            will_renew: Some(false),
            environment: "sandbox".to_string(),
            updated_at: Some(1_799_999_999_000),
        };

        let client = open_test_client(temp.path(), db_key);
        client
            .set_internal_metadata_value(
                BILLING_ENTITLEMENT_CACHE_METADATA_KEY,
                &serde_json::to_string(&expected).expect("serialize billing state"),
            )
            .expect("persist billing cache");
        assert_eq!(
            client.cached_billing().expect("read cache"),
            Some(expected.clone())
        );
        drop(client);

        let reopened = open_test_client(temp.path(), db_key);
        assert_eq!(
            reopened.cached_billing().expect("read reopened cache"),
            Some(expected)
        );
        reopened
            .set_internal_metadata_value(BILLING_ENTITLEMENT_CACHE_METADATA_KEY, "{not-json")
            .expect("persist corrupt cache fixture");
        assert!(matches!(
            reopened.cached_billing(),
            Err(ClientError::AccountRequest)
        ));
    }

    #[test]
    fn organization_trust_pin_is_strict_and_root_changes_require_reconfirmation() {
        let owner = Uuid::now_v7();
        let member = Uuid::now_v7();
        let response = taskveil_sync::organization::OrganizationSafetyResponse {
            owner_user_id: owner,
            member_user_id: member,
            owner_root_public: "owner-root".to_string(),
            member_root_public: "member-root".to_string(),
            digest: "digest".to_string(),
            decimal: "decimal".to_string(),
            qr_payload: "qr".to_string(),
            verification_state: "verified".to_string(),
            owner_confirmed: true,
            member_confirmed: true,
        };
        let mut pin = OrganizationTrustPin::candidate(&response);
        pin.locally_confirmed = true;
        pin.minimum_generation = 7;
        pin.owner_roster_revision = 3;
        pin.member_roster_revision = 4;
        assert_eq!(
            OrganizationTrustPin::decode(&pin.encode()),
            Some(pin.clone())
        );
        assert!(pin.matches(&response));

        let mut substituted = response;
        substituted.member_root_public = "server-substituted-root".to_string();
        assert!(!pin.matches(&substituted));
        assert!(OrganizationTrustPin::decode("partial|pin").is_none());
        assert!(OrganizationTrustPin::decode("a|b|c|1|0|0|0").is_none());
    }

    #[test]
    fn authentication_completion_never_deletes_initial_backfill_cursor() {
        const DB_KEY: [u8; 32] = [0x71; 32];
        const MASTER_KEY: [u8; 32] = [0x72; 32];
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cursor.sqlite3");
        let identity = LocalCryptoIdentity {
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
        };
        let crypto = persist_local_crypto_context(
            &db_path,
            &DB_KEY,
            identity,
            &MASTER_KEY,
            LocalSyncKeys {
                tenant_id: identity.tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x73; 32])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            1,
        )
        .unwrap();
        let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
        store
            .set_cursor(super::super::INITIAL_BACKFILL_CURSOR_NAME, 1, 10)
            .unwrap();

        let client = TaskveilClient {
            db_dir: temp.path().to_path_buf(),
            profile_coordinator: TaskveilClient::pinned_test_coordinator(temp.path(), &db_path),
            db_path,
            db_key: Mutex::new(Zeroizing::new(DB_KEY)),
            account: std::sync::Mutex::new(super::super::AccountRuntimeState {
                session: None,
                session_restored: false,
                loaded_credential_generation: None,
                crypto: CryptoRuntimeState::Unloaded,
            }),
            sync: std::sync::Mutex::new(super::super::SyncRuntimeState::default()),
            runtime_epoch: std::sync::atomic::AtomicI64::new(1),
            capsule_generation: std::sync::atomic::AtomicU64::new(1),
        };
        client
            .replace_account_runtime(
                Some(account_session_state(
                    "user@example.com".into(),
                    identity.user_id.to_string(),
                    identity.tenant_id.to_string(),
                    identity.device_id.to_string(),
                )),
                crypto,
            )
            .unwrap();

        // Runtime replacement is the final login/register boundary. It must
        // not reset durable backfill progress for a same-profile relogin.
        assert_eq!(
            store
                .get_cursor_seq(super::super::INITIAL_BACKFILL_CURSOR_NAME)
                .unwrap(),
            Some(1)
        );
    }

    #[test]
    fn same_profile_authentication_rebinds_clock_and_pending_outbox_to_fresh_device() {
        const DB_KEY: [u8; 32] = [0x74; 32];
        let temp = TempDir::new().unwrap();
        let client = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 1).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();

        let old_device = Uuid::now_v7();
        let new_device = Uuid::now_v7();
        let old_clock = Hlc {
            wall_ms: 100,
            counter: 5,
            device_id: old_device.to_string(),
        };
        let old_revision = old_clock.encode().unwrap();
        let old_op_id = Uuid::now_v7();
        let mut store = SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY);
        store
            .set_setting(SYNC_LOCAL_HLC_METADATA_KEY, &old_revision, 100)
            .unwrap();
        store
            .set_setting(ACCOUNT_DEVICE_ID_METADATA_KEY, &old_device.to_string(), 100)
            .unwrap();
        store
            .put_outbox_head(NewLocalSyncOutboxEntry {
                op_id: old_op_id,
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: old_revision.clone(),
                state: EncryptedSyncState::Live {
                    mutation_hlc: old_revision.clone(),
                    blob: vec![1, 2, 3],
                },
                created_at: 100,
            })
            .unwrap();

        client.rebind_sync_device_locked(new_device, 200).unwrap();

        let rebound_clock = Hlc::decode(
            &store
                .get_setting(SYNC_LOCAL_HLC_METADATA_KEY)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let rebound = store.list_all_outbox_heads(10).unwrap();
        assert_eq!(rebound.len(), 1);
        let rebound_revision = Hlc::decode(&rebound[0].revision_hlc).unwrap();
        assert_eq!(rebound_clock.device_id, new_device.to_string());
        assert_eq!(rebound_revision.device_id, new_device.to_string());
        assert!(rebound_revision.encode().unwrap() > old_revision);
        assert_ne!(rebound[0].op_id, old_op_id);
        assert_eq!(
            rebound[0].state,
            EncryptedSyncState::Live {
                mutation_hlc: old_clock.encode().unwrap(),
                blob: vec![1, 2, 3],
            }
        );
        assert_eq!(
            store
                .get_setting(ACCOUNT_DEVICE_ID_METADATA_KEY)
                .unwrap()
                .as_deref(),
            Some(new_device.to_string().as_str())
        );
    }

    #[test]
    fn device_rebind_failure_rolls_back_clock_outbox_and_account_device() {
        const DB_KEY: [u8; 32] = [0x75; 32];
        let temp = TempDir::new().unwrap();
        let client = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 1).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();

        let old_device = Uuid::now_v7();
        let old_clock = Hlc {
            wall_ms: 100,
            counter: 5,
            device_id: old_device.to_string(),
        }
        .encode()
        .unwrap();
        let old_op_id = Uuid::now_v7();
        let mut store = SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY);
        store
            .set_setting(SYNC_LOCAL_HLC_METADATA_KEY, &old_clock, 100)
            .unwrap();
        store
            .set_setting(ACCOUNT_DEVICE_ID_METADATA_KEY, &old_device.to_string(), 100)
            .unwrap();
        store
            .put_outbox_head(NewLocalSyncOutboxEntry {
                op_id: old_op_id,
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: old_clock.clone(),
                state: EncryptedSyncState::Tombstone {
                    delete_hlc: old_clock.clone(),
                },
                created_at: 100,
            })
            .unwrap();
        open_encrypted(client.db_path(), &DB_KEY)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_device_rebind BEFORE UPDATE ON sync_outbox
                 BEGIN SELECT RAISE(ABORT, 'fail device rebind'); END;",
            )
            .unwrap();

        assert!(client
            .rebind_sync_device_locked(Uuid::now_v7(), 200)
            .is_err());

        assert_eq!(
            store.get_setting(SYNC_LOCAL_HLC_METADATA_KEY).unwrap(),
            Some(old_clock.clone())
        );
        assert_eq!(
            store
                .get_setting(ACCOUNT_DEVICE_ID_METADATA_KEY)
                .unwrap()
                .as_deref(),
            Some(old_device.to_string().as_str())
        );
        let pending = store.list_all_outbox_heads(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].op_id, old_op_id);
        assert_eq!(pending[0].revision_hlc, old_clock);
    }

    #[test]
    fn account_persistence_failure_retries_same_device_without_reclocking_outbox_again() {
        // Apple unit-test binaries are not signed with the production
        // Keychain entitlement. Select the same file-backed secret path used
        // by the Flutter test runner.
        std::env::set_var("FLUTTER_TEST", "1");
        let temp = TempDir::new().unwrap();
        let client =
            TaskveilClient::open(super::super::LocalProfileConfig::new(temp.path(), "Inbox"))
                .unwrap();
        let old_device = Uuid::now_v7();
        let new_device = Uuid::now_v7();
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let list = new_list("Pending".into(), "a1".into(), 1).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &client.db_key()).unwrap())
            .insert(list.clone())
            .unwrap();
        let old_revision = Hlc {
            wall_ms: 100,
            counter: 5,
            device_id: old_device.to_string(),
        }
        .encode()
        .unwrap();
        let mut store = SqliteSyncStore::new(client.db_path().to_path_buf(), *client.db_key());
        store
            .set_setting(SYNC_LOCAL_HLC_METADATA_KEY, &old_revision, 100)
            .unwrap();
        store
            .put_outbox_head(NewLocalSyncOutboxEntry {
                op_id: Uuid::now_v7(),
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: old_revision.clone(),
                state: EncryptedSyncState::Tombstone {
                    delete_hlc: old_revision,
                },
                created_at: 100,
            })
            .unwrap();

        let root = taskveil_crypto::organization::generate_account_root(user_id).unwrap();
        let keys = AccountKeyMaterial {
            generation: 1,
            tenant_generation: 1,
            master_key: Zeroizing::new([0x31; KEY_LEN]),
            account_root_private: root.private,
            account_root_public: root.public,
            tenant_root_dek: Zeroizing::new([0x32; KEY_LEN]),
        };
        let session = account_session_state(
            "retry@example.com".into(),
            user_id.to_string(),
            tenant_id.to_string(),
            new_device.to_string(),
        );
        let tokens = AccountTokenSet {
            access_token: Zeroizing::new("new-access".to_string()),
            access_expires_at_ms: 2_000_000_000_000,
            refresh_token: Zeroizing::new("new-refresh".to_string()),
            refresh_expires_at_ms: 2_100_000_000_000,
        };
        open_encrypted(client.db_path(), &client.db_key())
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_account_persistence
                 BEFORE INSERT ON internal_metadata
                 WHEN NEW.key = 'account_email'
                 BEGIN SELECT RAISE(ABORT, 'fail account persistence'); END;",
            )
            .unwrap();

        assert!(client
            .persist_account_state_locked(
                "https://sync.example.com",
                &session,
                &tokens,
                &[0x44; 48],
                &keys,
            )
            .is_err());
        assert!(load_session_tokens(temp.path()).unwrap().is_none());
        let after_failure = store.list_all_outbox_heads(10).unwrap();
        assert_eq!(after_failure.len(), 1);
        let rebound_op_id = after_failure[0].op_id;
        assert_eq!(
            Hlc::decode(&after_failure[0].revision_hlc)
                .unwrap()
                .device_id,
            new_device.to_string()
        );

        open_encrypted(client.db_path(), &client.db_key())
            .unwrap()
            .execute_batch("DROP TRIGGER fail_account_persistence;")
            .unwrap();
        client
            .persist_account_state_locked(
                "https://sync.example.com",
                &session,
                &tokens,
                &[0x44; 48],
                &keys,
            )
            .unwrap();

        assert!(load_session_tokens(temp.path()).unwrap().is_some());
        let after_retry = store.list_all_outbox_heads(10).unwrap();
        assert_eq!(after_retry.len(), 1);
        assert_eq!(after_retry[0].op_id, rebound_op_id);
        assert_eq!(
            Hlc::decode(&after_retry[0].revision_hlc).unwrap().device_id,
            new_device.to_string()
        );
    }

    #[test]
    fn stale_anonymous_instance_fails_closed_after_another_instance_binds_profile() {
        const DB_KEY: [u8; 32] = [0x76; 32];
        let temp = TempDir::new().unwrap();
        let first = open_test_client(temp.path(), DB_KEY);
        let second = open_test_client(temp.path(), DB_KEY);
        assert!(matches!(
            first.local_mutation_state().unwrap(),
            super::super::LocalMutationState::Anonymous
        ));
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(second.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        let master_key = [0x51; KEY_LEN];
        persist_local_crypto_context(
            second.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id,
            },
            &master_key,
            LocalSyncKeys {
                tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x52; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            101,
        )
        .unwrap();

        let result = first.create_task(super::super::CreateTaskCommand {
            list_id: list.id,
            title: "must not commit anonymously".into(),
            parent_task_id: None,
            due: None,
            note: None,
            priority: 0,
            scheduled_at: None,
            estimated_minutes: None,
        });
        assert!(matches!(
            result,
            Err(ClientError::LocalKeyState | ClientError::AccountBoundUnavailable)
        ));
        assert!(
            SqliteTaskRepository::new(open_encrypted(second.db_path(), &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .is_empty()
        );
        assert!(SqliteSyncStore::new(second.db_path().to_path_buf(), DB_KEY)
            .list_outbox_heads(1)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn real_child_stale_anonymous_fails_closed_after_profile_binding() {
        const DB_KEY: [u8; 32] = [0xb6; 32];
        const MASTER_KEY: [u8; KEY_LEN] = [0x71; KEY_LEN];
        let temp = TempDir::new().unwrap();
        let client = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let (mut child, _output) = spawn_stale_runtime_child(temp.path(), "anonymous", list.id);

        let tenant_id = Uuid::now_v7();
        let _exclusive = ProfileCoordinator::for_profile(temp.path())
            .unwrap()
            .try_exclusive()
            .unwrap();
        persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
            },
            &MASTER_KEY,
            LocalSyncKeys {
                tenant_id,
                tenant_root_dek: Some(Zeroizing::new([0x72; KEY_LEN])),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            101,
        )
        .unwrap();
        drop(_exclusive);

        child.stdin.take().unwrap().write_all(b"mutate\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(
            SqliteTaskRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .is_empty()
        );
        assert!(SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY)
            .list_outbox_heads(1)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stale_ready_instance_fails_closed_after_device_identity_rotation() {
        const DB_KEY: [u8; 32] = [0x77; 32];
        let temp = TempDir::new().unwrap();
        let first = open_test_client(temp.path(), DB_KEY);
        let second = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(first.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let master_key = [0x61; KEY_LEN];
        let sync_keys = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new([0x62; KEY_LEN])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        };
        let ready = persist_local_crypto_context(
            first.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id: Uuid::now_v7(),
            },
            &master_key,
            sync_keys.clone(),
            101,
        )
        .unwrap();
        first
            .runtime_epoch
            .store(2, std::sync::atomic::Ordering::Release);
        first.account_state().unwrap().crypto = CryptoRuntimeState::Ready(Box::new(ready));
        assert!(matches!(
            first.local_mutation_state().unwrap(),
            super::super::LocalMutationState::Ready(_)
        ));

        persist_local_crypto_context(
            second.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id: Uuid::now_v7(),
            },
            &master_key,
            sync_keys,
            102,
        )
        .unwrap();

        let result = first.create_task(super::super::CreateTaskCommand {
            list_id: list.id,
            title: "must not use stale device identity".into(),
            parent_task_id: None,
            due: None,
            note: None,
            priority: 0,
            scheduled_at: None,
            estimated_minutes: None,
        });
        assert!(matches!(
            result,
            Err(ClientError::LocalKeyState | ClientError::AccountBoundUnavailable)
        ));
        assert!(
            SqliteTaskRepository::new(open_encrypted(second.db_path(), &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn real_child_stale_ready_fails_closed_after_device_identity_rotation() {
        const DB_KEY: [u8; 32] = [0xb6; 32];
        const MASTER_KEY: [u8; KEY_LEN] = [0x71; KEY_LEN];
        let temp = TempDir::new().unwrap();
        let client = open_test_client(temp.path(), DB_KEY);
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let sync_keys = LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some(Zeroizing::new([0x72; KEY_LEN])),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        };
        persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id: Uuid::now_v7(),
            },
            &MASTER_KEY,
            sync_keys.clone(),
            101,
        )
        .unwrap();
        let (mut child, _output) = spawn_stale_runtime_child(temp.path(), "ready", list.id);

        let _exclusive = ProfileCoordinator::for_profile(temp.path())
            .unwrap()
            .try_exclusive()
            .unwrap();
        persist_local_crypto_context(
            client.db_path(),
            &DB_KEY,
            LocalCryptoIdentity {
                tenant_id,
                user_id,
                device_id: Uuid::now_v7(),
            },
            &MASTER_KEY,
            sync_keys,
            102,
        )
        .unwrap();
        drop(_exclusive);

        child.stdin.take().unwrap().write_all(b"mutate\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(
            SqliteTaskRepository::new(open_encrypted(client.db_path(), &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .is_empty()
        );
        assert!(SqliteSyncStore::new(client.db_path().to_path_buf(), DB_KEY)
            .list_outbox_heads(1)
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn post_open_profile_root_swap_is_rejected_by_the_pinned_child() {
        const DB_KEY: [u8; 32] = [0xb6; 32];
        let workspace = TempDir::new().unwrap();
        let profile = workspace.path().join("profile");
        let original = workspace.path().join("original-profile");
        std::fs::create_dir(&profile).unwrap();
        let db_path = profile.join("taskveil.db");
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let (mut child, _output) = spawn_stale_runtime_child(&profile, "root-swap", list.id);

        std::fs::rename(&profile, &original).unwrap();
        std::fs::create_dir(&profile).unwrap();
        let replacement_db_path = profile.join("taskveil.db");
        SqliteListRepository::new(open_encrypted(&replacement_db_path, &DB_KEY).unwrap())
            .insert(list)
            .unwrap();

        child.stdin.take().unwrap().write_all(b"mutate\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(
            SqliteTaskRepository::new(open_encrypted(&replacement_db_path, &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .is_empty()
        );
        assert!(SqliteSyncStore::new(replacement_db_path, DB_KEY)
            .list_outbox_heads(1)
            .unwrap()
            .is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn post_open_profile_root_handle_denies_rename_until_child_exit() {
        const DB_KEY: [u8; 32] = [0xb6; 32];
        let workspace = TempDir::new().unwrap();
        let profile = workspace.path().join("profile");
        let moved = workspace.path().join("moved-profile");
        std::fs::create_dir(&profile).unwrap();
        let db_path = profile.join("taskveil.db");
        let list = new_list("Inbox".into(), "a0".into(), 100).unwrap();
        SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .insert(list.clone())
            .unwrap();
        let (mut child, _output) =
            spawn_stale_runtime_child(&profile, "windows-root-handle", list.id);

        assert!(
            std::fs::rename(&profile, &moved).is_err(),
            "the retained root handle must deny delete/rename sharing"
        );
        child.stdin.take().unwrap().write_all(b"mutate\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            SqliteTaskRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
                .list_all_for_sync()
                .unwrap()
                .len(),
            1
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match std::fs::rename(&profile, &moved) {
                Ok(()) => break,
                Err(error) if std::time::Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("profile rename did not recover after child exit: {error}"),
            }
        }
    }
}
