//! Account registration/login client and key bundle DTOs.

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use opaque_ke::{ClientLogin, ClientRegistration, CredentialResponse, RegistrationResponse};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use taskveil_crypto::{
    key_hierarchy::{
        derive_kek_pw, derive_recovery_wrap_key, generate_master_key, generate_recovery_key,
        generate_tenant_root_dek, unwrap_account_root_private_key_with_master_key,
        unwrap_master_key_with_kek_pw, unwrap_tenant_root_dek_with_master_key,
        wrap_account_root_private_key_with_master_key, wrap_master_key_with_device_key,
        wrap_master_key_with_kek_pw, wrap_master_key_with_recovery_key,
        wrap_tenant_root_dek_with_master_key, KeyHierarchyError, INITIAL_KEY_GENERATION, KEY_LEN,
    },
    opaque_login_parameters, opaque_registration_parameters,
    organization::{
        create_device_proof, derive_safety_number, generate_account_root, generate_device_keys,
        issue_device_certificate, verify_device_certificate, AccountRootPrivateKeys,
        AccountRootPublicKeys, DeviceCertificate, DeviceIdentity, DeviceProofOfPossession,
        HybridDekPackage, OrganizationCryptoError, SignedDeviceRevocation, DEVICE_CHALLENGE_LEN,
    },
    TaskveilCipherSuite, CRYPTO_SUITE_ID,
};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{KeyManifest, KeyManifestError, RotationStatus};
pub use taskveil_protocol::account::{
    AccountKeyBundleDto, ActiveKeyBundleDto, BillingEntitlementDto, BillingResponseDto,
    DeviceEnrollmentDto, HistoricalKeyBundleDto, UpdateKeyWrappersRequest,
};

#[derive(Debug, Error)]
pub enum AccountClientError {
    #[error("server URL is empty")]
    EmptyServerUrl,
    #[error("server URL is not a secure origin")]
    InvalidServerOrigin,
    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("server returned account error with HTTP status {0}")]
    Server(u16),
    #[error("remote session is no longer refreshable")]
    InvalidGrant,
    #[error("a Pro entitlement is required")]
    EntitlementRequired,
    #[error("invalid base64 field")]
    Base64,
    #[error("OPAQUE protocol error")]
    Opaque,
    #[error("key hierarchy error")]
    KeyHierarchy(#[from] KeyHierarchyError),
    #[error("key manifest error")]
    KeyManifest(#[from] KeyManifestError),
    #[error("organization cryptography error")]
    OrganizationCrypto(#[from] OrganizationCryptoError),
    #[error("list key bundle conflicts with the immutable server value")]
    KeyBundleConflict,
    #[error("organization public-key verification failed")]
    OrganizationVerification,
    #[error("email verification expired")]
    EmailVerificationExpired,
    #[error("email verification resend is available at {0} milliseconds since Unix epoch")]
    EmailVerificationRetryAt(i64),
}

pub struct AccountClient {
    base_url: String,
    http: reqwest::Client,
}

pub struct AccountRegisterOutcome {
    pub session: AccountSession,
    pub recovery_key: Zeroizing<String>,
    pub local_wrapped_master_key: Vec<u8>,
    pub keys: AccountKeyMaterial,
    pub device_identity: DeviceIdentity,
}

pub enum AccountRegistrationReconcile {
    Pending,
    Committed(Box<AccountRegisterOutcome>),
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountRegistrationRequestPrepared {
    version: u8,
    origin: String,
    email: String,
    request_body: Vec<u8>,
    request_idempotency_key: String,
    handoff_secret: String,
    expires_at_ms: i64,
}

impl AccountRegistrationRequestPrepared {
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, AccountClientError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| AccountClientError::Opaque)
    }

    pub fn decode(value: &[u8]) -> Result<Self, AccountClientError> {
        let prepared: Self =
            serde_json::from_slice(value).map_err(|_| AccountClientError::Opaque)?;
        if prepared.version != 1
            || prepared.origin.is_empty()
            || prepared.email.is_empty()
            || prepared.request_body.is_empty()
            || prepared.request_idempotency_key.is_empty()
            || prepared.handoff_secret.is_empty()
            || prepared.expires_at_ms <= 0
        {
            return Err(AccountClientError::Opaque);
        }
        Ok(prepared)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountRegistrationMailbox {
    version: u8,
    origin: String,
    email: String,
    request_id: String,
    handoff_secret: String,
    resend_idempotency_key: String,
    verify_idempotency_key: String,
    #[serde(default)]
    verify_request_binding: Option<String>,
    expires_at_ms: i64,
    next_retry_at_ms: i64,
}

impl AccountRegistrationMailbox {
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, AccountClientError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| AccountClientError::Opaque)
    }

    pub fn decode(value: &[u8]) -> Result<Self, AccountClientError> {
        let mailbox: Self =
            serde_json::from_slice(value).map_err(|_| AccountClientError::Opaque)?;
        if mailbox.version != 1
            || mailbox.origin.is_empty()
            || mailbox.email.is_empty()
            || Uuid::parse_str(&mailbox.request_id).is_err()
            || mailbox.handoff_secret.is_empty()
            || mailbox.resend_idempotency_key.is_empty()
            || mailbox.verify_idempotency_key.is_empty()
            || mailbox.expires_at_ms <= 0
            || mailbox.next_retry_at_ms <= 0
        {
            return Err(AccountClientError::Opaque);
        }
        Ok(mailbox)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub fn prepare_otp_attempt(&mut self, otp: &str) -> Result<(), AccountClientError> {
        if otp.len() != 8 || !otp.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let handoff_secret = URL_SAFE_NO_PAD
            .decode(&self.handoff_secret)
            .map_err(|_| AccountClientError::Base64)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&handoff_secret)
            .map_err(|_| AccountClientError::Opaque)?;
        mac.update(b"taskveil/email/registration-verify-binding/v1\0");
        mac.update(otp.as_bytes());
        let binding = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        if self.verify_request_binding.as_deref() != Some(binding.as_str()) {
            self.verify_idempotency_key = Uuid::now_v7().to_string();
            self.verify_request_binding = Some(binding);
        }
        Ok(())
    }

    fn apply_resend_response(&mut self, response: &RegistrationRequestResponse) {
        self.expires_at_ms = response.expires_at.timestamp_millis();
        self.next_retry_at_ms = response.next_retry_at.timestamp_millis();
        self.resend_idempotency_key = Uuid::now_v7().to_string();
        // A resend creates a new OTP generation. Its verify idempotency
        // namespace must not inherit a rejected outcome from the prior
        // generation, even if the random OTP value happens to repeat.
        self.verify_idempotency_key = Uuid::now_v7().to_string();
        self.verify_request_binding = None;
    }

    pub fn next_retry_at_ms(&self) -> i64 {
        self.next_retry_at_ms
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountRegistrationVerified {
    version: u8,
    origin: String,
    email: String,
    request_id: String,
    handoff_secret: String,
    registration_ticket: String,
    expires_at_ms: i64,
}

impl AccountRegistrationVerified {
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, AccountClientError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| AccountClientError::Opaque)
    }

    pub fn decode(value: &[u8]) -> Result<Self, AccountClientError> {
        let verified: Self =
            serde_json::from_slice(value).map_err(|_| AccountClientError::Opaque)?;
        if verified.version != 1
            || verified.origin.is_empty()
            || verified.email.is_empty()
            || Uuid::parse_str(&verified.request_id).is_err()
            || verified.handoff_secret.is_empty()
            || verified.registration_ticket.is_empty()
            || verified.expires_at_ms <= 0
        {
            return Err(AccountClientError::Opaque);
        }
        Ok(verified)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountRegistrationStartPrepared {
    version: u8,
    origin: String,
    email: String,
    request_id: String,
    handoff_secret: String,
    start_body: Vec<u8>,
    start_idempotency_key: String,
    client_registration_state: Vec<u8>,
    expires_at_ms: i64,
}

impl AccountRegistrationStartPrepared {
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, AccountClientError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| AccountClientError::Opaque)
    }

    pub fn decode(value: &[u8]) -> Result<Self, AccountClientError> {
        let prepared: Self =
            serde_json::from_slice(value).map_err(|_| AccountClientError::Opaque)?;
        if prepared.version != 1
            || prepared.origin.is_empty()
            || prepared.email.is_empty()
            || Uuid::parse_str(&prepared.request_id).is_err()
            || prepared.handoff_secret.is_empty()
            || prepared.start_body.is_empty()
            || prepared.start_idempotency_key.is_empty()
            || prepared.client_registration_state.is_empty()
            || prepared.expires_at_ms <= 0
        {
            return Err(AccountClientError::Opaque);
        }
        Ok(prepared)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct AccountRegistrationPrepared {
    version: u8,
    origin: String,
    email: String,
    request_id: String,
    handoff_secret: String,
    finish_body: Vec<u8>,
    start_idempotency_key: String,
    finish_idempotency_key: String,
    expires_at_ms: i64,
    recovery_key: String,
    local_wrapped_master_key: Vec<u8>,
    generation: u64,
    tenant_generation: u64,
    master_key: Vec<u8>,
    account_root_private: Vec<u8>,
    account_root_public: Vec<u8>,
    tenant_root_dek: Vec<u8>,
    device_identity: Vec<u8>,
}

impl AccountRegistrationPrepared {
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, AccountClientError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| AccountClientError::Opaque)
    }

    pub fn decode(value: &[u8]) -> Result<Self, AccountClientError> {
        let prepared: Self =
            serde_json::from_slice(value).map_err(|_| AccountClientError::Opaque)?;
        if prepared.version != 1
            || prepared.origin.is_empty()
            || prepared.email.is_empty()
            || Uuid::parse_str(&prepared.request_id).is_err()
            || prepared.handoff_secret.is_empty()
            || prepared.finish_body.is_empty()
            || prepared.start_idempotency_key.is_empty()
            || prepared.finish_idempotency_key.is_empty()
            || prepared.expires_at_ms <= 0
            || prepared.master_key.len() != KEY_LEN
            || prepared.tenant_root_dek.len() != KEY_LEN
        {
            return Err(AccountClientError::Opaque);
        }
        Ok(prepared)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

pub struct AccountLoginProvisional {
    pub session: AccountSession,
    pub local_wrapped_master_key: Vec<u8>,
    pub keys: AccountKeyMaterial,
    pub device_identity: DeviceIdentity,
    pub enrollment: DeviceEnrollmentDto,
    pub challenge_expires_at_ms: i64,
}

pub struct AccountSession {
    pub user_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub email: String,
    pub tokens: AccountTokenSet,
}

pub struct AccountTokenSet {
    pub access_token: Zeroizing<String>,
    pub access_expires_at_ms: i64,
    pub refresh_token: Zeroizing<String>,
    pub refresh_expires_at_ms: i64,
}

const NATIVE_CLIENT_ID: &str = "taskveil-native";

pub struct AccountKeyMaterial {
    pub generation: u64,
    pub tenant_generation: u64,
    pub master_key: Zeroizing<[u8; KEY_LEN]>,
    pub account_root_private: AccountRootPrivateKeys,
    pub account_root_public: AccountRootPublicKeys,
    pub tenant_root_dek: Zeroizing<[u8; KEY_LEN]>,
}

pub struct OrganizationDekDelivery<'a> {
    pub sender_identity: &'a DeviceIdentity,
    pub sender_root: &'a AccountRootPublicKeys,
    pub recipient: &'a crate::organization::OrganizationDeviceDto,
    pub expected_recipient_root: &'a AccountRootPublicKeys,
    pub scope_kind: taskveil_crypto::organization::HybridScopeKind,
    pub scope_id: Uuid,
    pub generation: u64,
    pub dek: &'a [u8; KEY_LEN],
    pub now_ms: i64,
}

pub struct VerifiedOrganizationDeviceRoster {
    pub revision: u64,
    pub head_hash: [u8; 32],
    pub devices: Vec<crate::organization::OrganizationDeviceDto>,
}

#[derive(Clone, Copy)]
pub struct OrganizationRosterTrust<'a> {
    pub user_id: Uuid,
    pub root_public: &'a str,
    pub minimum_revision: u64,
    pub minimum_head_hash: [u8; 32],
}

pub fn wrap_organization_dek_for_verified_device(
    delivery: OrganizationDekDelivery<'_>,
) -> Result<HybridDekPackage, AccountClientError> {
    let OrganizationDekDelivery {
        sender_identity,
        sender_root,
        recipient,
        expected_recipient_root,
        scope_kind,
        scope_id,
        generation,
        dek,
        now_ms,
    } = delivery;
    if recipient.revoked
        || recipient.user_id != expected_recipient_root.user_id
        || decode_base64(&recipient.account_root_public)? != expected_recipient_root.encode()?
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    let recipient_certificate = DeviceCertificate::decode(&decode_base64(&recipient.certificate)?)?;
    if recipient_certificate.device_id != recipient.device_id
        || decode_base64(&recipient.certificate_fingerprint)?
            != recipient_certificate.fingerprint()?
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    let verified_sender =
        verify_device_certificate(sender_identity.certificate(), sender_root, now_ms, false)?;
    let verified_recipient = verify_device_certificate(
        &recipient_certificate,
        expected_recipient_root,
        now_ms,
        recipient.revoked,
    )?;
    Ok(taskveil_crypto::organization::wrap_dek_for_device(
        sender_identity.private(),
        verified_sender,
        verified_recipient,
        scope_kind,
        scope_id,
        generation,
        dek,
    )?)
}

pub fn unwrap_organization_dek_from_verified_device(
    recipient_identity: &DeviceIdentity,
    recipient_root: &AccountRootPublicKeys,
    sender: &crate::organization::OrganizationDeviceDto,
    expected_sender_root: &AccountRootPublicKeys,
    package: &HybridDekPackage,
    now_ms: i64,
) -> Result<Zeroizing<[u8; KEY_LEN]>, AccountClientError> {
    if sender.revoked
        || sender.user_id != expected_sender_root.user_id
        || decode_base64(&sender.account_root_public)? != expected_sender_root.encode()?
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    let sender_certificate = DeviceCertificate::decode(&decode_base64(&sender.certificate)?)?;
    if sender_certificate.device_id != sender.device_id
        || decode_base64(&sender.certificate_fingerprint)? != sender_certificate.fingerprint()?
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    let verified_sender = verify_device_certificate(
        &sender_certificate,
        expected_sender_root,
        now_ms,
        sender.revoked,
    )?;
    let verified_recipient = verify_device_certificate(
        recipient_identity.certificate(),
        recipient_root,
        now_ms,
        false,
    )?;
    Ok(taskveil_crypto::organization::unwrap_dek_for_device(
        recipient_identity.private(),
        verified_sender,
        verified_recipient,
        package,
    )?)
}

/// Short-lived authorization material for the foreground notification
/// channel. Callers must treat `ticket` as a secret and pass it only in the
/// WebSocket Upgrade Authorization header.
pub struct RealtimeTicketResponse {
    pub websocket_url: String,
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

pub struct HistoricalKeyMaterial {
    pub generation: u64,
    pub tenant_root_dek: Zeroizing<[u8; KEY_LEN]>,
}

pub fn password_wrapper_update(
    user_id: Uuid,
    generation: u64,
    current_revision: u64,
    master_key: &[u8; KEY_LEN],
    new_opaque_export_key: &[u8],
    existing_recovery_wrapper: String,
) -> Result<UpdateKeyWrappersRequest, AccountClientError> {
    let next_revision = current_revision
        .checked_add(1)
        .ok_or(AccountClientError::KeyBundleConflict)?;
    let kek = Zeroizing::new(derive_kek_pw(new_opaque_export_key));
    Ok(UpdateKeyWrappersRequest {
        suite_id: CRYPTO_SUITE_ID,
        generation,
        expected_wrapper_revision: current_revision,
        wrapper_revision: next_revision,
        wrapped_master_key_by_password: STANDARD.encode(wrap_master_key_with_kek_pw(
            user_id, generation, master_key, &kek,
        )?),
        wrapped_master_key_by_recovery: existing_recovery_wrapper,
    })
}

pub fn recovery_wrapper_reissue(
    user_id: Uuid,
    generation: u64,
    current_revision: u64,
    master_key: &[u8; KEY_LEN],
    existing_password_wrapper: String,
) -> Result<(UpdateKeyWrappersRequest, Zeroizing<String>), AccountClientError> {
    let next_revision = current_revision
        .checked_add(1)
        .ok_or(AccountClientError::KeyBundleConflict)?;
    let recovery_key = generate_recovery_key();
    let recovery_wrap_key = Zeroizing::new(derive_recovery_wrap_key(&recovery_key)?);
    let request = UpdateKeyWrappersRequest {
        suite_id: CRYPTO_SUITE_ID,
        generation,
        expected_wrapper_revision: current_revision,
        wrapper_revision: next_revision,
        wrapped_master_key_by_password: existing_password_wrapper,
        wrapped_master_key_by_recovery: STANDARD.encode(wrap_master_key_with_recovery_key(
            user_id,
            generation,
            master_key,
            &recovery_wrap_key,
        )?),
    };
    Ok((request, recovery_key))
}

impl AccountClient {
    pub fn new(server_url: impl Into<String>) -> Result<Self, AccountClientError> {
        let base_url = normalize_base_url(server_url.into())?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { base_url, http })
    }

    pub async fn begin_registration(
        &self,
        email: &str,
    ) -> Result<AccountRegistrationMailbox, AccountClientError> {
        let prepared = self.prepare_registration_request(email)?;
        self.send_registration_request(&prepared).await
    }

    pub fn prepare_registration_request(
        &self,
        email: &str,
    ) -> Result<AccountRegistrationRequestPrepared, AccountClientError> {
        let mut handoff_secret = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *handoff_secret);
        let handoff_challenge: [u8; 32] = Sha256::digest(handoff_secret.as_slice()).into();
        let request_body = serde_json::to_vec(&RegistrationRequest {
            email: email.to_string(),
            handoff_challenge: URL_SAFE_NO_PAD.encode(handoff_challenge),
        })
        .map_err(|_| AccountClientError::Opaque)?;
        Ok(AccountRegistrationRequestPrepared {
            version: 1,
            origin: self.base_url.clone(),
            email: email.to_string(),
            request_body,
            request_idempotency_key: Uuid::now_v7().to_string(),
            handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret.as_slice()),
            expires_at_ms: Utc::now().timestamp_millis()
                + chrono::Duration::minutes(35).num_milliseconds(),
        })
    }

    pub async fn send_registration_request(
        &self,
        prepared: &AccountRegistrationRequestPrepared,
    ) -> Result<AccountRegistrationMailbox, AccountClientError> {
        if prepared.version != 1
            || prepared.origin != self.base_url
            || Utc::now().timestamp_millis() >= prepared.expires_at_ms
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let verification = self
            .post_json_bytes_with_idempotency::<RegistrationRequestResponse>(
                "/v1/auth/register/request",
                &prepared.request_body,
                &prepared.request_idempotency_key,
            )
            .await?;
        Ok(AccountRegistrationMailbox {
            version: 1,
            origin: self.base_url.clone(),
            email: prepared.email.clone(),
            request_id: verification.request_id.to_string(),
            handoff_secret: prepared.handoff_secret.clone(),
            resend_idempotency_key: Uuid::now_v7().to_string(),
            verify_idempotency_key: Uuid::now_v7().to_string(),
            verify_request_binding: None,
            expires_at_ms: verification.expires_at.timestamp_millis(),
            next_retry_at_ms: verification.next_retry_at.timestamp_millis(),
        })
    }

    pub async fn resend_registration(
        &self,
        mailbox: &mut AccountRegistrationMailbox,
    ) -> Result<(), AccountClientError> {
        if mailbox.version != 1
            || mailbox.origin != self.base_url
            || Utc::now().timestamp_millis() >= mailbox.expires_at_ms
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let now_ms = Utc::now().timestamp_millis();
        if mailbox.next_retry_at_ms > now_ms {
            return Err(AccountClientError::EmailVerificationRetryAt(
                mailbox.next_retry_at_ms,
            ));
        }
        let request_id =
            Uuid::parse_str(&mailbox.request_id).map_err(|_| AccountClientError::Opaque)?;
        let response = self
            .http
            .post(format!("{}{}", self.base_url, "/v1/auth/register/resend"))
            .header("idempotency-key", &mailbox.resend_idempotency_key)
            .json(&RegistrationResendRequest {
                request_id,
                handoff_secret: mailbox.handoff_secret.clone(),
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        let response = response
            .json::<RegistrationRequestResponse>()
            .await
            .map_err(AccountClientError::Http)?;
        mailbox.apply_resend_response(&response);
        Ok(())
    }

    pub async fn verify_registration_otp(
        &self,
        mailbox: &AccountRegistrationMailbox,
        otp: &str,
    ) -> Result<AccountRegistrationVerified, AccountClientError> {
        if mailbox.version != 1
            || mailbox.origin != self.base_url
            || Utc::now().timestamp_millis() >= mailbox.expires_at_ms
            || otp.len() != 8
            || !otp.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let request_id =
            Uuid::parse_str(&mailbox.request_id).map_err(|_| AccountClientError::Opaque)?;
        let response = self
            .http
            .post(format!("{}{}", self.base_url, "/v1/auth/register/verify"))
            .header("idempotency-key", &mailbox.verify_idempotency_key)
            .json(&RegistrationVerifyRequest {
                request_id,
                handoff_secret: mailbox.handoff_secret.clone(),
                otp: otp.to_string(),
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        let response = response
            .json::<RegistrationVerifyResponse>()
            .await
            .map_err(AccountClientError::Http)?;
        Ok(AccountRegistrationVerified {
            version: 1,
            origin: mailbox.origin.clone(),
            email: mailbox.email.clone(),
            request_id: mailbox.request_id.clone(),
            handoff_secret: mailbox.handoff_secret.clone(),
            registration_ticket: response.registration_ticket,
            expires_at_ms: response.expires_at.timestamp_millis(),
        })
    }

    pub async fn prepare_registration(
        &self,
        verified: &AccountRegistrationVerified,
        password: &str,
        device_name: Option<&str>,
        device_key: &[u8; KEY_LEN],
    ) -> Result<AccountRegistrationPrepared, AccountClientError> {
        let start = self.prepare_registration_start(verified, password, device_name)?;
        self.send_registration_start(&start, password, device_key)
            .await
    }

    pub fn prepare_registration_start(
        &self,
        verified: &AccountRegistrationVerified,
        password: &str,
        device_name: Option<&str>,
    ) -> Result<AccountRegistrationStartPrepared, AccountClientError> {
        if verified.version != 1
            || verified.origin != self.base_url
            || Utc::now().timestamp_millis() >= verified.expires_at_ms
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let mut rng = OsRng;
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let client_start = ClientRegistration::<TaskveilCipherSuite>::start(&mut rng, &password)
            .map_err(|_| AccountClientError::Opaque)?;
        let start_body = serde_json::to_vec(&RegistrationOpaqueStartRequest {
            registration_ticket: verified.registration_ticket.clone(),
            device_name: device_name.map(ToOwned::to_owned),
            opaque_suite_id: CRYPTO_SUITE_ID,
            message: STANDARD.encode(client_start.message.serialize()),
        })
        .map_err(|_| AccountClientError::Opaque)?;
        Ok(AccountRegistrationStartPrepared {
            version: 1,
            origin: self.base_url.clone(),
            email: verified.email.clone(),
            request_id: verified.request_id.clone(),
            handoff_secret: verified.handoff_secret.clone(),
            start_body,
            start_idempotency_key: Uuid::now_v7().to_string(),
            client_registration_state: client_start.state.serialize().to_vec(),
            // A sent start may be replayed after the ticket itself expires.
            // The server returns the exact deadline with the start response;
            // before that response is known, retain the journal for its
            // maximum five-minute replay window.
            expires_at_ms: verified.expires_at_ms + chrono::Duration::minutes(5).num_milliseconds(),
        })
    }

    pub async fn send_registration_start(
        &self,
        prepared: &AccountRegistrationStartPrepared,
        password: &str,
        device_key: &[u8; KEY_LEN],
    ) -> Result<AccountRegistrationPrepared, AccountClientError> {
        if prepared.version != 1
            || prepared.origin != self.base_url
            || Utc::now().timestamp_millis() >= prepared.expires_at_ms
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let mut rng = OsRng;
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let start = self
            .post_json_bytes_with_idempotency::<RegistrationStartResponse>(
                "/v1/auth/register/start",
                &prepared.start_body,
                &prepared.start_idempotency_key,
            )
            .await?;
        validate_registration_start(&start)?;
        let server_message = RegistrationResponse::<TaskveilCipherSuite>::deserialize(
            &decode_base64(&start.message)?,
        )
        .map_err(|_| AccountClientError::Opaque)?;
        let client_state = ClientRegistration::<TaskveilCipherSuite>::deserialize(
            &prepared.client_registration_state,
        )
        .map_err(|_| AccountClientError::Opaque)?;
        let client_finish = client_state
            .finish(
                &mut rng,
                &password,
                server_message,
                opaque_registration_parameters(),
            )
            .map_err(|_| AccountClientError::Opaque)?;
        let mut export_key = Zeroizing::new(client_finish.export_key.to_vec());
        let key_setup =
            build_registration_key_bundle(start.user_id, start.tenant_id, &export_key, device_key)?;
        export_key.zeroize();

        let device_keys = generate_device_keys()?;
        let now_ms = Utc::now().timestamp_millis();
        let certificate = issue_device_certificate(
            &key_setup.keys.account_root_private,
            &key_setup.keys.account_root_public,
            start.device_id,
            &device_keys,
            now_ms,
            now_ms + chrono::Duration::days(365).num_milliseconds(),
        )?;
        let challenge = decode_fixed_array::<DEVICE_CHALLENGE_LEN>(&start.device_challenge)?;
        let proof = create_device_proof(&device_keys.private, &certificate, &challenge)?;
        let enrollment =
            device_enrollment_dto(&key_setup.keys.account_root_public, &certificate, &proof)?;
        let device_identity = DeviceIdentity::new(device_keys.private, certificate)?;
        let finish_request = RegisterFinishRequest {
            state_id: start.state_id,
            message: STANDARD.encode(client_finish.message.serialize()),
            key_bundle: key_setup.bundle,
            device_enrollment: enrollment,
        };
        let finish_body =
            serde_json::to_vec(&finish_request).map_err(|_| AccountClientError::Opaque)?;
        let account_root_private = key_setup.keys.account_root_private.encode().to_vec();
        let account_root_public = key_setup.keys.account_root_public.encode()?;
        let device_identity = device_identity.encode()?.to_vec();
        Ok(AccountRegistrationPrepared {
            version: 1,
            origin: self.base_url.clone(),
            email: prepared.email.clone(),
            request_id: prepared.request_id.clone(),
            handoff_secret: prepared.handoff_secret.clone(),
            finish_body,
            start_idempotency_key: prepared.start_idempotency_key.clone(),
            finish_idempotency_key: Uuid::now_v7().to_string(),
            expires_at_ms: start.expires_at.timestamp_millis(),
            recovery_key: key_setup.recovery_key.to_string(),
            local_wrapped_master_key: key_setup.local_wrapped_master_key,
            generation: key_setup.keys.generation,
            tenant_generation: key_setup.keys.tenant_generation,
            master_key: key_setup.keys.master_key.to_vec(),
            account_root_private,
            account_root_public,
            tenant_root_dek: key_setup.keys.tenant_root_dek.to_vec(),
            device_identity,
        })
    }

    pub async fn finish_registration(
        &self,
        prepared: &AccountRegistrationPrepared,
    ) -> Result<AccountRegisterOutcome, AccountClientError> {
        if prepared.version != 1
            || prepared.origin != self.base_url
            || Utc::now().timestamp_millis() >= prepared.expires_at_ms
        {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let session = self
            .post_json_bytes_with_idempotency::<SessionResponse>(
                "/v1/auth/register/finish",
                &prepared.finish_body,
                &prepared.finish_idempotency_key,
            )
            .await?;
        Self::registration_outcome(prepared, session)
    }

    pub async fn reconcile_registration(
        &self,
        prepared: &AccountRegistrationPrepared,
    ) -> Result<AccountRegistrationReconcile, AccountClientError> {
        if prepared.version != 1 || prepared.origin != self.base_url {
            return Err(AccountClientError::EmailVerificationExpired);
        }
        let request_id =
            Uuid::parse_str(&prepared.request_id).map_err(|_| AccountClientError::Opaque)?;
        let response = self
            .http
            .post(format!("{}{}", self.base_url, "/v1/auth/register/status"))
            .json(&RegistrationStatusRequest {
                request_id,
                handoff_secret: prepared.handoff_secret.clone(),
                start_idempotency_key: prepared.start_idempotency_key.clone(),
                finish_idempotency_key: prepared.finish_idempotency_key.clone(),
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        let response = response
            .json::<RegistrationStatusResponse>()
            .await
            .map_err(AccountClientError::Http)?;
        match (response.status.as_str(), response.result) {
            ("pending", None) => Ok(AccountRegistrationReconcile::Pending),
            ("committed", Some(session)) => Self::registration_outcome(prepared, session)
                .map(Box::new)
                .map(AccountRegistrationReconcile::Committed),
            _ => Err(AccountClientError::Opaque),
        }
    }

    fn registration_outcome(
        prepared: &AccountRegistrationPrepared,
        session: SessionResponse,
    ) -> Result<AccountRegisterOutcome, AccountClientError> {
        let mut master_key = Zeroizing::new([0u8; KEY_LEN]);
        master_key.copy_from_slice(&prepared.master_key);
        let mut tenant_root_dek = Zeroizing::new([0u8; KEY_LEN]);
        tenant_root_dek.copy_from_slice(&prepared.tenant_root_dek);
        Ok(AccountRegisterOutcome {
            session: session.into_account_session(&prepared.email),
            recovery_key: Zeroizing::new(prepared.recovery_key.clone()),
            local_wrapped_master_key: prepared.local_wrapped_master_key.clone(),
            keys: AccountKeyMaterial {
                generation: prepared.generation,
                tenant_generation: prepared.tenant_generation,
                master_key,
                account_root_private: AccountRootPrivateKeys::decode(
                    &prepared.account_root_private,
                )?,
                account_root_public: AccountRootPublicKeys::decode(&prepared.account_root_public)?,
                tenant_root_dek,
            },
            device_identity: DeviceIdentity::decode(&prepared.device_identity)?,
        })
    }

    pub async fn begin_login(
        &self,
        email: &str,
        password: &str,
        device_name: Option<&str>,
        device_key: &[u8; KEY_LEN],
    ) -> Result<AccountLoginProvisional, AccountClientError> {
        let mut rng = OsRng;
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let client_start = ClientLogin::<TaskveilCipherSuite>::start(&mut rng, &password)
            .map_err(|_| AccountClientError::Opaque)?;
        let start = self
            .post_json::<LoginStartResponse>(
                "/v1/auth/login/start",
                &OpaqueStartRequest {
                    email: email.to_string(),
                    device_name: device_name.map(ToOwned::to_owned),
                    opaque_suite_id: CRYPTO_SUITE_ID,
                    message: STANDARD.encode(client_start.message.serialize()),
                },
                None,
            )
            .await?;
        validate_login_start(&start)?;
        let server_message =
            CredentialResponse::<TaskveilCipherSuite>::deserialize(&decode_base64(&start.message)?)
                .map_err(|_| AccountClientError::Opaque)?;
        let client_finish = client_start
            .state
            .finish(
                &mut rng,
                &password,
                server_message,
                opaque_login_parameters(),
            )
            .map_err(|_| AccountClientError::Opaque)?;
        let mut export_key = Zeroizing::new(client_finish.export_key.to_vec());
        let response = self
            .post_json::<LoginFinishResponse>(
                "/v1/auth/login/finish",
                &LoginFinishRequest {
                    state_id: start.state_id,
                    message: STANDARD.encode(client_finish.message.serialize()),
                },
                None,
            )
            .await?;
        let keys = unwrap_login_key_bundle(
            &response.key_bundle,
            response.session.user_id,
            response.session.tenant_id,
            &export_key,
        )?;
        export_key.zeroize();
        let device_keys = generate_device_keys()?;
        let now_ms = Utc::now().timestamp_millis();
        let certificate = issue_device_certificate(
            &keys.account_root_private,
            &keys.account_root_public,
            response.session.device_id,
            &device_keys,
            now_ms,
            now_ms + chrono::Duration::days(365).num_milliseconds(),
        )?;
        let challenge = decode_fixed_array::<DEVICE_CHALLENGE_LEN>(&response.device_challenge)?;
        let proof = create_device_proof(&device_keys.private, &certificate, &challenge)?;
        let enrollment = device_enrollment_dto(&keys.account_root_public, &certificate, &proof)?;
        let device_identity = DeviceIdentity::new(device_keys.private, certificate)?;
        let local_wrapped_master_key = wrap_master_key_with_device_key(
            response.session.user_id,
            response.key_bundle.generation,
            &keys.master_key,
            device_key,
        )?;

        Ok(AccountLoginProvisional {
            session: response.session.into_account_session(email),
            local_wrapped_master_key,
            keys,
            device_identity,
            enrollment,
            challenge_expires_at_ms: response.device_challenge_expires_at.timestamp_millis(),
        })
    }

    pub fn registration_matches_account_keys(
        prepared: &AccountRegistrationPrepared,
        keys: &AccountKeyMaterial,
    ) -> Result<bool, AccountClientError> {
        let login_root_public = keys.account_root_public.encode()?;
        let same_generation = prepared.generation == keys.generation;
        let same_tenant_generation = prepared.tenant_generation == keys.tenant_generation;
        let same_master_key = prepared
            .master_key
            .as_slice()
            .ct_eq(keys.master_key.as_slice())
            .into();
        let same_account_root = prepared
            .account_root_public
            .as_slice()
            .ct_eq(login_root_public.as_slice())
            .into();
        let same_tenant_root = prepared
            .tenant_root_dek
            .as_slice()
            .ct_eq(keys.tenant_root_dek.as_slice())
            .into();
        Ok(same_generation
            && same_tenant_generation
            && same_master_key
            && same_account_root
            && same_tenant_root)
    }

    pub async fn certify_login(
        &self,
        provisional: &AccountLoginProvisional,
    ) -> Result<(), AccountClientError> {
        self.post_json::<LogoutResponse>(
            "/v1/auth/device/certify",
            &provisional.enrollment,
            Some(&provisional.session.tokens.access_token),
        )
        .await?;
        Ok(())
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
    ) -> Result<AccountTokenSet, AccountClientError> {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, "/v1/auth/token"))
            .form(&TokenRequest {
                grant_type: "refresh_token",
                refresh_token,
                client_id: NATIVE_CLIENT_ID,
            })
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(AccountClientError::InvalidGrant);
        }
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response
            .json::<TokenResponse>()
            .await
            .map(TokenResponse::into_account_token_set)
            .map_err(AccountClientError::Http)
    }

    pub async fn logout(&self, refresh_token: &str) -> Result<(), AccountClientError> {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, "/v1/auth/revoke"))
            .form(&RevocationRequest {
                token: refresh_token,
                token_type_hint: "refresh_token",
                client_id: NATIVE_CLIENT_ID,
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        Ok(())
    }

    pub async fn invite_organization_member(
        &self,
        tenant_id: Uuid,
        email: String,
        session_token: &str,
    ) -> Result<crate::organization::OrganizationMemberResponse, AccountClientError> {
        self.post_protocol_json(
            &format!("/v2/tenants/{tenant_id}/organization/invites"),
            &crate::organization::OrganizationInviteRequest { email },
            session_token,
        )
        .await
    }

    pub async fn organization_safety_number(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        session_token: &str,
    ) -> Result<crate::organization::OrganizationSafetyResponse, AccountClientError> {
        let response = self
            .get_protocol_json(
                &format!("/v2/tenants/{tenant_id}/organization/safety/{member_user_id}"),
                session_token,
            )
            .await?;
        verify_safety_response(&response)?;
        if response.member_user_id != member_user_id {
            return Err(AccountClientError::OrganizationVerification);
        }
        Ok(response)
    }

    pub async fn confirm_organization_safety_number(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        digest: String,
        session_token: &str,
    ) -> Result<crate::organization::OrganizationSafetyResponse, AccountClientError> {
        let current = self
            .organization_safety_number(tenant_id, member_user_id, session_token)
            .await?;
        if current.digest != digest {
            return Err(AccountClientError::OrganizationVerification);
        }
        let response = self
            .post_protocol_json(
                &format!("/v2/tenants/{tenant_id}/organization/safety/confirm"),
                &crate::organization::OrganizationSafetyConfirmRequest {
                    member_user_id,
                    digest,
                },
                session_token,
            )
            .await?;
        verify_safety_response(&response)?;
        if response.member_user_id != member_user_id {
            return Err(AccountClientError::OrganizationVerification);
        }
        Ok(response)
    }

    pub async fn organization_member_devices(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        trust: OrganizationRosterTrust<'_>,
        session_token: &str,
    ) -> Result<VerifiedOrganizationDeviceRoster, AccountClientError> {
        let safety = self
            .organization_safety_number(tenant_id, member_user_id, session_token)
            .await?;
        if safety.verification_state != "verified"
            || trust.user_id != member_user_id
            || safety.member_user_id != trust.user_id
            || safety.member_root_public != trust.root_public
        {
            return Err(AccountClientError::OrganizationVerification);
        }
        let roster: crate::organization::OrganizationDeviceRosterDto = self
            .get_protocol_json(
                &format!("/v2/tenants/{tenant_id}/organization/devices/{member_user_id}"),
                session_token,
            )
            .await?;
        verify_organization_devices(
            roster,
            trust.user_id,
            trust.root_public,
            trust.minimum_revision,
            trust.minimum_head_hash,
        )
    }

    pub async fn organization_owner_devices(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        trust: OrganizationRosterTrust<'_>,
        session_token: &str,
    ) -> Result<VerifiedOrganizationDeviceRoster, AccountClientError> {
        let safety = self
            .organization_safety_number(tenant_id, member_user_id, session_token)
            .await?;
        if safety.verification_state != "verified"
            || safety.owner_user_id != trust.user_id
            || safety.owner_root_public != trust.root_public
        {
            return Err(AccountClientError::OrganizationVerification);
        }
        let roster: crate::organization::OrganizationDeviceRosterDto = self
            .get_protocol_json(
                &format!(
                    "/v2/tenants/{tenant_id}/organization/devices/{}",
                    trust.user_id
                ),
                session_token,
            )
            .await?;
        verify_organization_devices(
            roster,
            trust.user_id,
            trust.root_public,
            trust.minimum_revision,
            trust.minimum_head_hash,
        )
    }

    pub async fn remove_organization_member(
        &self,
        tenant_id: Uuid,
        member_user_id: Uuid,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        self.delete_protocol(
            &format!("/v2/tenants/{tenant_id}/organization/members/{member_user_id}"),
            session_token,
        )
        .await
    }

    pub async fn revoke_organization_device(
        &self,
        tenant_id: Uuid,
        device_id: Uuid,
        signed_revocation: &SignedDeviceRevocation,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        let _: serde_json::Value = self
            .post_protocol_json(
                &format!("/v2/tenants/{tenant_id}/organization/device-revocations/{device_id}"),
                &crate::organization::OrganizationDeviceRevocationRequest {
                    signed_revocation: STANDARD.encode(signed_revocation.encode()?),
                },
                session_token,
            )
            .await?;
        Ok(())
    }

    pub async fn store_recipient_package(
        &self,
        tenant_id: Uuid,
        device_id: Uuid,
        package: &taskveil_crypto::organization::HybridDekPackage,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        let response: crate::organization::RecipientPackageResponse = self
            .post_protocol_json(
                &recipient_package_path(tenant_id, package),
                &crate::organization::RecipientPackageRequest {
                    device_id,
                    package: STANDARD.encode(package.encode()?),
                },
                session_token,
            )
            .await?;
        if decode_base64(&response.package)? != package.encode()? {
            return Err(AccountClientError::OrganizationVerification);
        }
        Ok(())
    }

    pub async fn load_recipient_package(
        &self,
        tenant_id: Uuid,
        scope_kind: taskveil_crypto::organization::HybridScopeKind,
        scope_id: Uuid,
        generation: u64,
        session_token: &str,
    ) -> Result<taskveil_crypto::organization::HybridDekPackage, AccountClientError> {
        let path = format!(
            "/v2/tenants/{tenant_id}/organization/recipients/{}/{scope_id}/{generation}",
            scope_kind as u8
        );
        let response: crate::organization::RecipientPackageResponse =
            self.get_protocol_json(&path, session_token).await?;
        let package = taskveil_crypto::organization::HybridDekPackage::decode(&decode_base64(
            &response.package,
        )?)?;
        if package.scope_kind != scope_kind
            || package.scope_id != scope_id
            || package.generation != generation
        {
            return Err(AccountClientError::OrganizationVerification);
        }
        Ok(package)
    }

    pub async fn active_key_bundle(
        &self,
        tenant_id: Uuid,
        session_token: &str,
    ) -> Result<ActiveKeyBundleDto, AccountClientError> {
        let response = self
            .http
            .get(format!(
                "{}/v2/tenants/{tenant_id}/key-rotation/bundle",
                self.base_url
            ))
            .bearer_auth(session_token)
            .header(
                crate::protocol::SYNC_PROTOCOL_VERSION_HEADER,
                crate::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response.json().await.map_err(AccountClientError::Http)
    }

    pub async fn acknowledge_key_generation(
        &self,
        tenant_id: Uuid,
        generation: u64,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        let response = self
            .http
            .post(format!(
                "{}/v2/tenants/{tenant_id}/key-rotation/ack",
                self.base_url
            ))
            .bearer_auth(session_token)
            .header(
                crate::protocol::SYNC_PROTOCOL_VERSION_HEADER,
                crate::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(&serde_json::json!({ "generation": generation }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        Ok(())
    }

    pub async fn update_key_wrappers(
        &self,
        request: &UpdateKeyWrappersRequest,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        self.post_json::<serde_json::Value>("/v1/auth/key-wrappers", request, Some(session_token))
            .await?;
        Ok(())
    }

    pub async fn realtime_ticket(
        &self,
        tenant_id: Uuid,
        session_token: &str,
    ) -> Result<RealtimeTicketResponse, AccountClientError> {
        let response: RealtimeTicketWireResponse = self
            .post_protocol_json(
                &format!("/v2/tenants/{tenant_id}/realtime/ticket"),
                &serde_json::json!({}),
                session_token,
            )
            .await?;
        Ok(response.into())
    }

    pub async fn billing(
        &self,
        tenant_id: Uuid,
        session_token: &str,
    ) -> Result<BillingResponseDto, AccountClientError> {
        self.get_json(
            &format!("/v2/tenants/{tenant_id}/billing"),
            Some(session_token),
        )
        .await
    }

    pub async fn refresh_billing(
        &self,
        tenant_id: Uuid,
        session_token: &str,
    ) -> Result<BillingResponseDto, AccountClientError> {
        self.post_json(
            &format!("/v2/tenants/{tenant_id}/billing/refresh"),
            &serde_json::json!({}),
            Some(session_token),
        )
        .await
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        bearer_token: Option<&str>,
    ) -> Result<T, AccountClientError> {
        let mut request = self.http.get(format!("{}{}", self.base_url, path));
        if let Some(token) = bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response.json::<T>().await.map_err(AccountClientError::Http)
    }

    async fn get_protocol_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        session_token: &str,
    ) -> Result<T, AccountClientError> {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .bearer_auth(session_token)
            .header(
                crate::protocol::SYNC_PROTOCOL_VERSION_HEADER,
                crate::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response.json().await.map_err(AccountClientError::Http)
    }

    async fn post_protocol_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &impl Serialize,
        session_token: &str,
    ) -> Result<T, AccountClientError> {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(session_token)
            .header(
                crate::protocol::SYNC_PROTOCOL_VERSION_HEADER,
                crate::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response.json().await.map_err(AccountClientError::Http)
    }

    async fn delete_protocol(
        &self,
        path: &str,
        session_token: &str,
    ) -> Result<(), AccountClientError> {
        let response = self
            .http
            .delete(format!("{}{}", self.base_url, path))
            .bearer_auth(session_token)
            .header(
                crate::protocol::SYNC_PROTOCOL_VERSION_HEADER,
                crate::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        Ok(())
    }

    async fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &impl Serialize,
        bearer_token: Option<&str>,
    ) -> Result<T, AccountClientError> {
        let mut request = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .json(body);
        if let Some(token) = bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(account_response_error(response.status()));
        }
        response.json::<T>().await.map_err(AccountClientError::Http)
    }

    async fn post_json_bytes_with_idempotency<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &[u8],
        idempotency_key: &str,
    ) -> Result<T, AccountClientError> {
        let mut last_error = None;
        for attempt in 0..3 {
            let response = self
                .http
                .post(format!("{}{}", self.base_url, path))
                .header("content-type", "application/json")
                .header("idempotency-key", idempotency_key)
                .body(body.to_vec())
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 2 {
                        continue;
                    }
                    break;
                }
            };
            if !response.status().is_success() {
                return Err(account_response_error(response.status()));
            }
            match response.json::<T>().await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 2 {
                        continue;
                    }
                }
            }
        }
        Err(AccountClientError::Http(last_error.expect(
            "an idempotent request attempt always records an error",
        )))
    }
}

pub fn unwrap_login_key_bundle(
    bundle: &AccountKeyBundleDto,
    user_id: Uuid,
    tenant_id: Uuid,
    export_key: &[u8],
) -> Result<AccountKeyMaterial, AccountClientError> {
    validate_key_bundle_header(bundle)?;
    let mut kek_pw = Zeroizing::new(derive_kek_pw(export_key));
    let master_key = Zeroizing::new(unwrap_master_key_with_kek_pw(
        user_id,
        bundle.generation,
        &decode_base64(&bundle.wrapped_master_key_by_password)?,
        &kek_pw,
    )?);
    kek_pw.zeroize();

    let account_root_private_bytes = unwrap_account_root_private_key_with_master_key(
        user_id,
        bundle.generation,
        &decode_base64(&bundle.wrapped_account_root_private)?,
        &master_key,
    )?;
    let account_root_private = AccountRootPrivateKeys::decode(&*account_root_private_bytes)?;
    let account_root_public =
        AccountRootPublicKeys::decode(&decode_base64(&bundle.account_root_public)?)?;
    if account_root_public.user_id != user_id {
        return Err(AccountClientError::KeyBundleConflict);
    }
    if account_root_private.public_keys(user_id)? != account_root_public {
        return Err(AccountClientError::KeyBundleConflict);
    }
    let tenant_root_dek = Zeroizing::new(unwrap_tenant_root_dek_with_master_key(
        tenant_id,
        bundle.tenant_generation,
        &decode_base64(&bundle.wrapped_tenant_root_dek)?,
        &master_key,
    )?);
    let tenant_manifest =
        KeyManifest::from_authenticated_bytes(&decode_base64(&bundle.tenant_key_manifest)?)?;
    tenant_manifest.verify_personal(&master_key)?;
    if tenant_manifest.tenant_id != tenant_id
        || tenant_manifest.generation != bundle.tenant_generation
        || tenant_manifest.minimum_write_generation != bundle.tenant_generation
        || !matches!(
            tenant_manifest.status,
            RotationStatus::Active | RotationStatus::Migrating
        )
    {
        return Err(AccountClientError::KeyBundleConflict);
    }
    Ok(AccountKeyMaterial {
        generation: bundle.generation,
        tenant_generation: bundle.tenant_generation,
        master_key,
        account_root_private,
        account_root_public,
        tenant_root_dek,
    })
}

pub fn unwrap_active_key_bundle(
    tenant_id: Uuid,
    bundle: &ActiveKeyBundleDto,
    master_key: &[u8; KEY_LEN],
) -> Result<Zeroizing<[u8; KEY_LEN]>, AccountClientError> {
    if bundle.suite_id != CRYPTO_SUITE_ID || bundle.generation == 0 {
        return Err(AccountClientError::KeyBundleConflict);
    }
    let manifest = KeyManifest::from_authenticated_bytes(&decode_base64(&bundle.signed_manifest)?)?;
    manifest.verify_personal(master_key)?;
    if manifest.tenant_id != tenant_id
        || manifest.generation != bundle.generation
        || manifest.minimum_write_generation != bundle.generation
        || manifest.status != RotationStatus::Active
    {
        return Err(AccountClientError::KeyBundleConflict);
    }
    let tenant_root_dek = Zeroizing::new(unwrap_tenant_root_dek_with_master_key(
        tenant_id,
        bundle.generation,
        &decode_base64(&bundle.wrapped_tenant_root_dek)?,
        master_key,
    )?);
    Ok(tenant_root_dek)
}

pub fn unwrap_historical_key_bundles(
    tenant_id: Uuid,
    bundles: &[HistoricalKeyBundleDto],
    master_key: &[u8; KEY_LEN],
) -> Result<Vec<HistoricalKeyMaterial>, AccountClientError> {
    let mut result = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        if bundle.generation == 0 {
            return Err(AccountClientError::KeyBundleConflict);
        }
        let manifest =
            KeyManifest::from_authenticated_bytes(&decode_base64(&bundle.signed_manifest)?)?;
        manifest.verify_personal(master_key)?;
        if manifest.tenant_id != tenant_id
            || manifest.generation != bundle.generation
            || !matches!(
                manifest.status,
                RotationStatus::Active | RotationStatus::Migrating
            )
        {
            return Err(AccountClientError::KeyBundleConflict);
        }
        let tenant_root_dek = Zeroizing::new(unwrap_tenant_root_dek_with_master_key(
            tenant_id,
            bundle.generation,
            &decode_base64(&bundle.wrapped_tenant_root_dek)?,
            master_key,
        )?);
        result.push(HistoricalKeyMaterial {
            generation: bundle.generation,
            tenant_root_dek,
        });
    }
    Ok(result)
}

fn build_registration_key_bundle(
    user_id: Uuid,
    tenant_id: Uuid,
    export_key: &[u8],
    device_key: &[u8; KEY_LEN],
) -> Result<RegistrationKeySetup, AccountClientError> {
    let mut kek_pw = Zeroizing::new(derive_kek_pw(export_key));
    let master_key = Zeroizing::new(generate_master_key());
    let recovery_key = generate_recovery_key();
    let mut recovery_wrap_key = Zeroizing::new(derive_recovery_wrap_key(&recovery_key)?);
    let account_root = generate_account_root(user_id)?;
    let tenant_root_dek = Zeroizing::new(generate_tenant_root_dek());

    let wrapped_master_key_by_password =
        wrap_master_key_with_kek_pw(user_id, INITIAL_KEY_GENERATION, &master_key, &kek_pw)?;
    let wrapped_master_key_by_recovery = wrap_master_key_with_recovery_key(
        user_id,
        INITIAL_KEY_GENERATION,
        &master_key,
        &recovery_wrap_key,
    )?;
    let account_root_private_bytes = account_root.private.encode();
    let wrapped_account_root_private = wrap_account_root_private_key_with_master_key(
        user_id,
        INITIAL_KEY_GENERATION,
        &account_root_private_bytes,
        &master_key,
    )?;
    let wrapped_tenant_root_dek = wrap_tenant_root_dek_with_master_key(
        tenant_id,
        INITIAL_KEY_GENERATION,
        &tenant_root_dek,
        &master_key,
    )?;
    let local_wrapped_master_key =
        wrap_master_key_with_device_key(user_id, INITIAL_KEY_GENERATION, &master_key, device_key)?;
    let tenant_key_manifest = KeyManifest::authenticate_personal(
        tenant_id,
        INITIAL_KEY_GENERATION,
        RotationStatus::Active,
        INITIAL_KEY_GENERATION,
        [0; 32],
        Vec::new(),
        &master_key,
    )?
    .authenticated_bytes()?;
    kek_pw.zeroize();
    recovery_wrap_key.zeroize();

    Ok(RegistrationKeySetup {
        bundle: AccountKeyBundleDto {
            suite_id: CRYPTO_SUITE_ID,
            generation: INITIAL_KEY_GENERATION,
            tenant_generation: INITIAL_KEY_GENERATION,
            wrapper_revision: 1,
            wrapped_master_key_by_password: STANDARD.encode(wrapped_master_key_by_password),
            wrapped_master_key_by_recovery: STANDARD.encode(wrapped_master_key_by_recovery),
            account_root_public: STANDARD.encode(account_root.public.encode()?),
            wrapped_account_root_private: STANDARD.encode(wrapped_account_root_private),
            wrapped_tenant_root_dek: STANDARD.encode(wrapped_tenant_root_dek),
            tenant_key_manifest: STANDARD.encode(tenant_key_manifest),
        },
        recovery_key,
        local_wrapped_master_key,
        keys: AccountKeyMaterial {
            generation: INITIAL_KEY_GENERATION,
            tenant_generation: INITIAL_KEY_GENERATION,
            master_key,
            account_root_private: account_root.private,
            account_root_public: account_root.public,
            tenant_root_dek,
        },
    })
}

fn normalize_base_url(mut value: String) -> Result<String, AccountClientError> {
    value = value.trim().to_string();
    if value.is_empty() {
        return Err(AccountClientError::EmptyServerUrl);
    }
    crate::canonical_server_origin(&value).map_err(|_| AccountClientError::InvalidServerOrigin)
}

fn account_response_error(status: reqwest::StatusCode) -> AccountClientError {
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        AccountClientError::EntitlementRequired
    } else {
        AccountClientError::Server(status.as_u16())
    }
}

fn decode_base64(value: &str) -> Result<Vec<u8>, AccountClientError> {
    STANDARD
        .decode(value)
        .map_err(|_| AccountClientError::Base64)
}

fn decode_fixed_array<const N: usize>(value: &str) -> Result<[u8; N], AccountClientError> {
    decode_base64(value)?
        .try_into()
        .map_err(|_| AccountClientError::Base64)
}

fn verify_safety_response(
    response: &crate::organization::OrganizationSafetyResponse,
) -> Result<(), AccountClientError> {
    let owner = AccountRootPublicKeys::decode(&decode_base64(&response.owner_root_public)?)?;
    let member = AccountRootPublicKeys::decode(&decode_base64(&response.member_root_public)?)?;
    if owner.user_id != response.owner_user_id || member.user_id != response.member_user_id {
        return Err(AccountClientError::OrganizationVerification);
    }
    let expected = derive_safety_number(&owner, &member)?;
    if decode_base64(&response.digest)? != expected.digest
        || response.decimal != expected.decimal
        || decode_base64(&response.qr_payload)? != expected.qr_payload
        || !matches!(
            response.verification_state.as_str(),
            "verified" | "unverified"
        )
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    Ok(())
}

fn verify_organization_devices(
    roster: crate::organization::OrganizationDeviceRosterDto,
    user_id: Uuid,
    expected_root: &str,
    minimum_roster_revision: u64,
    minimum_roster_head_hash: [u8; 32],
) -> Result<VerifiedOrganizationDeviceRoster, AccountClientError> {
    let expected_root_bytes = decode_base64(expected_root)?;
    let root = AccountRootPublicKeys::decode(&expected_root_bytes)?;
    if root.user_id != user_id
        || roster.user_id != user_id
        || decode_base64(&roster.account_root_public)? != expected_root_bytes
        || roster.revision < minimum_roster_revision
        || usize::try_from(roster.revision).ok() != Some(roster.signed_revocations.len())
        || roster.devices.is_empty()
    {
        return Err(AccountClientError::OrganizationVerification);
    }
    let mut revoked_fingerprints =
        std::collections::HashSet::with_capacity(roster.signed_revocations.len());
    let mut expected_previous_hash = [0u8; 32];
    let mut pinned_revision_hash = (minimum_roster_revision == 0).then_some([0u8; 32]);
    for (index, encoded) in roster.signed_revocations.iter().enumerate() {
        let statement = SignedDeviceRevocation::decode(&decode_base64(encoded)?)?;
        statement.verify(&root)?;
        if statement.user_id != user_id
            || statement.revision != u64::try_from(index + 1).unwrap_or(u64::MAX)
            || statement.previous_statement_hash != expected_previous_hash
            || !revoked_fingerprints.insert(statement.certificate_fingerprint)
        {
            return Err(AccountClientError::OrganizationVerification);
        }
        expected_previous_hash = statement.authenticated_hash()?;
        if statement.revision == minimum_roster_revision {
            pinned_revision_hash = Some(expected_previous_hash);
        }
    }
    if pinned_revision_hash != Some(minimum_roster_head_hash) {
        return Err(AccountClientError::OrganizationVerification);
    }
    let now_ms = Utc::now().timestamp_millis();
    let mut fingerprints = std::collections::HashSet::with_capacity(roster.devices.len());
    for device in &roster.devices {
        if device.user_id != user_id
            || device.revoked
            || decode_base64(&device.account_root_public)? != expected_root_bytes
        {
            return Err(AccountClientError::OrganizationVerification);
        }
        let certificate = DeviceCertificate::decode(&decode_base64(&device.certificate)?)?;
        if certificate.user_id != user_id || certificate.device_id != device.device_id {
            return Err(AccountClientError::OrganizationVerification);
        }
        verify_device_certificate(&certificate, &root, now_ms, device.revoked)?;
        let fingerprint = certificate.fingerprint()?;
        if decode_base64(&device.certificate_fingerprint)? != fingerprint
            || revoked_fingerprints.contains(&fingerprint)
            || !fingerprints.insert(fingerprint)
        {
            return Err(AccountClientError::OrganizationVerification);
        }
    }
    Ok(VerifiedOrganizationDeviceRoster {
        revision: roster.revision,
        head_hash: expected_previous_hash,
        devices: roster.devices,
    })
}

fn recipient_package_path(tenant_id: Uuid, package: &HybridDekPackage) -> String {
    format!(
        "/v2/tenants/{tenant_id}/organization/recipients/{}/{}/{}",
        package.scope_kind as u8, package.scope_id, package.generation
    )
}

fn device_enrollment_dto(
    root_public: &AccountRootPublicKeys,
    certificate: &DeviceCertificate,
    proof: &DeviceProofOfPossession,
) -> Result<DeviceEnrollmentDto, AccountClientError> {
    Ok(DeviceEnrollmentDto {
        suite_id: CRYPTO_SUITE_ID,
        account_root_public: STANDARD.encode(root_public.encode()?),
        device_certificate: STANDARD.encode(certificate.encode()?),
        certificate_fingerprint: STANDARD.encode(proof.certificate_fingerprint),
        proof_signature: STANDARD.encode(proof.signature),
    })
}

fn validate_registration_start(
    start: &RegistrationStartResponse,
) -> Result<(), AccountClientError> {
    if start.opaque_suite_id != CRYPTO_SUITE_ID
        || start.device_id.is_nil()
        || start.replay_expires_at > start.expires_at
        || decode_fixed_array::<DEVICE_CHALLENGE_LEN>(&start.device_challenge).is_err()
    {
        return Err(AccountClientError::Opaque);
    }
    Ok(())
}

fn validate_login_start(start: &LoginStartResponse) -> Result<(), AccountClientError> {
    if start.opaque_suite_id != CRYPTO_SUITE_ID {
        return Err(AccountClientError::Opaque);
    }
    Ok(())
}

fn validate_key_bundle_header(bundle: &AccountKeyBundleDto) -> Result<(), AccountClientError> {
    if bundle.suite_id != CRYPTO_SUITE_ID
        || bundle.generation == 0
        || bundle.tenant_generation == 0
        || bundle.wrapper_revision == 0
    {
        return Err(AccountClientError::KeyBundleConflict);
    }
    Ok(())
}

struct RegistrationKeySetup {
    bundle: AccountKeyBundleDto,
    recovery_key: Zeroizing<String>,
    local_wrapped_master_key: Vec<u8>,
    keys: AccountKeyMaterial,
}

#[derive(Debug, Serialize)]
struct OpaqueStartRequest {
    email: String,
    device_name: Option<String>,
    opaque_suite_id: u16,
    message: String,
}

#[derive(Serialize)]
struct RegistrationRequest {
    email: String,
    handoff_challenge: String,
}

#[derive(Debug, Deserialize)]
struct RegistrationRequestResponse {
    request_id: Uuid,
    expires_at: DateTime<Utc>,
    next_retry_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationResendRequest {
    request_id: Uuid,
    handoff_secret: String,
}

#[derive(Serialize)]
struct RegistrationVerifyRequest {
    request_id: Uuid,
    handoff_secret: String,
    otp: String,
}

#[derive(Deserialize)]
struct RegistrationVerifyResponse {
    registration_ticket: String,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationOpaqueStartRequest {
    registration_ticket: String,
    device_name: Option<String>,
    opaque_suite_id: u16,
    message: String,
}

#[derive(Deserialize)]
struct RegistrationStartResponse {
    state_id: Uuid,
    opaque_suite_id: u16,
    user_id: Uuid,
    tenant_id: Uuid,
    device_id: Uuid,
    device_challenge: String,
    message: String,
    #[allow(dead_code)]
    expires_at: DateTime<Utc>,
    replay_expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    state_id: Uuid,
    opaque_suite_id: u16,
    message: String,
    #[allow(dead_code)]
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct RegisterFinishRequest {
    state_id: Uuid,
    message: String,
    key_bundle: AccountKeyBundleDto,
    device_enrollment: DeviceEnrollmentDto,
}

#[derive(Serialize)]
struct RegistrationStatusRequest {
    request_id: Uuid,
    handoff_secret: String,
    start_idempotency_key: String,
    finish_idempotency_key: String,
}

#[derive(Deserialize)]
struct RegistrationStatusResponse {
    status: String,
    result: Option<SessionResponse>,
}

#[derive(Debug, Serialize)]
struct LoginFinishRequest {
    state_id: Uuid,
    message: String,
}

#[derive(Deserialize)]
struct LoginFinishResponse {
    #[serde(flatten)]
    session: SessionResponse,
    key_bundle: AccountKeyBundleDto,
    device_challenge: String,
    device_challenge_expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct SessionResponse {
    user_id: Uuid,
    tenant_id: Uuid,
    device_id: Uuid,
    #[serde(flatten)]
    tokens: TokenResponse,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    #[allow(dead_code)]
    expires_in: u64,
    access_expires_at: DateTime<Utc>,
    refresh_token: String,
    #[allow(dead_code)]
    refresh_token_expires_in: u64,
    refresh_expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'static str,
}

#[derive(Serialize)]
struct RevocationRequest<'a> {
    token: &'a str,
    token_type_hint: &'static str,
    client_id: &'static str,
}

#[derive(Debug, Deserialize)]
struct LogoutResponse {}

#[derive(Deserialize)]
struct RealtimeTicketWireResponse {
    websocket_url: String,
    ticket: String,
    expires_at: DateTime<Utc>,
}

impl From<RealtimeTicketWireResponse> for RealtimeTicketResponse {
    fn from(value: RealtimeTicketWireResponse) -> Self {
        Self {
            websocket_url: value.websocket_url,
            ticket: value.ticket,
            expires_at: value.expires_at,
        }
    }
}

impl SessionResponse {
    fn into_account_session(self, email: &str) -> AccountSession {
        AccountSession {
            user_id: self.user_id.to_string(),
            tenant_id: self.tenant_id.to_string(),
            device_id: self.device_id.to_string(),
            email: email.to_string(),
            tokens: self.tokens.into_account_token_set(),
        }
    }
}

impl TokenResponse {
    fn into_account_token_set(self) -> AccountTokenSet {
        AccountTokenSet {
            access_token: Zeroizing::new(self.access_token),
            access_expires_at_ms: self.access_expires_at.timestamp_millis(),
            refresh_token: Zeroizing::new(self.refresh_token),
            refresh_expires_at_ms: self.refresh_expires_at.timestamp_millis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otp_pending_journal_round_trips_durable_retry_deadline() {
        let mailbox = AccountRegistrationMailbox {
            version: 1,
            origin: "https://api.example.com".to_string(),
            email: "owner@example.com".to_string(),
            request_id: Uuid::now_v7().to_string(),
            handoff_secret: URL_SAFE_NO_PAD.encode([0x42; 32]),
            resend_idempotency_key: Uuid::now_v7().to_string(),
            verify_idempotency_key: Uuid::now_v7().to_string(),
            verify_request_binding: None,
            expires_at_ms: Utc::now().timestamp_millis() + 60_000,
            next_retry_at_ms: Utc::now().timestamp_millis() + 30_000,
        };
        let restored = AccountRegistrationMailbox::decode(&mailbox.encode().unwrap()).unwrap();
        assert_eq!(restored.next_retry_at_ms(), mailbox.next_retry_at_ms());
        assert_eq!(restored.request_id(), mailbox.request_id());
    }

    #[test]
    fn otp_idempotency_key_is_stable_for_retry_and_rotates_for_new_input() {
        let mut mailbox = AccountRegistrationMailbox {
            version: 1,
            origin: "https://api.example.com".to_string(),
            email: "owner@example.com".to_string(),
            request_id: Uuid::now_v7().to_string(),
            handoff_secret: URL_SAFE_NO_PAD.encode([0x42; 32]),
            resend_idempotency_key: Uuid::now_v7().to_string(),
            verify_idempotency_key: Uuid::now_v7().to_string(),
            verify_request_binding: None,
            expires_at_ms: Utc::now().timestamp_millis() + 60_000,
            next_retry_at_ms: Utc::now().timestamp_millis() + 30_000,
        };
        mailbox.prepare_otp_attempt("00000001").unwrap();
        let first_key = mailbox.verify_idempotency_key.clone();
        let encoded = mailbox.encode().unwrap();
        let mut restored = AccountRegistrationMailbox::decode(&encoded).unwrap();
        restored.prepare_otp_attempt("00000001").unwrap();
        assert_eq!(restored.verify_idempotency_key, first_key);
        restored.prepare_otp_attempt("00000002").unwrap();
        assert_ne!(restored.verify_idempotency_key, first_key);
        let prior_generation_key = restored.verify_idempotency_key.clone();
        restored.apply_resend_response(&RegistrationRequestResponse {
            request_id: Uuid::parse_str(&restored.request_id).unwrap(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            next_retry_at: Utc::now() + chrono::Duration::minutes(1),
        });
        restored.prepare_otp_attempt("00000002").unwrap();
        assert_ne!(restored.verify_idempotency_key, prior_generation_key);
    }

    #[test]
    fn expired_receipt_login_adopts_recovery_only_for_the_same_account_keys() {
        let user_id = Uuid::now_v7();
        let root = generate_account_root(user_id).unwrap();
        let encoded_root_public = root.public.encode().unwrap();
        let prepared = AccountRegistrationPrepared {
            version: 1,
            origin: "https://api.example.com".to_string(),
            email: "owner@example.com".to_string(),
            request_id: Uuid::now_v7().to_string(),
            handoff_secret: URL_SAFE_NO_PAD.encode([0x42; 32]),
            finish_body: vec![1],
            start_idempotency_key: Uuid::now_v7().to_string(),
            finish_idempotency_key: Uuid::now_v7().to_string(),
            expires_at_ms: Utc::now().timestamp_millis() + 60_000,
            recovery_key: "recovery words".to_string(),
            local_wrapped_master_key: vec![0x33; 48],
            generation: 1,
            tenant_generation: 1,
            master_key: vec![0x44; KEY_LEN],
            account_root_private: root.private.encode().to_vec(),
            account_root_public: encoded_root_public,
            tenant_root_dek: vec![0x55; KEY_LEN],
            device_identity: vec![1],
        };
        let matching = AccountKeyMaterial {
            generation: 1,
            tenant_generation: 1,
            master_key: Zeroizing::new([0x44; KEY_LEN]),
            account_root_private: root.private,
            account_root_public: root.public,
            tenant_root_dek: Zeroizing::new([0x55; KEY_LEN]),
        };
        assert!(AccountClient::registration_matches_account_keys(&prepared, &matching).unwrap());

        let different_root = generate_account_root(Uuid::now_v7()).unwrap();
        let existing_account = AccountKeyMaterial {
            generation: 1,
            tenant_generation: 1,
            master_key: Zeroizing::new([0x66; KEY_LEN]),
            account_root_private: different_root.private,
            account_root_public: different_root.public,
            tenant_root_dek: Zeroizing::new([0x77; KEY_LEN]),
        };
        assert!(
            !AccountClient::registration_matches_account_keys(&prepared, &existing_account)
                .unwrap()
        );
    }

    #[test]
    fn maps_payment_required_to_typed_entitlement_error() {
        assert!(matches!(
            account_response_error(reqwest::StatusCode::PAYMENT_REQUIRED),
            AccountClientError::EntitlementRequired
        ));
    }

    #[test]
    fn device_roster_rejects_revocation_omission_rollback_and_replayed_certificate() {
        let user_id = Uuid::now_v7();
        let root = generate_account_root(user_id).unwrap();
        let device = generate_device_keys().unwrap();
        let now = Utc::now().timestamp_millis();
        let certificate = issue_device_certificate(
            &root.private,
            &root.public,
            Uuid::now_v7(),
            &device,
            now - 1_000,
            now + 60_000,
        )
        .unwrap();
        let fingerprint = certificate.fingerprint().unwrap();
        let statement = SignedDeviceRevocation::sign(
            &root.private,
            &root.public,
            certificate.device_id,
            fingerprint,
            1,
            now,
            [0; 32],
        )
        .unwrap();
        let root_encoded = STANDARD.encode(root.public.encode().unwrap());
        let replayed = crate::organization::OrganizationDeviceDto {
            user_id,
            device_id: certificate.device_id,
            account_root_public: root_encoded.clone(),
            certificate: STANDARD.encode(certificate.encode().unwrap()),
            certificate_fingerprint: STANDARD.encode(fingerprint),
            revoked: false,
        };
        let roster = crate::organization::OrganizationDeviceRosterDto {
            user_id,
            account_root_public: root_encoded.clone(),
            revision: 1,
            devices: vec![replayed],
            signed_revocations: vec![STANDARD.encode(statement.encode().unwrap())],
        };
        let fork = SignedDeviceRevocation::sign(
            &root.private,
            &root.public,
            certificate.device_id,
            fingerprint,
            1,
            now + 1,
            [0; 32],
        )
        .unwrap();
        let mut forked_roster = roster.clone();
        forked_roster.signed_revocations = vec![STANDARD.encode(fork.encode().unwrap())];
        assert!(matches!(
            verify_organization_devices(
                forked_roster,
                user_id,
                &root_encoded,
                1,
                statement.authenticated_hash().unwrap(),
            ),
            Err(AccountClientError::OrganizationVerification)
        ));
        assert!(matches!(
            verify_organization_devices(
                roster.clone(),
                user_id,
                &root_encoded,
                1,
                statement.authenticated_hash().unwrap(),
            ),
            Err(AccountClientError::OrganizationVerification)
        ));

        let mut omitted = roster;
        omitted.devices.clear();
        omitted.signed_revocations.clear();
        assert!(matches!(
            verify_organization_devices(
                omitted,
                user_id,
                &root_encoded,
                1,
                statement.authenticated_hash().unwrap(),
            ),
            Err(AccountClientError::OrganizationVerification)
        ));
    }

    #[test]
    fn client_rejects_unknown_opaque_suite_before_deserializing_protocol_state() {
        let response = LoginStartResponse {
            state_id: Uuid::now_v7(),
            opaque_suite_id: CRYPTO_SUITE_ID - 1,
            message: String::new(),
            expires_at: Utc::now(),
        };

        assert!(matches!(
            validate_login_start(&response),
            Err(AccountClientError::Opaque)
        ));
    }

    #[test]
    fn client_rejects_key_bundle_generation_downgrade() {
        let bundle = AccountKeyBundleDto {
            suite_id: CRYPTO_SUITE_ID,
            generation: 0,
            tenant_generation: INITIAL_KEY_GENERATION,
            wrapper_revision: 1,
            wrapped_master_key_by_password: String::new(),
            wrapped_master_key_by_recovery: String::new(),
            account_root_public: String::new(),
            wrapped_account_root_private: String::new(),
            wrapped_tenant_root_dek: String::new(),
            tenant_key_manifest: String::new(),
        };

        assert!(matches!(
            validate_key_bundle_header(&bundle),
            Err(AccountClientError::KeyBundleConflict)
        ));
    }

    #[test]
    fn realtime_ticket_wire_uses_only_frontend_authorization_fields() {
        let wire: RealtimeTicketWireResponse = serde_json::from_str(
            r#"{"websocket_url":"wss://realtime.example/v1/connect","ticket":"opaque-ticket","expires_at":"2026-07-15T00:05:00Z"}"#,
        )
        .unwrap();
        let response: RealtimeTicketResponse = wire.into();

        assert_eq!(response.websocket_url, "wss://realtime.example/v1/connect");
        assert_eq!(response.ticket, "opaque-ticket");
        assert_eq!(
            response.expires_at.to_rfc3339(),
            "2026-07-15T00:05:00+00:00"
        );
    }

    #[test]
    fn registration_bundle_unwraps_with_export_key_and_rejects_wrong_key() {
        let export_key = b"opaque export key";
        let wrong_export_key = b"wrong opaque export key";
        let device_key = [0x44; KEY_LEN];
        let user_id = Uuid::now_v7();
        let tenant_id = Uuid::now_v7();

        let setup =
            build_registration_key_bundle(user_id, tenant_id, export_key, &device_key).unwrap();
        let unwrapped =
            unwrap_login_key_bundle(&setup.bundle, user_id, tenant_id, export_key).unwrap();

        assert_eq!(*unwrapped.master_key, *setup.keys.master_key);
        assert_eq!(
            unwrapped.account_root_public,
            setup.keys.account_root_public
        );
        assert_eq!(*unwrapped.tenant_root_dek, *setup.keys.tenant_root_dek);
        assert!(
            unwrap_login_key_bundle(&setup.bundle, user_id, tenant_id, wrong_export_key).is_err()
        );
    }

    #[test]
    fn local_wrapped_master_key_uses_device_key_only_locally() {
        let setup = build_registration_key_bundle(
            Uuid::now_v7(),
            Uuid::now_v7(),
            b"opaque export key",
            &[0x44; KEY_LEN],
        )
        .unwrap();

        assert!(!setup.local_wrapped_master_key.is_empty());
        assert!(!setup
            .bundle
            .wrapped_master_key_by_password
            .contains(&STANDARD.encode(&setup.local_wrapped_master_key)));
    }
}
