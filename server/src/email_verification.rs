use std::sync::Arc;

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use axum::http::StatusCode;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use idna::{domain_to_ascii_cow, uts46::AsciiDenyList};
use rand::{rngs::OsRng, RngCore};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use sqlx_postgres::PgPool;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::AppError;

const TOKEN_BYTES: usize = 32;
const HANDOFF_BYTES: usize = 32;
const VERSIONED_DIGEST_BYTES: usize = 4 + 32;
const SEALED_MAGIC: &[u8; 4] = b"TVE1";
const SEALED_NONCE_BYTES: usize = 12;
const OTP_TTL_MINUTES: i64 = 10;
const REGISTRATION_TICKET_TTL_MINUTES: i64 = 5;
const REGISTRATION_REQUEST_TTL_MINUTES: i64 = 35;
const REGISTRATION_REQUEST_IDEMPOTENCY_TTL_MINUTES: i64 = 35;
const REGISTRATION_FINISH_IDEMPOTENCY_TTL_MINUTES: i64 = 15;
const REGISTRATION_RECONCILE_TTL_HOURS: i64 = 24;
// A successful start must keep the challenge/reservation alive for the full
// OPAQUE state lifetime plus the finish replay window. Otherwise a start near
// the verification deadline could be garbage-collected before its finish
// response can be replayed after a transport failure.
const REGISTRATION_POST_START_TTL_MINUTES: i64 = 25;
const DELIVERY_COOLDOWN_SECONDS: i64 = 60;
const DELIVERY_WINDOW_MINUTES: i64 = 35;
const MAX_DELIVERIES_PER_CANONICAL_WINDOW: i16 = 4;
const MAX_RESENDS_PER_CHALLENGE: i16 = 3;
const MAX_HANDOFF_ATTEMPTS: i16 = 8;
const MAX_OTP_ATTEMPTS: i16 = 5;
const EMAIL_DISPATCH_BATCH_SIZE: i64 = 32;
const EMAIL_DISPATCH_CLAIM_SECONDS: i64 = 30;
const EMAIL_DISPATCH_MAX_ATTEMPTS: i16 = 12;
const EMAIL_REGISTRATION_GC_BATCH_SIZE: i64 = 128;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct EmailVerificationService {
    token_keys: VersionedKeyring,
    state_keys: VersionedKeyring,
    delivery_keys: VersionedKeyring,
    delivery: EmailDeliveryGateway,
    dispatch_trigger_key: Arc<[u8; 32]>,
    deterministic_otp_for_tests: bool,
}

#[derive(Clone)]
struct VersionedKeyring {
    current: VersionedKey,
    previous: Option<VersionedKey>,
}

#[derive(Clone)]
struct VersionedKey {
    version: u32,
    key: Arc<[u8; 32]>,
}

#[derive(Clone)]
pub struct EmailDeliveryGateway {
    endpoint: Url,
    signing_key_id: Arc<str>,
    signing_key: Arc<[u8; 32]>,
    http: reqwest::Client,
}

pub struct EmailVerificationConfig {
    pub token_key_current_version: u32,
    pub token_key_current: [u8; 32],
    pub token_key_previous: Option<(u32, [u8; 32])>,
    pub state_key_current_version: u32,
    pub state_key_current: [u8; 32],
    pub state_key_previous: Option<(u32, [u8; 32])>,
    pub delivery_key_current_version: u32,
    pub delivery_key_current: [u8; 32],
    pub delivery_key_previous: Option<(u32, [u8; 32])>,
    pub delivery_endpoint: String,
    pub delivery_signing_key_id: String,
    pub delivery_signing_key: [u8; 32],
    pub dispatch_trigger_key: [u8; 32],
}

#[derive(Clone)]
pub struct CanonicalEmail {
    display: String,
    canonical: String,
}

impl std::fmt::Debug for CanonicalEmail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CanonicalEmail([redacted])")
    }
}

impl CanonicalEmail {
    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn canonical(&self) -> &str {
        &self.canonical
    }
}

#[derive(Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub email: String,
    pub handoff_challenge: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrationRequestResponse {
    pub request_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub next_retry_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
pub struct RegistrationResendRequest {
    pub request_id: Uuid,
    pub handoff_secret: String,
}

#[derive(Serialize, Deserialize)]
pub struct RegistrationVerifyRequest {
    pub request_id: Uuid,
    pub handoff_secret: String,
    pub otp: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrationVerifyResponse {
    pub registration_ticket: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum StoredRegistrationVerifyOutcome {
    Accepted {
        response: RegistrationVerifyResponse,
    },
    Rejected,
}

#[derive(Serialize, Deserialize)]
pub struct RegistrationStatusRequest {
    pub request_id: Uuid,
    pub handoff_secret: String,
    pub start_idempotency_key: String,
    pub finish_idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RegistrationStatusResponse {
    pub status: &'static str,
    pub result: Option<serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
struct EncryptedRegistration {
    display_email: String,
    canonical_email: String,
    delivery_recipient: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct EmailDeliveryPayload {
    recipient: String,
    otp: String,
    template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailDeliveryCommand {
    pub version: u8,
    pub delivery_id: String,
    pub not_after: DateTime<Utc>,
    pub encrypted_payload: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedEmailDelivery {
    challenge_id: Uuid,
    generation: i32,
    claim_id: Uuid,
    command: EmailDeliveryCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EmailDispatchSummary {
    pub claimed: usize,
    pub accepted: usize,
    pub retryable: usize,
    pub terminal: usize,
}

pub struct VerifiedRegistration {
    pub challenge_id: Uuid,
    pub opaque_credential_id: Uuid,
    pub display_email: String,
    pub canonical_email: String,
    pub is_decoy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryOutcome {
    Accepted,
    Retryable {
        retry_at: Option<DateTime<Utc>>,
        consume_attempt: bool,
    },
    Terminal,
}

impl EmailVerificationService {
    pub fn new(config: EmailVerificationConfig) -> Result<Self, &'static str> {
        let delivery_endpoint =
            secure_endpoint_url(&config.delivery_endpoint).ok_or("invalid delivery endpoint")?;
        if config.token_key_current_version == 0
            || config.state_key_current_version == 0
            || config.delivery_key_current_version == 0
            || config.delivery_signing_key_id.is_empty()
            || config.delivery_signing_key_id.len() > 32
            || !config
                .delivery_signing_key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("invalid email verification key metadata");
        }
        let token_keys = VersionedKeyring::new(
            config.token_key_current_version,
            config.token_key_current,
            config.token_key_previous,
        )?;
        let state_keys = VersionedKeyring::new(
            config.state_key_current_version,
            config.state_key_current,
            config.state_key_previous,
        )?;
        let delivery_keys = VersionedKeyring::new(
            config.delivery_key_current_version,
            config.delivery_key_current,
            config.delivery_key_previous,
        )?;
        let configured_keys = token_keys
            .all_keys()
            .map(<[u8; 32]>::as_slice)
            .chain(state_keys.all_keys().map(<[u8; 32]>::as_slice))
            .chain(delivery_keys.all_keys().map(<[u8; 32]>::as_slice))
            .chain([
                config.delivery_signing_key.as_slice(),
                config.dispatch_trigger_key.as_slice(),
            ]);
        let configured_keys: Vec<&[u8]> = configured_keys.collect();
        if configured_keys.iter().enumerate().any(|(index, key)| {
            configured_keys
                .iter()
                .skip(index + 1)
                .any(|candidate| key == candidate)
        }) {
            return Err("email verification keys must be independent");
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| "invalid delivery client")?;
        Ok(Self {
            token_keys,
            state_keys,
            delivery_keys,
            delivery: EmailDeliveryGateway {
                endpoint: delivery_endpoint,
                signing_key_id: config.delivery_signing_key_id.into(),
                signing_key: Arc::new(config.delivery_signing_key),
                http,
            },
            dispatch_trigger_key: Arc::new(config.dispatch_trigger_key),
            deterministic_otp_for_tests: false,
        })
    }

    #[doc(hidden)]
    pub fn for_tests() -> Self {
        Self::for_tests_with_config(EmailVerificationConfig {
            token_key_current_version: 1,
            token_key_current: [0x71; 32],
            token_key_previous: None,
            state_key_current_version: 1,
            state_key_current: [0x72; 32],
            state_key_previous: None,
            delivery_key_current_version: 1,
            delivery_key_current: [0x75; 32],
            delivery_key_previous: None,
            delivery_endpoint: "http://127.0.0.1:1/v1/enqueue".to_string(),
            delivery_signing_key_id: "test-v1".to_string(),
            delivery_signing_key: [0x73; 32],
            dispatch_trigger_key: [0x74; 32],
        })
    }

    #[doc(hidden)]
    pub fn for_tests_with_config(config: EmailVerificationConfig) -> Self {
        let mut service = Self::new(config).expect("test email verification configuration");
        service.deterministic_otp_for_tests = true;
        service
    }

    fn canonical_digest(&self, canonical_email: &str) -> [u8; VERSIONED_DIGEST_BYTES] {
        self.token_keys
            .digest(b"taskveil/email/canonical/v1\0", canonical_email.as_bytes())
    }

    fn canonical_digest_candidates(&self, canonical_email: &str) -> Vec<Vec<u8>> {
        let purpose = b"taskveil/email/canonical/v1\0";
        let mut candidates = vec![self
            .token_keys
            .digest(purpose, canonical_email.as_bytes())
            .to_vec()];
        if let Some(previous) = &self.token_keys.previous {
            candidates.push(
                self.token_keys
                    .digest_for(previous.version, purpose, canonical_email.as_bytes())
                    .expect("configured previous key is present")
                    .to_vec(),
            );
        }
        candidates
    }

    async fn lock_canonical_candidates(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        candidates: &[Vec<u8>],
    ) -> Result<(), AppError> {
        // Every process holding the same current/previous rotation window
        // acquires both locks in a stable order.  This serializes one mailbox
        // even when one challenge was created under the previous digest key.
        let mut ordered = candidates.to_vec();
        ordered.sort();
        ordered.dedup();
        for digest in ordered {
            sqlx::query(
                "SELECT pg_advisory_xact_lock(
                    hashtextextended(encode($1::bytea, 'hex'), 0)
                 )",
            )
            .bind(digest)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    async fn ensure_token_rotation_supported(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<(), AppError> {
        let supported_versions: Vec<Vec<u8>> = self
            .token_keys
            .versions()
            .map(|version| version.to_be_bytes().to_vec())
            .collect();
        let unsupported = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM email_registration_challenges
                 WHERE substring(canonical_email_digest FROM 1 FOR 4) <> ALL($1)
                 UNION ALL
                 SELECT 1 FROM email_registration_reservations
                 WHERE substring(canonical_email_digest FROM 1 FOR 4) <> ALL($1)
                 UNION ALL
                 SELECT 1 FROM email_registration_identifier_capacity
                 WHERE substring(canonical_email_digest FROM 1 FOR 4) <> ALL($1)
                 UNION ALL
                 SELECT 1 FROM email_registration_delivery_limits
                 WHERE substring(canonical_email_digest FROM 1 FOR 4) <> ALL($1)
             )",
        )
        .bind(&supported_versions)
        .fetch_one(&mut **tx)
        .await?;
        if unsupported {
            return Err(AppError::service_unavailable(
                "email verification key rotation incomplete",
            ));
        }
        Ok(())
    }

    async fn claim_canonical_delivery_slot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        current_digest: &[u8; VERSIONED_DIGEST_BYTES],
        candidates: &[Vec<u8>],
    ) -> Result<(bool, DateTime<Utc>), AppError> {
        let now = Utc::now();
        sqlx::query(
            "DELETE FROM email_registration_delivery_limits
             WHERE canonical_email_digest = ANY($1) AND expires_at <= now()",
        )
        .bind(candidates)
        .execute(&mut **tx)
        .await?;
        let existing = sqlx::query(
            "SELECT canonical_email_digest, delivery_count, last_delivery_at, expires_at
             FROM email_registration_delivery_limits
             WHERE canonical_email_digest = ANY($1)
             ORDER BY canonical_email_digest
             LIMIT 1
             FOR UPDATE",
        )
        .bind(candidates)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(existing) = existing {
            let count: i16 = existing.try_get("delivery_count")?;
            let last_delivery_at: DateTime<Utc> = existing.try_get("last_delivery_at")?;
            let window_expires_at: DateTime<Utc> = existing.try_get("expires_at")?;
            if count >= MAX_DELIVERIES_PER_CANONICAL_WINDOW {
                return Ok((false, window_expires_at));
            }
            let cooldown_expires_at =
                last_delivery_at + Duration::seconds(DELIVERY_COOLDOWN_SECONDS);
            if cooldown_expires_at > now {
                return Ok((false, cooldown_expires_at));
            }
            let digest: Vec<u8> = existing.try_get("canonical_email_digest")?;
            sqlx::query(
                "UPDATE email_registration_delivery_limits
                 SET delivery_count = delivery_count + 1,
                     last_delivery_at = now(), updated_at = now()
                 WHERE canonical_email_digest = $1",
            )
            .bind(digest)
            .execute(&mut **tx)
            .await?;
            return Ok((true, now + Duration::seconds(DELIVERY_COOLDOWN_SECONDS)));
        }
        sqlx::query(
            "INSERT INTO email_registration_delivery_limits
                (canonical_email_digest, delivery_count, last_delivery_at, expires_at)
             VALUES ($1, 1, now(), now() + make_interval(mins => $2))",
        )
        .bind(current_digest.as_slice())
        .bind(i32::try_from(DELIVERY_WINDOW_MINUTES).map_err(|_| AppError::internal())?)
        .execute(&mut **tx)
        .await?;
        Ok((true, now + Duration::seconds(DELIVERY_COOLDOWN_SECONDS)))
    }

    async fn identifier_capacity_available(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        candidates: &[Vec<u8>],
    ) -> Result<bool, AppError> {
        let active_count = sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(sum(active_count), 0)
             FROM email_registration_identifier_capacity
             WHERE canonical_email_digest = ANY($1)",
        )
        .bind(candidates)
        .fetch_one(&mut **tx)
        .await?;
        Ok(active_count < 4)
    }

    fn token_digest(
        &self,
        purpose: &[u8],
        token: &str,
    ) -> Result<[u8; VERSIONED_DIGEST_BYTES], AppError> {
        let (version, secret) = parse_versioned_secret(token)?;
        self.token_keys
            .digest_for(version, purpose, &secret)
            .ok_or_else(registration_unavailable)
    }

    fn generate_token(&self, purpose: &[u8]) -> (String, [u8; VERSIONED_DIGEST_BYTES]) {
        let mut secret = [0u8; TOKEN_BYTES];
        OsRng.fill_bytes(&mut secret);
        let version = self.token_keys.current.version;
        let token = format!("{version}.{}", URL_SAFE_NO_PAD.encode(secret));
        let digest = self
            .token_keys
            .digest_for(version, purpose, &secret)
            .expect("current key version is always present");
        (token, digest)
    }

    fn generate_otp(
        &self,
        challenge_id: Uuid,
        generation: i32,
    ) -> (String, [u8; VERSIONED_DIGEST_BYTES]) {
        let otp = if self.deterministic_otp_for_tests {
            format!("{generation:08}")
        } else {
            // Rejection sampling avoids modulo bias over the 100,000,000
            // possible eight-digit confirmation codes.
            let zone = u32::MAX - (u32::MAX % 100_000_000);
            let value = loop {
                let candidate = OsRng.next_u32();
                if candidate < zone {
                    break candidate % 100_000_000;
                }
            };
            format!("{value:08}")
        };
        let purpose = otp_digest_purpose(challenge_id, generation);
        let digest = self
            .token_keys
            .digest_for(self.token_keys.current.version, &purpose, otp.as_bytes())
            .expect("current key version is always present");
        (otp, digest)
    }

    fn verify_otp_digest(
        &self,
        challenge_id: Uuid,
        generation: i32,
        otp: &str,
        stored_digest: &[u8],
    ) -> bool {
        if otp.len() != 8 || !otp.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Some(version_bytes) = stored_digest.get(..4) else {
            return false;
        };
        let Ok(version_bytes) = <[u8; 4]>::try_from(version_bytes) else {
            return false;
        };
        let version = u32::from_be_bytes(version_bytes);
        let purpose = otp_digest_purpose(challenge_id, generation);
        self.token_keys
            .digest_for(version, &purpose, otp.as_bytes())
            .is_some_and(|digest| constant_time_equal(&digest, stored_digest))
    }

    fn seal_json(
        &self,
        purpose: &[u8],
        binding: &[u8],
        value: &impl Serialize,
    ) -> Result<Vec<u8>, AppError> {
        let plaintext = serde_json::to_vec(value).map_err(|_| AppError::internal())?;
        self.state_keys.seal(purpose, binding, &plaintext)
    }

    fn open_json<T: for<'de> Deserialize<'de>>(
        &self,
        purpose: &[u8],
        binding: &[u8],
        ciphertext: &[u8],
    ) -> Result<T, AppError> {
        let plaintext = self.state_keys.open(purpose, binding, ciphertext)?;
        serde_json::from_slice(&plaintext).map_err(|_| AppError::internal())
    }

    pub async fn request_registration(
        &self,
        pool: &PgPool,
        request: RegistrationRequest,
        idempotency_key: &str,
    ) -> Result<RegistrationRequestResponse, AppError> {
        let idempotency_key_digest = validate_and_hash_idempotency_key(idempotency_key)?;
        let request_hash = registration_request_hash(&request)?;
        let mut tx = pool.begin().await?;
        self.ensure_token_rotation_supported(&mut tx).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended(encode($1::bytea, 'hex'), 0)
             )",
        )
        .bind(idempotency_key_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        if let Some(row) = sqlx::query(
            "SELECT challenge_id, request_hash, response_ciphertext
             FROM registration_request_idempotency
             WHERE idempotency_key_digest = $1
               AND purpose = 'email_registration_request'
               AND expires_at > now()",
        )
        .bind(idempotency_key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            let challenge_id: Uuid = row.try_get("challenge_id")?;
            let stored_request_hash: Vec<u8> = row.try_get("request_hash")?;
            if !constant_time_equal(&stored_request_hash, &request_hash) {
                return Err(registration_unavailable());
            }
            let ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
            let plaintext = self.state_keys.open(
                b"taskveil/email/register-request-response/v1\0",
                &request_response_binding(challenge_id, &idempotency_key_digest, &request_hash),
                &ciphertext,
            )?;
            let response = serde_json::from_slice(&plaintext).map_err(|_| AppError::internal())?;
            tx.commit().await?;
            return Ok(response);
        }
        let email = canonicalize_email(&request.email)?;
        let handoff = decode_handoff_challenge(&request.handoff_challenge)?;
        let request_id = Uuid::now_v7();
        let opaque_credential_id = Uuid::now_v7();
        let now = Utc::now();
        let otp_expires_at = now + Duration::minutes(OTP_TTL_MINUTES);
        let expires_at = now + Duration::minutes(REGISTRATION_REQUEST_TTL_MINUTES);
        let (otp, otp_digest) = self.generate_otp(request_id, 1);
        let canonical_digest = self.canonical_digest(email.canonical());
        let canonical_digest_candidates = self.canonical_digest_candidates(email.canonical());
        let encrypted_registration = self.seal_json(
            b"taskveil/email/registration/v1\0",
            request_id.as_bytes(),
            &EncryptedRegistration {
                display_email: email.display().to_string(),
                canonical_email: email.canonical().to_string(),
                delivery_recipient: email.canonical().to_string(),
            },
        )?;
        let delivery =
            self.delivery_command(request_id, 1, otp_expires_at, email.canonical(), &otp)?;
        let encrypted_command = self.seal_json(
            b"taskveil/email/outbox-command/v1\0",
            &delivery_binding(request_id, 1),
            &delivery,
        )?;

        self.lock_canonical_candidates(&mut tx, &canonical_digest_candidates)
            .await?;
        let capacity_claimed = self
            .identifier_capacity_available(&mut tx, &canonical_digest_candidates)
            .await?;
        let (enqueue_delivery, next_retry_at) = if capacity_claimed {
            self.claim_canonical_delivery_slot(
                &mut tx,
                &canonical_digest,
                &canonical_digest_candidates,
            )
            .await?
        } else {
            (false, now + Duration::seconds(DELIVERY_COOLDOWN_SECONDS))
        };
        sqlx::query(
            "INSERT INTO email_registration_challenges
                (id, canonical_email_digest, encrypted_registration, handoff_challenge,
                 opaque_credential_id, capacity_claimed, generation, last_delivery_at,
                 next_retry_at, otp_digest, otp_expires_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, 1,
                     CASE WHEN $7 THEN now() ELSE NULL END, $8, $9, $10, $11)",
        )
        .bind(request_id)
        .bind(canonical_digest.as_slice())
        .bind(encrypted_registration)
        .bind(handoff.as_slice())
        .bind(opaque_credential_id)
        .bind(capacity_claimed)
        .bind(enqueue_delivery)
        .bind(next_retry_at)
        .bind(otp_digest.as_slice())
        .bind(otp_expires_at)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(map_email_capacity_error)?;
        if enqueue_delivery {
            sqlx::query(
                "INSERT INTO email_delivery_outbox
                    (challenge_id, generation, encrypted_command, not_after)
                 VALUES ($1, 1, $2, $3)",
            )
            .bind(request_id)
            .bind(encrypted_command)
            .bind(otp_expires_at)
            .execute(&mut *tx)
            .await?;
        }
        let response = RegistrationRequestResponse {
            request_id,
            expires_at: otp_expires_at,
            next_retry_at,
        };
        let response_bytes = serde_json::to_vec(&response).map_err(|_| AppError::internal())?;
        let response_ciphertext = self.state_keys.seal(
            b"taskveil/email/register-request-response/v1\0",
            &request_response_binding(request_id, &idempotency_key_digest, &request_hash),
            &response_bytes,
        )?;
        sqlx::query(
            "INSERT INTO registration_request_idempotency
                (challenge_id, purpose, idempotency_key_digest, request_hash,
                 response_ciphertext, expires_at)
             VALUES ($1, 'email_registration_request', $2, $3, $4, $5)",
        )
        .bind(request_id)
        .bind(idempotency_key_digest.as_slice())
        .bind(request_hash.as_slice())
        .bind(response_ciphertext)
        .bind(Utc::now() + Duration::minutes(REGISTRATION_REQUEST_IDEMPOTENCY_TTL_MINUTES))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(response)
    }

    pub async fn cleanup_expired_registration_state(&self, pool: &PgPool) -> Result<u64, AppError> {
        let removed = sqlx::query(
            "WITH expired AS (
                 SELECT id FROM email_registration_challenges
                 WHERE expires_at <= now()
                 ORDER BY expires_at, id
                 LIMIT $1
             )
             DELETE FROM email_registration_challenges
             USING expired
             WHERE email_registration_challenges.id = expired.id",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?
        .rows_affected();
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, purpose, idempotency_key_digest
                 FROM registration_request_idempotency
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_request_idempotency
             USING expired
             WHERE registration_request_idempotency.challenge_id = expired.challenge_id
               AND registration_request_idempotency.purpose = expired.purpose
               AND registration_request_idempotency.idempotency_key_digest =
                   expired.idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, idempotency_key_digest
                 FROM registration_start_idempotency
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_start_idempotency
             USING expired
             WHERE registration_start_idempotency.challenge_id = expired.challenge_id
               AND registration_start_idempotency.idempotency_key_digest =
                   expired.idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, idempotency_key_digest
                 FROM registration_resend_idempotency
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_resend_idempotency
             USING expired
             WHERE registration_resend_idempotency.challenge_id = expired.challenge_id
               AND registration_resend_idempotency.idempotency_key_digest =
                   expired.idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, idempotency_key_digest
                 FROM registration_verify_idempotency
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_verify_idempotency
             USING expired
             WHERE registration_verify_idempotency.challenge_id = expired.challenge_id
               AND registration_verify_idempotency.idempotency_key_digest =
                   expired.idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, idempotency_key_digest
                 FROM registration_finish_idempotency
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_finish_idempotency
             USING expired
             WHERE registration_finish_idempotency.challenge_id = expired.challenge_id
               AND registration_finish_idempotency.idempotency_key_digest =
                   expired.idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id, finish_idempotency_key_digest
                 FROM registration_reconciliation_receipts
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_reconciliation_receipts
             USING expired
             WHERE registration_reconciliation_receipts.challenge_id = expired.challenge_id
               AND registration_reconciliation_receipts.finish_idempotency_key_digest =
                   expired.finish_idempotency_key_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT challenge_id
                 FROM registration_reconciliation_authorizations
                 WHERE expires_at <= now()
                 ORDER BY expires_at, challenge_id
                 LIMIT $1
             )
             DELETE FROM registration_reconciliation_authorizations
             USING expired
             WHERE registration_reconciliation_authorizations.challenge_id = expired.challenge_id",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        sqlx::query(
            "WITH expired AS (
                 SELECT canonical_email_digest
                 FROM email_registration_delivery_limits
                 WHERE expires_at <= now()
                 ORDER BY expires_at, canonical_email_digest
                 LIMIT $1
             )
             DELETE FROM email_registration_delivery_limits
             USING expired
             WHERE email_registration_delivery_limits.canonical_email_digest =
                   expired.canonical_email_digest",
        )
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(pool)
        .await?;
        Ok(removed)
    }

    pub async fn resend_registration(
        &self,
        pool: &PgPool,
        request: RegistrationResendRequest,
        idempotency_key: &str,
    ) -> Result<RegistrationRequestResponse, AppError> {
        let idempotency_key_digest = validate_and_hash_idempotency_key(idempotency_key)?;
        let request_hash = registration_resend_hash(&request)?;
        let handoff_secret = decode_handoff_secret(&request.handoff_secret)?;
        let supplied_handoff = handoff_challenge(&handoff_secret);
        let mut tx = pool.begin().await?;
        self.ensure_token_rotation_supported(&mut tx).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended(encode($1::bytea, 'hex'), 2)
             )",
        )
        .bind(idempotency_key_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        if let Some(response) = self
            .registration_resend_replay(
                &mut tx,
                request.request_id,
                &idempotency_key_digest,
                &request_hash,
            )
            .await?
        {
            tx.commit().await?;
            return Ok(response);
        }
        let row = sqlx::query(
            "SELECT canonical_email_digest, encrypted_registration,
                    handoff_challenge, generation,
                    resend_count, last_delivery_at, next_retry_at,
                    otp_expires_at, expires_at, capacity_claimed
             FROM email_registration_challenges
             WHERE id = $1 AND expires_at > now() AND verified_at IS NULL
               AND handoff_failed_attempts < $2
             FOR UPDATE",
        )
        .bind(request.request_id)
        .bind(MAX_HANDOFF_ATTEMPTS)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(registration_unavailable)?;
        let stored_handoff: Vec<u8> = row.try_get("handoff_challenge")?;
        if !constant_time_equal(&stored_handoff, &supplied_handoff) {
            sqlx::query(
                "UPDATE email_registration_challenges
                 SET handoff_failed_attempts = handoff_failed_attempts + 1,
                     updated_at = now()
                 WHERE id = $1 AND handoff_failed_attempts < $2",
            )
            .bind(request.request_id)
            .bind(MAX_HANDOFF_ATTEMPTS)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Err(registration_unavailable());
        }
        let encrypted_registration: Vec<u8> = row.try_get("encrypted_registration")?;
        let registration: EncryptedRegistration = self.open_json(
            b"taskveil/email/registration/v1\0",
            request.request_id.as_bytes(),
            &encrypted_registration,
        )?;
        let current_otp_expires_at: DateTime<Utc> = row.try_get("otp_expires_at")?;
        let challenge_expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        let stored_next_retry_at: DateTime<Utc> = row.try_get("next_retry_at")?;
        let resend_count: i16 = row.try_get("resend_count")?;
        let last_delivery_at: Option<DateTime<Utc>> = row.try_get("last_delivery_at")?;
        let mut capacity_claimed: bool = row.try_get("capacity_claimed")?;
        let canonical_digest = self.canonical_digest(&registration.canonical_email);
        let canonical_digest_candidates =
            self.canonical_digest_candidates(&registration.canonical_email);
        self.lock_canonical_candidates(&mut tx, &canonical_digest_candidates)
            .await?;
        if !capacity_claimed
            && self
                .identifier_capacity_available(&mut tx, &canonical_digest_candidates)
                .await?
        {
            capacity_claimed = sqlx::query_scalar::<_, bool>(
                "SELECT taskveil_promote_email_registration_capacity($1)",
            )
            // The SECURITY DEFINER function accepts only a real challenge ID,
            // locks that row, and promotes its stored digest exactly once.
            .bind(request.request_id)
            .fetch_one(&mut *tx)
            .await?;
        }
        let now = Utc::now();
        let challenge_allows_delivery = resend_count < MAX_RESENDS_PER_CHALLENGE
            && stored_next_retry_at <= now
            && last_delivery_at
                .is_none_or(|last| last + Duration::seconds(DELIVERY_COOLDOWN_SECONDS) <= now);
        let (canonical_allows_delivery, next_retry_at) =
            if challenge_allows_delivery && capacity_claimed {
                self.claim_canonical_delivery_slot(
                    &mut tx,
                    &canonical_digest,
                    &canonical_digest_candidates,
                )
                .await?
            } else {
                let retry_at = if challenge_allows_delivery {
                    stored_next_retry_at.max(now + Duration::seconds(DELIVERY_COOLDOWN_SECONDS))
                } else {
                    stored_next_retry_at
                };
                (false, retry_at)
            };
        if !challenge_allows_delivery || !canonical_allows_delivery {
            sqlx::query(
                "UPDATE email_registration_challenges
                 SET capacity_claimed = $2, next_retry_at = $3, updated_at = now()
                 WHERE id = $1",
            )
            .bind(request.request_id)
            .bind(capacity_claimed)
            .bind(next_retry_at)
            .execute(&mut *tx)
            .await?;
            let response = RegistrationRequestResponse {
                request_id: request.request_id,
                expires_at: current_otp_expires_at.min(challenge_expires_at),
                next_retry_at: next_retry_at.min(challenge_expires_at),
            };
            self.store_registration_resend_response(
                &mut tx,
                &response,
                &idempotency_key_digest,
                &request_hash,
            )
            .await?;
            tx.commit().await?;
            return Ok(response);
        }
        let generation: i32 = row.try_get("generation")?;
        let generation = generation.checked_add(1).ok_or_else(AppError::internal)?;
        let otp_expires_at = (now + Duration::minutes(OTP_TTL_MINUTES)).min(challenge_expires_at);
        let (otp, otp_digest) = self.generate_otp(request.request_id, generation);
        let delivery = self.delivery_command(
            request.request_id,
            generation,
            otp_expires_at,
            &registration.delivery_recipient,
            &otp,
        )?;
        let encrypted_command = self.seal_json(
            b"taskveil/email/outbox-command/v1\0",
            &delivery_binding(request.request_id, generation),
            &delivery,
        )?;
        sqlx::query(
            "UPDATE email_registration_challenges
             SET capacity_claimed = TRUE, generation = $2,
                 otp_digest = $3, otp_expires_at = $4, next_retry_at = $5,
                 resend_count = resend_count + 1, last_delivery_at = now(),
                 handoff_failed_attempts = 0, otp_failed_attempts = 0,
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(request.request_id)
        .bind(generation)
        .bind(otp_digest.as_slice())
        .bind(otp_expires_at)
        .bind(next_retry_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO email_delivery_outbox
                (challenge_id, generation, encrypted_command, not_after)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(request.request_id)
        .bind(generation)
        .bind(encrypted_command)
        .bind(otp_expires_at)
        .execute(&mut *tx)
        .await?;
        let response = RegistrationRequestResponse {
            request_id: request.request_id,
            expires_at: otp_expires_at,
            next_retry_at: next_retry_at.min(challenge_expires_at),
        };
        self.store_registration_resend_response(
            &mut tx,
            &response,
            &idempotency_key_digest,
            &request_hash,
        )
        .await?;
        tx.commit().await?;
        Ok(response)
    }

    async fn registration_resend_replay(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        challenge_id: Uuid,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
    ) -> Result<Option<RegistrationRequestResponse>, AppError> {
        let row = sqlx::query(
            "SELECT challenge_id, request_hash, response_ciphertext
             FROM registration_resend_idempotency
             WHERE idempotency_key_digest = $1
               AND purpose = 'email_registration_resend'
               AND expires_at > now()",
        )
        .bind(idempotency_key_digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored_challenge_id: Uuid = row.try_get("challenge_id")?;
        let stored_request_hash: Vec<u8> = row.try_get("request_hash")?;
        if stored_challenge_id != challenge_id
            || !constant_time_equal(&stored_request_hash, request_hash)
        {
            return Err(registration_unavailable());
        }
        let ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
        let plaintext = self.state_keys.open(
            b"taskveil/email/register-resend-response/v1\0",
            &resend_response_binding(challenge_id, idempotency_key_digest, request_hash),
            &ciphertext,
        )?;
        serde_json::from_slice(&plaintext)
            .map(Some)
            .map_err(|_| AppError::internal())
    }

    async fn store_registration_resend_response(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        response: &RegistrationRequestResponse,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
    ) -> Result<(), AppError> {
        let plaintext = serde_json::to_vec(response).map_err(|_| AppError::internal())?;
        let ciphertext = self.state_keys.seal(
            b"taskveil/email/register-resend-response/v1\0",
            &resend_response_binding(response.request_id, idempotency_key_digest, request_hash),
            &plaintext,
        )?;
        sqlx::query(
            "INSERT INTO registration_resend_idempotency
                (idempotency_key_digest, challenge_id, purpose, request_hash,
                 response_ciphertext, expires_at)
             VALUES ($1, $2, 'email_registration_resend', $3, $4, $5)",
        )
        .bind(idempotency_key_digest.as_slice())
        .bind(response.request_id)
        .bind(request_hash.as_slice())
        .bind(&ciphertext)
        .bind(Utc::now() + Duration::minutes(REGISTRATION_REQUEST_IDEMPOTENCY_TTL_MINUTES))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn verify_registration(
        &self,
        pool: &PgPool,
        request: RegistrationVerifyRequest,
        idempotency_key: &str,
    ) -> Result<RegistrationVerifyResponse, AppError> {
        let idempotency_key_digest = validate_and_hash_idempotency_key(idempotency_key)?;
        let request_hash = registration_verify_hash(&request)?;
        let handoff_secret = decode_handoff_secret(&request.handoff_secret)?;
        let supplied_handoff = handoff_challenge(&handoff_secret);
        let mut tx = pool.begin().await?;
        self.ensure_token_rotation_supported(&mut tx).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended(encode($1::bytea, 'hex'), 3)
             )",
        )
        .bind(idempotency_key_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        if let Some(row) = sqlx::query(
            "SELECT challenge_id, request_hash, response_ciphertext
             FROM registration_verify_idempotency
             WHERE idempotency_key_digest = $1
               AND purpose = 'email_registration_verify'
               AND expires_at > now()",
        )
        .bind(idempotency_key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored_challenge_id: Uuid = row.try_get("challenge_id")?;
            let stored_request_hash: Vec<u8> = row.try_get("request_hash")?;
            if stored_challenge_id != request.request_id
                || !constant_time_equal(&stored_request_hash, &request_hash)
            {
                return Err(registration_unavailable());
            }
            let ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
            let plaintext = self.state_keys.open(
                b"taskveil/email/register-verify-response/v1\0",
                &request_response_binding(
                    request.request_id,
                    &idempotency_key_digest,
                    &request_hash,
                ),
                &ciphertext,
            )?;
            let outcome: StoredRegistrationVerifyOutcome =
                serde_json::from_slice(&plaintext).map_err(|_| AppError::internal())?;
            tx.commit().await?;
            return match outcome {
                StoredRegistrationVerifyOutcome::Accepted { response } => Ok(response),
                StoredRegistrationVerifyOutcome::Rejected => Err(registration_unavailable()),
            };
        }
        let row = sqlx::query(
            "SELECT canonical_email_digest, encrypted_registration,
                    handoff_challenge, generation, otp_digest, expires_at
             FROM email_registration_challenges
             WHERE id = $1 AND otp_expires_at > now()
               AND expires_at > now() AND verified_at IS NULL
               AND handoff_failed_attempts < $2
               AND otp_failed_attempts < $3
             FOR UPDATE",
        )
        .bind(request.request_id)
        .bind(MAX_HANDOFF_ATTEMPTS)
        .bind(MAX_OTP_ATTEMPTS)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(registration_unavailable)?;
        let challenge_expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        let stored_handoff: Vec<u8> = row.try_get("handoff_challenge")?;
        if !constant_time_equal(&stored_handoff, &supplied_handoff) {
            sqlx::query(
                "UPDATE email_registration_challenges
                 SET handoff_failed_attempts = handoff_failed_attempts + 1,
                     updated_at = now()
                 WHERE id = $1 AND handoff_failed_attempts < $2",
            )
            .bind(request.request_id)
            .bind(MAX_HANDOFF_ATTEMPTS)
            .execute(&mut *tx)
            .await?;
            self.store_registration_verify_outcome(
                &mut tx,
                request.request_id,
                &idempotency_key_digest,
                &request_hash,
                &StoredRegistrationVerifyOutcome::Rejected,
                challenge_expires_at,
            )
            .await?;
            tx.commit().await?;
            return Err(registration_unavailable());
        }
        let generation: i32 = row.try_get("generation")?;
        let stored_otp_digest: Vec<u8> = row.try_get("otp_digest")?;
        if !self.verify_otp_digest(
            request.request_id,
            generation,
            &request.otp,
            &stored_otp_digest,
        ) {
            sqlx::query(
                "UPDATE email_registration_challenges
                 SET otp_failed_attempts = otp_failed_attempts + 1,
                     updated_at = now()
                 WHERE id = $1 AND otp_failed_attempts < $2",
            )
            .bind(request.request_id)
            .bind(MAX_OTP_ATTEMPTS)
            .execute(&mut *tx)
            .await?;
            self.store_registration_verify_outcome(
                &mut tx,
                request.request_id,
                &idempotency_key_digest,
                &request_hash,
                &StoredRegistrationVerifyOutcome::Rejected,
                challenge_expires_at,
            )
            .await?;
            tx.commit().await?;
            return Err(registration_unavailable());
        }
        let challenge_id = request.request_id;
        let canonical_digest: Vec<u8> = row.try_get("canonical_email_digest")?;
        let encrypted_registration: Vec<u8> = row.try_get("encrypted_registration")?;
        let registration: EncryptedRegistration = self.open_json(
            b"taskveil/email/registration/v1\0",
            challenge_id.as_bytes(),
            &encrypted_registration,
        )?;
        let canonical_digest_candidates =
            self.canonical_digest_candidates(&registration.canonical_email);
        self.lock_canonical_candidates(&mut tx, &canonical_digest_candidates)
            .await?;
        sqlx::query(
            "DELETE FROM email_registration_reservations
             WHERE canonical_email_digest = ANY($1) AND expires_at <= now()",
        )
        .bind(&canonical_digest_candidates)
        .execute(&mut *tx)
        .await?;
        let account_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM users WHERE canonical_email = $1
             )",
        )
        .bind(&registration.canonical_email)
        .fetch_one(&mut *tx)
        .await?;
        let ticket_expires_at = (Utc::now() + Duration::minutes(REGISTRATION_TICKET_TTL_MINUTES))
            .min(challenge_expires_at);
        let owns_reservation = sqlx::query(
            "INSERT INTO email_registration_reservations
                    (canonical_email_digest, challenge_id, expires_at)
                 SELECT $1, $2, $3
                 WHERE NOT $5
                   AND NOT EXISTS (
                     SELECT 1 FROM email_registration_reservations
                     WHERE canonical_email_digest = ANY($4)
                 )
                 ON CONFLICT (canonical_email_digest) DO NOTHING",
        )
        .bind(&canonical_digest)
        .bind(challenge_id)
        .bind(ticket_expires_at)
        .bind(&canonical_digest_candidates)
        .bind(account_exists)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let is_decoy = !owns_reservation || account_exists;
        let (ticket, ticket_digest) =
            self.generate_token(b"taskveil/email/registration-ticket/v1\0");
        let ticket_ciphertext = self.seal_json(
            b"taskveil/email/registration-ticket/v1\0",
            challenge_id.as_bytes(),
            &ticket,
        )?;
        let updated = sqlx::query(
            "UPDATE email_registration_challenges
             SET verified_at = now(), is_decoy = $2,
                 registration_ticket_ciphertext = $3,
                 registration_ticket_digest = $4, ticket_expires_at = $5,
                 updated_at = now()
             WHERE id = $1 AND verified_at IS NULL",
        )
        .bind(challenge_id)
        .bind(is_decoy)
        .bind(ticket_ciphertext)
        .bind(ticket_digest.as_slice())
        .bind(ticket_expires_at)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(registration_unavailable());
        }
        let response = RegistrationVerifyResponse {
            registration_ticket: ticket,
            expires_at: ticket_expires_at,
        };
        let outcome = StoredRegistrationVerifyOutcome::Accepted { response };
        self.store_registration_verify_outcome(
            &mut tx,
            challenge_id,
            &idempotency_key_digest,
            &request_hash,
            &outcome,
            ticket_expires_at,
        )
        .await?;
        tx.commit().await?;
        match outcome {
            StoredRegistrationVerifyOutcome::Accepted { response } => Ok(response),
            StoredRegistrationVerifyOutcome::Rejected => unreachable!(),
        }
    }

    async fn store_registration_verify_outcome(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        challenge_id: Uuid,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
        outcome: &StoredRegistrationVerifyOutcome,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let plaintext = serde_json::to_vec(outcome).map_err(|_| AppError::internal())?;
        let ciphertext = self.state_keys.seal(
            b"taskveil/email/register-verify-response/v1\0",
            &request_response_binding(challenge_id, idempotency_key_digest, request_hash),
            &plaintext,
        )?;
        sqlx::query(
            "INSERT INTO registration_verify_idempotency
                (challenge_id, purpose, idempotency_key_digest, request_hash,
                 response_ciphertext, expires_at)
             VALUES ($1, 'email_registration_verify', $2, $3, $4, $5)",
        )
        .bind(challenge_id)
        .bind(idempotency_key_digest.as_slice())
        .bind(request_hash.as_slice())
        .bind(ciphertext)
        .bind(expires_at)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn claim_registration_ticket(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ticket: &str,
    ) -> Result<VerifiedRegistration, AppError> {
        let ticket_digest =
            self.token_digest(b"taskveil/email/registration-ticket/v1\0", ticket)?;
        let row = sqlx::query(
            "SELECT id, opaque_credential_id, encrypted_registration,
                    canonical_email_digest, is_decoy
             FROM email_registration_challenges
             WHERE registration_ticket_digest = $1
               AND verified_at IS NOT NULL AND ticket_expires_at > now()
               AND ticket_consumed_at IS NULL AND expires_at > now()
             FOR UPDATE",
        )
        .bind(ticket_digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(registration_unavailable)?;
        let challenge_id: Uuid = row.try_get("id")?;
        let is_decoy: bool = row.try_get("is_decoy")?;
        let canonical_digest: Vec<u8> = row.try_get("canonical_email_digest")?;
        let owns_reservation = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM email_registration_reservations
                WHERE canonical_email_digest = $1 AND challenge_id = $2
                  AND expires_at > now()
             )",
        )
        .bind(&canonical_digest)
        .bind(challenge_id)
        .fetch_one(&mut **tx)
        .await?;
        let encrypted_registration: Vec<u8> = row.try_get("encrypted_registration")?;
        let registration: EncryptedRegistration = self.open_json(
            b"taskveil/email/registration/v1\0",
            challenge_id.as_bytes(),
            &encrypted_registration,
        )?;
        let account_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE canonical_email = $1)",
        )
        .bind(&registration.canonical_email)
        .fetch_one(&mut **tx)
        .await?;
        let is_decoy = is_decoy || !owns_reservation || account_exists;
        let consumed = sqlx::query(
            "UPDATE email_registration_challenges
             SET ticket_consumed_at = now(), is_decoy = $2, updated_at = now()
             WHERE id = $1 AND ticket_consumed_at IS NULL",
        )
        .bind(challenge_id)
        .bind(is_decoy)
        .execute(&mut **tx)
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(registration_unavailable());
        }
        Ok(VerifiedRegistration {
            challenge_id,
            opaque_credential_id: row.try_get("opaque_credential_id")?,
            display_email: registration.display_email,
            canonical_email: registration.canonical_email,
            is_decoy,
        })
    }

    pub async fn registration_start_replay(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ticket: &str,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
    ) -> Result<Option<Vec<u8>>, AppError> {
        let ticket_digest =
            self.token_digest(b"taskveil/email/registration-ticket/v1\0", ticket)?;
        let row = sqlx::query(
            "SELECT c.id, i.idempotency_key_digest, i.request_hash, i.response_ciphertext
             FROM email_registration_challenges c
             JOIN registration_start_idempotency i ON i.challenge_id = c.id
             WHERE c.registration_ticket_digest = $1
               AND i.purpose = 'account_registration' AND i.expires_at > now()",
        )
        .bind(ticket_digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let challenge_id: Uuid = row.try_get("id")?;
        let stored_key: Vec<u8> = row.try_get("idempotency_key_digest")?;
        let stored_request: Vec<u8> = row.try_get("request_hash")?;
        if !constant_time_equal(&stored_key, idempotency_key_digest)
            || !constant_time_equal(&stored_request, request_hash)
        {
            return Err(registration_unavailable());
        }
        let response_ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
        self.state_keys
            .open(
                b"taskveil/email/register-start-response/v1\0",
                &start_response_binding(challenge_id, idempotency_key_digest, request_hash),
                &response_ciphertext,
            )
            .map(Some)
    }

    pub async fn store_registration_start_response(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        challenge_id: Uuid,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
        response: &[u8],
        replay_expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let ciphertext = self.state_keys.seal(
            b"taskveil/email/register-start-response/v1\0",
            &start_response_binding(challenge_id, idempotency_key_digest, request_hash),
            response,
        )?;
        sqlx::query(
            "INSERT INTO registration_start_idempotency
                (challenge_id, purpose, idempotency_key_digest, request_hash,
                 response_ciphertext, expires_at)
             VALUES ($1, 'account_registration', $2, $3, $4, $5)",
        )
        .bind(challenge_id)
        .bind(idempotency_key_digest.as_slice())
        .bind(request_hash.as_slice())
        .bind(&ciphertext)
        .bind(replay_expires_at)
        .execute(&mut **tx)
        .await?;
        let handoff_challenge = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT handoff_challenge
             FROM email_registration_challenges
             WHERE id = $1",
        )
        .bind(challenge_id)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO registration_reconciliation_authorizations
                (challenge_id, handoff_challenge,
                 start_idempotency_key_digest, expires_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (challenge_id) DO NOTHING",
        )
        .bind(challenge_id)
        .bind(handoff_challenge)
        .bind(idempotency_key_digest.as_slice())
        .bind(Utc::now() + Duration::hours(REGISTRATION_RECONCILE_TTL_HOURS))
        .execute(&mut **tx)
        .await?;
        let post_start_expiry = Utc::now() + Duration::minutes(REGISTRATION_POST_START_TTL_MINUTES);
        sqlx::query(
            "UPDATE email_registration_challenges
             SET expires_at = GREATEST(expires_at, $2)
             WHERE id = $1",
        )
        .bind(challenge_id)
        .bind(post_start_expiry)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE email_registration_reservations
             SET expires_at = GREATEST(expires_at, $2)
             WHERE challenge_id = $1",
        )
        .bind(challenge_id)
        .bind(post_start_expiry)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn registration_finish_replay(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
    ) -> Result<Option<Vec<u8>>, AppError> {
        // Serialize first execution and response-loss retries before observing
        // either the OPAQUE state or the replay row. This closes the
        // commit/visibility race between two concurrent identical finishes.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended(encode($1::bytea, 'hex'), 1)
             )",
        )
        .bind(idempotency_key_digest.as_slice())
        .execute(&mut **tx)
        .await?;
        let row = sqlx::query(
            "SELECT state_id, idempotency_key_digest, request_hash, response_ciphertext
             FROM registration_finish_idempotency
             WHERE idempotency_key_digest = $1
               AND purpose = 'account_registration_finish'
               AND expires_at > now()",
        )
        .bind(idempotency_key_digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let state_id: Uuid = row.try_get("state_id")?;
        let stored_key: Vec<u8> = row.try_get("idempotency_key_digest")?;
        let stored_request: Vec<u8> = row.try_get("request_hash")?;
        if !constant_time_equal(&stored_key, idempotency_key_digest)
            || !constant_time_equal(&stored_request, request_hash)
        {
            return Err(registration_unavailable());
        }
        let response_ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
        self.state_keys
            .open(
                b"taskveil/email/register-finish-response/v1\0",
                &finish_response_binding(state_id, idempotency_key_digest, request_hash),
                &response_ciphertext,
            )
            .map(Some)
    }

    pub async fn store_registration_finish_response(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        state_id: Uuid,
        challenge_id: Uuid,
        idempotency_key_digest: &[u8; 32],
        request_hash: &[u8; 32],
        response: &[u8],
    ) -> Result<(), AppError> {
        let ciphertext = self.state_keys.seal(
            b"taskveil/email/register-finish-response/v1\0",
            &finish_response_binding(state_id, idempotency_key_digest, request_hash),
            response,
        )?;
        sqlx::query(
            "INSERT INTO registration_finish_idempotency
                (state_id, challenge_id, purpose, idempotency_key_digest,
                 request_hash, response_ciphertext, expires_at)
             VALUES ($1, $2, 'account_registration_finish', $3, $4, $5, $6)",
        )
        .bind(state_id)
        .bind(challenge_id)
        .bind(idempotency_key_digest.as_slice())
        .bind(request_hash.as_slice())
        .bind(&ciphertext)
        .bind(Utc::now() + Duration::minutes(REGISTRATION_FINISH_IDEMPOTENCY_TTL_MINUTES))
        .execute(&mut **tx)
        .await?;
        let handoff_challenge = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT handoff_challenge
             FROM email_registration_challenges
             WHERE id = $1",
        )
        .bind(challenge_id)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO registration_reconciliation_receipts
                (challenge_id, handoff_challenge, state_id,
                 finish_idempotency_key_digest, request_hash,
                 response_ciphertext, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(challenge_id)
        .bind(handoff_challenge)
        .bind(state_id)
        .bind(idempotency_key_digest.as_slice())
        .bind(request_hash.as_slice())
        .bind(ciphertext)
        .bind(Utc::now() + Duration::hours(REGISTRATION_RECONCILE_TTL_HOURS))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn registration_status(
        &self,
        pool: &PgPool,
        request: RegistrationStatusRequest,
    ) -> Result<RegistrationStatusResponse, AppError> {
        let supplied_handoff = handoff_challenge(&decode_handoff_secret(&request.handoff_secret)?);
        let start_idempotency_key_digest =
            validate_and_hash_idempotency_key(&request.start_idempotency_key)?;
        let idempotency_key_digest =
            validate_and_hash_idempotency_key(&request.finish_idempotency_key)?;
        let mut tx = pool.begin().await?;
        // The finish path acquires this lock before checking or mutating any
        // registration state. Waiting on the same key prevents a status read
        // from reporting "pending" while that exact finish is committing.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended(encode($1::bytea, 'hex'), 1)
             )",
        )
        .bind(idempotency_key_digest.as_slice())
        .execute(&mut *tx)
        .await?;
        let authorization = sqlx::query(
            "SELECT handoff_challenge, start_idempotency_key_digest
             FROM registration_reconciliation_authorizations
             WHERE challenge_id = $1 AND expires_at > now()",
        )
        .bind(request.request_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(registration_unavailable)?;
        let stored_handoff: Vec<u8> = authorization.try_get("handoff_challenge")?;
        let stored_start_key: Vec<u8> = authorization.try_get("start_idempotency_key_digest")?;
        if !constant_time_equal(&stored_handoff, &supplied_handoff)
            || !constant_time_equal(&stored_start_key, &start_idempotency_key_digest)
        {
            return Err(registration_unavailable());
        }
        if let Some(row) = sqlx::query(
            "SELECT state_id, request_hash, response_ciphertext
             FROM registration_reconciliation_receipts
             WHERE challenge_id = $1 AND finish_idempotency_key_digest = $2
               AND expires_at > now()",
        )
        .bind(request.request_id)
        .bind(idempotency_key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            let state_id: Uuid = row.try_get("state_id")?;
            let request_hash: Vec<u8> = row.try_get("request_hash")?;
            let request_hash: [u8; 32] =
                request_hash.try_into().map_err(|_| AppError::internal())?;
            let ciphertext: Vec<u8> = row.try_get("response_ciphertext")?;
            let plaintext = self.state_keys.open(
                b"taskveil/email/register-finish-response/v1\0",
                &finish_response_binding(state_id, &idempotency_key_digest, &request_hash),
                &ciphertext,
            )?;
            let result = serde_json::from_slice(&plaintext).map_err(|_| AppError::internal())?;
            tx.commit().await?;
            return Ok(RegistrationStatusResponse {
                status: "committed",
                result: Some(result),
            });
        }
        tx.commit().await?;
        Ok(RegistrationStatusResponse {
            status: "pending",
            result: None,
        })
    }

    pub fn open_registration_identity(
        &self,
        challenge_id: Uuid,
        ciphertext: &[u8],
    ) -> Result<(String, String), AppError> {
        let registration: EncryptedRegistration = self.open_json(
            b"taskveil/email/registration/v1\0",
            challenge_id.as_bytes(),
            ciphertext,
        )?;
        Ok((registration.display_email, registration.canonical_email))
    }

    fn delivery_command(
        &self,
        challenge_id: Uuid,
        generation: i32,
        not_after: DateTime<Utc>,
        recipient: &str,
        otp: &str,
    ) -> Result<EmailDeliveryCommand, AppError> {
        let payload = EmailDeliveryPayload {
            recipient: recipient.to_string(),
            otp: otp.to_string(),
            template: "verify-email-otp-v1".to_string(),
        };
        let binding = delivery_binding(challenge_id, generation);
        let plaintext = serde_json::to_vec(&payload).map_err(|_| AppError::internal())?;
        let encrypted_payload = self.delivery_keys.seal(
            b"taskveil/email/delivery-payload/v1\0",
            &binding,
            &plaintext,
        )?;
        Ok(EmailDeliveryCommand {
            version: 1,
            delivery_id: format!("{challenge_id}:{generation}"),
            not_after,
            encrypted_payload: URL_SAFE_NO_PAD.encode(encrypted_payload),
        })
    }

    pub async fn dispatch_email_batch(
        &self,
        pool: &PgPool,
    ) -> Result<EmailDispatchSummary, AppError> {
        // The authenticated scheduled dispatcher is the bounded GC clock.
        // Never put cleanup work on the unauthenticated registration hot path.
        self.cleanup_expired_registration_state(pool).await?;
        let deliveries = self.claim_email_deliveries(pool).await?;
        let mut summary = EmailDispatchSummary {
            claimed: deliveries.len(),
            accepted: 0,
            retryable: 0,
            terminal: 0,
        };
        // API Gateway and Lambda have a 30-second request ceiling while each
        // provider attempt has a 10-second timeout. Run the bounded DB batch
        // concurrently; sequential delivery could exceed the invocation
        // ceiling and leave every later claim waiting for its lease to expire.
        let mut attempts = tokio::task::JoinSet::new();
        for delivery in deliveries {
            let gateway = self.delivery.clone();
            attempts.spawn(async move {
                let outcome = gateway.send(&delivery.command).await;
                (delivery, outcome)
            });
        }
        while let Some(attempt) = attempts.join_next().await {
            let (delivery, outcome) = attempt.map_err(|_| AppError::internal())?;
            self.settle_email_delivery(pool, &delivery, outcome).await?;
            match outcome {
                DeliveryOutcome::Accepted => summary.accepted += 1,
                DeliveryOutcome::Retryable { .. } => summary.retryable += 1,
                DeliveryOutcome::Terminal => summary.terminal += 1,
            }
        }
        Ok(summary)
    }

    pub fn authorize_dispatch(&self, bearer: &str) -> bool {
        STANDARD.decode(bearer).ok().is_some_and(|supplied| {
            constant_time_equal(&supplied, self.dispatch_trigger_key.as_ref())
        })
    }

    async fn claim_email_deliveries(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<ClaimedEmailDelivery>, AppError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(
            "SELECT challenge_id, generation, encrypted_command
             FROM email_delivery_outbox
             WHERE accepted_at IS NULL AND terminal_at IS NULL
               AND encrypted_command IS NOT NULL
               AND not_after > now() AND attempt_count < $1
               AND available_at <= now()
               AND (claim_expires_at IS NULL OR claim_expires_at <= now())
             ORDER BY available_at, challenge_id, generation
             LIMIT $2
             FOR UPDATE SKIP LOCKED",
        )
        .bind(EMAIL_DISPATCH_MAX_ATTEMPTS)
        .bind(EMAIL_DISPATCH_BATCH_SIZE)
        .fetch_all(&mut *tx)
        .await?;
        let mut deliveries = Vec::with_capacity(rows.len());
        for row in rows {
            let challenge_id: Uuid = row.try_get("challenge_id")?;
            let generation: i32 = row.try_get("generation")?;
            let encrypted_command: Vec<u8> = row.try_get("encrypted_command")?;
            let Ok(command) = self.open_json(
                b"taskveil/email/outbox-command/v1\0",
                &delivery_binding(challenge_id, generation),
                &encrypted_command,
            ) else {
                // One corrupt or temporarily undecryptable command must not
                // poison the entire SKIP LOCKED batch. Retry it with the same
                // bounded policy, then crypto-shred it at the attempt limit.
                sqlx::query(
                    "UPDATE email_delivery_outbox
                     SET attempt_count = attempt_count + 1,
                         available_at = now() + make_interval(
                             secs => least(900, power(2, attempt_count + 1)::integer)
                         ),
                         terminal_at = CASE
                             WHEN attempt_count + 1 >= $3 THEN now()
                             ELSE terminal_at
                         END,
                         encrypted_command = CASE
                             WHEN attempt_count + 1 >= $3 THEN NULL
                             ELSE encrypted_command
                         END,
                         updated_at = now()
                     WHERE challenge_id = $1 AND generation = $2",
                )
                .bind(challenge_id)
                .bind(generation)
                .bind(EMAIL_DISPATCH_MAX_ATTEMPTS)
                .execute(&mut *tx)
                .await?;
                continue;
            };
            let claim_id = Uuid::now_v7();
            sqlx::query(
                "UPDATE email_delivery_outbox
                 SET claim_id = $3,
                     claim_expires_at = now() + make_interval(secs => $4),
                     attempt_count = attempt_count + 1, updated_at = now()
                 WHERE challenge_id = $1 AND generation = $2",
            )
            .bind(challenge_id)
            .bind(generation)
            .bind(claim_id)
            .bind(f64::from(EMAIL_DISPATCH_CLAIM_SECONDS as i32))
            .execute(&mut *tx)
            .await?;
            deliveries.push(ClaimedEmailDelivery {
                challenge_id,
                generation,
                claim_id,
                command,
            });
        }
        sqlx::query(
            "WITH terminal AS (
                 SELECT challenge_id, generation
                 FROM email_delivery_outbox
                 WHERE accepted_at IS NULL AND terminal_at IS NULL
                   AND (not_after <= now() OR attempt_count >= $1)
                 ORDER BY not_after, challenge_id, generation
                 LIMIT $2
             )
             UPDATE email_delivery_outbox
             SET terminal_at = now(), encrypted_command = NULL,
                 claim_id = NULL, claim_expires_at = NULL, updated_at = now()
             FROM terminal
             WHERE email_delivery_outbox.challenge_id = terminal.challenge_id
               AND email_delivery_outbox.generation = terminal.generation",
        )
        .bind(EMAIL_DISPATCH_MAX_ATTEMPTS)
        .bind(EMAIL_REGISTRATION_GC_BATCH_SIZE)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(deliveries)
    }

    async fn settle_email_delivery(
        &self,
        pool: &PgPool,
        delivery: &ClaimedEmailDelivery,
        outcome: DeliveryOutcome,
    ) -> Result<(), AppError> {
        match outcome {
            DeliveryOutcome::Accepted => {
                sqlx::query(
                    "UPDATE email_delivery_outbox
                     SET accepted_at = now(), encrypted_command = NULL,
                         claim_id = NULL, claim_expires_at = NULL, updated_at = now()
                     WHERE challenge_id = $1 AND generation = $2 AND claim_id = $3",
                )
                .bind(delivery.challenge_id)
                .bind(delivery.generation)
                .bind(delivery.claim_id)
                .execute(pool)
                .await?;
            }
            DeliveryOutcome::Terminal => {
                sqlx::query(
                    "UPDATE email_delivery_outbox
                     SET terminal_at = now(), encrypted_command = NULL,
                         claim_id = NULL, claim_expires_at = NULL, updated_at = now()
                     WHERE challenge_id = $1 AND generation = $2 AND claim_id = $3",
                )
                .bind(delivery.challenge_id)
                .bind(delivery.generation)
                .bind(delivery.claim_id)
                .execute(pool)
                .await?;
            }
            DeliveryOutcome::Retryable {
                retry_at,
                consume_attempt,
            } => {
                sqlx::query(
                    "UPDATE email_delivery_outbox
                     SET attempt_count = CASE
                             WHEN $4 THEN attempt_count
                             ELSE greatest(0, attempt_count - 1)
                         END,
                         available_at = least(
                             not_after,
                             coalesce(
                                 $5,
                                 now() + make_interval(
                                     secs => least(
                                         900,
                                         power(2, attempt_count)::integer
                                     )
                                 )
                             )
                         ),
                         claim_id = NULL, claim_expires_at = NULL, updated_at = now()
                     WHERE challenge_id = $1 AND generation = $2 AND claim_id = $3",
                )
                .bind(delivery.challenge_id)
                .bind(delivery.generation)
                .bind(delivery.claim_id)
                .bind(consume_attempt)
                .bind(retry_at)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

impl EmailDeliveryGateway {
    async fn send(&self, command: &EmailDeliveryCommand) -> DeliveryOutcome {
        let Ok(body) = serde_json::to_vec(command) else {
            return DeliveryOutcome::Terminal;
        };
        let timestamp = Utc::now().timestamp();
        let signing_input = email_ingress_signing_input(
            "POST",
            self.endpoint.path(),
            &self.signing_key_id,
            timestamp,
            &body,
        );
        let mut mac =
            <HmacSha256 as Mac>::new_from_slice(self.signing_key.as_ref()).expect("fixed key size");
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let result = self
            .http
            .post(self.endpoint.clone())
            .header("content-type", "application/json")
            .header("x-taskveil-key-id", self.signing_key_id.as_ref())
            .header("x-taskveil-timestamp", timestamp.to_string())
            .header("x-taskveil-signature", signature)
            .body(body)
            .send()
            .await;
        match result {
            Ok(response) if response.status() == StatusCode::ACCEPTED => DeliveryOutcome::Accepted,
            Ok(response)
                if response.status() == StatusCode::TOO_MANY_REQUESTS
                    || response.status().is_server_error() =>
            {
                let retry_at = parse_retry_after(response.headers().get("retry-after"));
                DeliveryOutcome::Retryable {
                    retry_at,
                    consume_attempt: retry_at.is_none(),
                }
            }
            Ok(_) => DeliveryOutcome::Terminal,
            Err(_) => DeliveryOutcome::Retryable {
                retry_at: None,
                consume_attempt: true,
            },
        }
    }
}

impl VersionedKeyring {
    fn new(
        current_version: u32,
        current_key: [u8; 32],
        previous: Option<(u32, [u8; 32])>,
    ) -> Result<Self, &'static str> {
        if current_version == 0
            || previous
                .as_ref()
                .is_some_and(|(version, _)| *version == 0 || *version == current_version)
        {
            return Err("invalid email verification key version");
        }
        Ok(Self {
            current: VersionedKey {
                version: current_version,
                key: Arc::new(current_key),
            },
            previous: previous.map(|(version, key)| VersionedKey {
                version,
                key: Arc::new(key),
            }),
        })
    }

    fn key(&self, version: u32) -> Option<&[u8; 32]> {
        if self.current.version == version {
            Some(&self.current.key)
        } else {
            self.previous
                .as_ref()
                .filter(|key| key.version == version)
                .map(|key| key.key.as_ref())
        }
    }

    fn all_keys(&self) -> impl Iterator<Item = &[u8; 32]> {
        std::iter::once(self.current.key.as_ref())
            .chain(self.previous.as_ref().map(|key| key.key.as_ref()))
    }

    fn versions(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.current.version).chain(self.previous.as_ref().map(|key| key.version))
    }

    fn digest(&self, purpose: &[u8], value: &[u8]) -> [u8; VERSIONED_DIGEST_BYTES] {
        self.digest_for(self.current.version, purpose, value)
            .expect("current key version is always present")
    }

    fn digest_for(
        &self,
        version: u32,
        purpose: &[u8],
        value: &[u8],
    ) -> Option<[u8; VERSIONED_DIGEST_BYTES]> {
        let key = self.key(version)?;
        let mut mac = <HmacSha256 as Mac>::new_from_slice(key).ok()?;
        mac.update(purpose);
        mac.update(value);
        let mut result = [0u8; VERSIONED_DIGEST_BYTES];
        result[..4].copy_from_slice(&version.to_be_bytes());
        result[4..].copy_from_slice(&mac.finalize().into_bytes());
        Some(result)
    }

    fn seal(&self, purpose: &[u8], binding: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
        let version = self.current.version;
        let cipher = Aes256Gcm::new_from_slice(self.current.key.as_ref())
            .map_err(|_| AppError::internal())?;
        let mut nonce = [0u8; SEALED_NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = encryption_aad(purpose, version, binding);
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| AppError::internal())?;
        let mut result =
            Vec::with_capacity(SEALED_MAGIC.len() + 4 + SEALED_NONCE_BYTES + sealed.len());
        result.extend_from_slice(SEALED_MAGIC);
        result.extend_from_slice(&version.to_be_bytes());
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&sealed);
        Ok(result)
    }

    fn open(&self, purpose: &[u8], binding: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, AppError> {
        if ciphertext.len() < SEALED_MAGIC.len() + 4 + SEALED_NONCE_BYTES + 16
            || &ciphertext[..4] != SEALED_MAGIC
        {
            return Err(AppError::internal());
        }
        let version = u32::from_be_bytes(
            ciphertext[4..8]
                .try_into()
                .map_err(|_| AppError::internal())?,
        );
        let key = self.key(version).ok_or_else(AppError::internal)?;
        let nonce = &ciphertext[8..8 + SEALED_NONCE_BYTES];
        let sealed = &ciphertext[8 + SEALED_NONCE_BYTES..];
        let aad = encryption_aad(purpose, version, binding);
        Aes256Gcm::new_from_slice(key)
            .map_err(|_| AppError::internal())?
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| AppError::internal())
    }
}

pub fn canonicalize_email(value: &str) -> Result<CanonicalEmail, AppError> {
    let display = value.trim();
    if display.is_empty()
        || display.len() > 254
        || display
            .chars()
            .any(|character| character.is_control() || character == '\r' || character == '\n')
    {
        return Err(AppError::bad_request("invalid email"));
    }
    let mut parts = display.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || local.is_empty()
        || local.len() > 64
        || !local.is_ascii()
        || domain.is_empty()
        || domain.ends_with('.')
        || domain.split('.').any(str::is_empty)
        || !valid_dot_atom(local)
    {
        return Err(AppError::bad_request("invalid email"));
    }
    let ascii_domain = domain_to_ascii_cow(domain.as_bytes(), AsciiDenyList::URL)
        .map_err(|_| AppError::bad_request("invalid email"))?
        .to_ascii_lowercase();
    if ascii_domain.is_empty()
        || ascii_domain.len() > 253
        || ascii_domain.ends_with('.')
        || ascii_domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(AppError::bad_request("invalid email"));
    }
    let canonical = format!("{local}@{ascii_domain}");
    if canonical.len() > 254 {
        return Err(AppError::bad_request("invalid email"));
    }
    Ok(CanonicalEmail {
        display: display.to_string(),
        canonical,
    })
}

fn valid_dot_atom(local: &str) -> bool {
    !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                        | b'.'
                )
        })
}

fn parse_versioned_secret(value: &str) -> Result<(u32, [u8; TOKEN_BYTES]), AppError> {
    let (version, secret) = value.split_once('.').ok_or_else(registration_unavailable)?;
    let version = version
        .parse::<u32>()
        .ok()
        .filter(|parsed| *parsed > 0 && parsed.to_string() == version)
        .ok_or_else(registration_unavailable)?;
    let secret = URL_SAFE_NO_PAD
        .decode(secret)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(registration_unavailable)?;
    Ok((version, secret))
}

fn otp_digest_purpose(challenge_id: Uuid, generation: i32) -> Vec<u8> {
    let mut purpose = Vec::with_capacity(55);
    purpose.extend_from_slice(b"taskveil/email/verification-otp/v1\0");
    purpose.extend_from_slice(challenge_id.as_bytes());
    purpose.extend_from_slice(&generation.to_be_bytes());
    purpose
}

fn parse_retry_after(value: Option<&axum::http::HeaderValue>) -> Option<DateTime<Utc>> {
    let value = value?.to_str().ok()?.trim();
    let now = Utc::now();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.parse::<i64>().ok()?.clamp(1, 86_400);
        return Some(now + Duration::seconds(seconds));
    }
    let parsed = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(parsed.clamp(now + Duration::seconds(1), now + Duration::seconds(86_400)))
}

fn decode_handoff_secret(value: &str) -> Result<[u8; HANDOFF_BYTES], AppError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(registration_unavailable)
}

fn decode_handoff_challenge(value: &str) -> Result<[u8; HANDOFF_BYTES], AppError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| AppError::bad_request("invalid handoff challenge"))
}

fn handoff_challenge(secret: &[u8; HANDOFF_BYTES]) -> [u8; HANDOFF_BYTES] {
    Sha256::digest(secret).into()
}

fn validate_and_hash_idempotency_key(value: &str) -> Result<[u8; 32], AppError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::bad_request("invalid idempotency key"));
    }
    Ok(Sha256::digest(value.as_bytes()).into())
}

fn registration_request_hash(request: &RegistrationRequest) -> Result<[u8; 32], AppError> {
    let encoded = serde_json::to_vec(request).map_err(|_| AppError::internal())?;
    let mut digest = Sha256::new();
    digest.update(b"taskveil/email/register-request/v1\0");
    digest.update((encoded.len() as u64).to_be_bytes());
    digest.update(encoded);
    Ok(digest.finalize().into())
}

fn registration_resend_hash(request: &RegistrationResendRequest) -> Result<[u8; 32], AppError> {
    let encoded = serde_json::to_vec(request).map_err(|_| AppError::internal())?;
    let mut digest = Sha256::new();
    digest.update(b"taskveil/email/register-resend/v1\0");
    digest.update((encoded.len() as u64).to_be_bytes());
    digest.update(encoded);
    Ok(digest.finalize().into())
}

fn registration_verify_hash(request: &RegistrationVerifyRequest) -> Result<[u8; 32], AppError> {
    let encoded = serde_json::to_vec(request).map_err(|_| AppError::internal())?;
    let mut digest = Sha256::new();
    digest.update(b"taskveil/email/register-verify/v1\0");
    digest.update((encoded.len() as u64).to_be_bytes());
    digest.update(encoded);
    Ok(digest.finalize().into())
}

fn request_response_binding(
    challenge_id: Uuid,
    idempotency_key_digest: &[u8; 32],
    request_hash: &[u8; 32],
) -> Vec<u8> {
    let mut binding = Vec::with_capacity(80);
    binding.extend_from_slice(challenge_id.as_bytes());
    binding.extend_from_slice(idempotency_key_digest);
    binding.extend_from_slice(request_hash);
    binding
}

fn resend_response_binding(
    challenge_id: Uuid,
    idempotency_key_digest: &[u8; 32],
    request_hash: &[u8; 32],
) -> Vec<u8> {
    let mut binding = Vec::with_capacity(80);
    binding.extend_from_slice(challenge_id.as_bytes());
    binding.extend_from_slice(idempotency_key_digest);
    binding.extend_from_slice(request_hash);
    binding
}

fn delivery_binding(challenge_id: Uuid, generation: i32) -> Vec<u8> {
    let mut binding = Vec::with_capacity(20);
    binding.extend_from_slice(challenge_id.as_bytes());
    binding.extend_from_slice(&generation.to_be_bytes());
    binding
}

fn start_response_binding(
    challenge_id: Uuid,
    idempotency_key_digest: &[u8; 32],
    request_hash: &[u8; 32],
) -> Vec<u8> {
    let mut binding = Vec::with_capacity(80);
    binding.extend_from_slice(challenge_id.as_bytes());
    binding.extend_from_slice(idempotency_key_digest);
    binding.extend_from_slice(request_hash);
    binding
}

fn finish_response_binding(
    state_id: Uuid,
    idempotency_key_digest: &[u8; 32],
    request_hash: &[u8; 32],
) -> Vec<u8> {
    let mut binding = Vec::with_capacity(80);
    binding.extend_from_slice(state_id.as_bytes());
    binding.extend_from_slice(idempotency_key_digest);
    binding.extend_from_slice(request_hash);
    binding
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn encryption_aad(purpose: &[u8], version: u32, binding: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(40 + purpose.len() + binding.len());
    aad.extend_from_slice(b"taskveil/email-verification/aead/v1\0");
    aad.extend_from_slice(&version.to_be_bytes());
    aad.extend_from_slice(purpose);
    aad.push(0);
    aad.extend_from_slice(binding);
    aad
}

fn email_ingress_signing_input(
    method: &str,
    path: &str,
    key_id: &str,
    timestamp: i64,
    body: &[u8],
) -> String {
    let body_digest = URL_SAFE_NO_PAD.encode(Sha256::digest(body));
    format!("{method}\n{path}\n{key_id}\n{timestamp}\n{body_digest}")
}

fn secure_endpoint_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let loopback_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if (url.scheme() != "https" && !loopback_http)
        || url.query().is_some()
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
    {
        return None;
    }
    Some(url)
}

fn registration_unavailable() -> AppError {
    AppError::bad_request("registration unavailable")
}

fn map_email_capacity_error(error: sqlx_core::Error) -> AppError {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .is_some_and(|code| code == "P0429")
    {
        AppError::rate_limited(None)
    } else {
        error.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_email_preserves_local_part_and_normalizes_idna_domain() {
        let email = canonicalize_email(" Case.Sensitive+tag@BÜCHER.example ").unwrap();
        assert_eq!(email.display(), "Case.Sensitive+tag@BÜCHER.example");
        assert_eq!(
            email.canonical(),
            "Case.Sensitive+tag@xn--bcher-kva.example"
        );
        assert_ne!(
            email.canonical(),
            canonicalize_email("case.sensitive+tag@bücher.example")
                .unwrap()
                .canonical()
        );
    }

    #[test]
    fn canonical_email_rejects_ambiguous_or_unsupported_forms() {
        for invalid in [
            "",
            "display <user@example.com>",
            "\"quoted\"@example.com",
            "user@[127.0.0.1]",
            "user@example.com.",
            "user@example..com",
            "user@@example.com",
            "üser@example.com",
            "user\n@example.com",
            ".user@example.com",
            "user..name@example.com",
        ] {
            assert!(canonicalize_email(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn versioned_secrets_and_ciphertexts_are_bound_and_rotatable() {
        let service = EmailVerificationService::for_tests();
        let (token, digest) = service.generate_token(b"verification");
        assert_eq!(
            service.token_digest(b"verification", &token).unwrap(),
            digest
        );
        assert_ne!(service.token_digest(b"different", &token).unwrap(), digest);

        let sealed = service
            .seal_json(b"registration", b"one", &serde_json::json!({"ok":true}))
            .unwrap();
        let opened: serde_json::Value =
            service.open_json(b"registration", b"one", &sealed).unwrap();
        assert_eq!(opened, serde_json::json!({"ok":true}));
        assert!(service
            .open_json::<serde_json::Value>(b"registration", b"two", &sealed)
            .is_err());

        let old = VersionedKeyring::new(1, [0x11; 32], None).unwrap();
        let rotated = VersionedKeyring::new(2, [0x22; 32], Some((1, [0x11; 32]))).unwrap();
        let old_digest = old.digest_for(1, b"token", b"value").unwrap();
        assert_eq!(
            rotated.digest_for(1, b"token", b"value").unwrap(),
            old_digest
        );
        let old_ciphertext = old.seal(b"purpose", b"binding", b"plaintext").unwrap();
        assert_eq!(
            rotated
                .open(b"purpose", b"binding", &old_ciphertext)
                .unwrap(),
            b"plaintext"
        );
    }

    #[test]
    fn delivery_recipient_and_ingress_signature_match_the_worker_contract() {
        let service = EmailVerificationService::for_tests();
        let email = canonicalize_email("Case.Sensitive@BÜCHER.example").unwrap();
        let command = service
            .delivery_command(
                Uuid::parse_str("019f9d8a-e8ca-7031-afb0-c11ab745caa9").unwrap(),
                1,
                Utc::now() + Duration::minutes(1),
                email.canonical(),
                "12345678",
            )
            .unwrap();
        let sealed = URL_SAFE_NO_PAD.decode(command.encrypted_payload).unwrap();
        let plaintext = service
            .delivery_keys
            .open(
                b"taskveil/email/delivery-payload/v1\0",
                &delivery_binding(
                    Uuid::parse_str("019f9d8a-e8ca-7031-afb0-c11ab745caa9").unwrap(),
                    1,
                ),
                &sealed,
            )
            .unwrap();
        let payload: EmailDeliveryPayload = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(payload.recipient, "Case.Sensitive@xn--bcher-kva.example");
        assert_eq!(payload.otp, "12345678");
        assert_eq!(payload.template, "verify-email-otp-v1");

        let input =
            email_ingress_signing_input("POST", "/v1/enqueue", "sign-v1", 1_785_024_000, b"{}");
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&[0x03; 32]).unwrap();
        mac.update(input.as_bytes());
        assert_eq!(
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
            "FKG_nYAP0pQJvfRPngo1Ewlg9t0sfyEf0hregqPoLSo"
        );
    }

    #[test]
    fn every_email_key_domain_must_be_independent() {
        let config = || EmailVerificationConfig {
            token_key_current_version: 1,
            token_key_current: [0x11; 32],
            token_key_previous: Some((0x10, [0x10; 32])),
            state_key_current_version: 1,
            state_key_current: [0x22; 32],
            state_key_previous: Some((0x20, [0x20; 32])),
            delivery_key_current_version: 1,
            delivery_key_current: [0x32; 32],
            delivery_key_previous: Some((0x30, [0x30; 32])),
            delivery_endpoint: "http://127.0.0.1:1/v1/enqueue".to_string(),
            delivery_signing_key_id: "test-v1".to_string(),
            delivery_signing_key: [0x43; 32],
            dispatch_trigger_key: [0x44; 32],
        };
        for mutate in [
            |value: &mut EmailVerificationConfig| value.state_key_current = [0x11; 32],
            |value: &mut EmailVerificationConfig| {
                value.delivery_key_previous = Some((2, [0x20; 32]))
            },
            |value: &mut EmailVerificationConfig| value.delivery_signing_key = [0x32; 32],
            |value: &mut EmailVerificationConfig| value.dispatch_trigger_key = [0x43; 32],
        ] {
            let mut duplicated = config();
            mutate(&mut duplicated);
            assert!(EmailVerificationService::new(duplicated).is_err());
        }
        assert!(EmailVerificationService::new(config()).is_ok());

        let old_state = VersionedKeyring::new(1, [0x51; 32], None).unwrap();
        let old_delivery = VersionedKeyring::new(7, [0x61; 32], None).unwrap();
        let state_ciphertext = old_state.seal(b"state", b"id", b"state").unwrap();
        let delivery_ciphertext = old_delivery.seal(b"delivery", b"id", b"delivery").unwrap();
        let rotated_state = VersionedKeyring::new(2, [0x52; 32], Some((1, [0x51; 32]))).unwrap();
        let rotated_delivery = VersionedKeyring::new(8, [0x62; 32], Some((7, [0x61; 32]))).unwrap();
        assert_eq!(
            rotated_state
                .open(b"state", b"id", &state_ciphertext)
                .unwrap(),
            b"state"
        );
        assert_eq!(
            rotated_delivery
                .open(b"delivery", b"id", &delivery_ciphertext)
                .unwrap(),
            b"delivery"
        );
        assert!(rotated_state
            .open(b"delivery", b"id", &delivery_ciphertext)
            .is_err());
    }

    #[test]
    fn retry_after_accepts_delta_and_http_date_with_bounds() {
        let delta = axum::http::HeaderValue::from_static("120");
        let parsed = parse_retry_after(Some(&delta)).unwrap();
        let remaining = parsed - Utc::now();
        assert!(remaining >= Duration::seconds(118));
        assert!(remaining <= Duration::seconds(120));

        let date = axum::http::HeaderValue::from_str(
            &(Utc::now() + Duration::hours(48))
                .format("%a, %d %b %Y %H:%M:%S GMT")
                .to_string(),
        )
        .unwrap();
        let parsed = parse_retry_after(Some(&date)).unwrap();
        assert!(parsed <= Utc::now() + Duration::seconds(86_400));

        let invalid = axum::http::HeaderValue::from_static("soon");
        assert!(parse_retry_after(Some(&invalid)).is_none());
    }
}
