use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx_postgres::{PgPool, PgTransaction};
use taskveil_crypto::{
    key_hierarchy::INITIAL_KEY_GENERATION,
    organization::{
        verify_device_certificate, verify_device_proof, AccountRootPublicKeys, DeviceCertificate,
        DeviceProofOfPossession, DEVICE_CHALLENGE_LEN, DEVICE_FINGERPRINT_LEN,
        ED25519_SIGNATURE_LEN,
    },
    TaskveilCipherSuite, CRYPTO_SUITE_ID,
};
use taskveil_protocol::account::{
    AccountKeyBundleDto, DeviceEnrollmentDto, UpdateKeyWrappersRequest,
};
use uuid::Uuid;

use crate::{db, AppError};

const OPAQUE_STATE_TTL_MINUTES: i64 = 10;
const ACCESS_TOKEN_TTL_MINUTES: i64 = 15;
const REFRESH_TOKEN_IDLE_TTL_DAYS: i64 = 30;
const SESSION_FAMILY_TTL_DAYS: i64 = 90;
const AUTH_GC_ACCESS_TOKEN_BATCH_SIZE: i64 = 128;
const AUTH_GC_REFRESH_TOKEN_BATCH_SIZE: i64 = 128;
const AUTH_GC_SESSION_FAMILY_BATCH_SIZE: i64 = 16;
const AUTH_GC_PENDING_DEVICE_BATCH_SIZE: i64 = 16;
const AUTH_GC_OPAQUE_STATE_BATCH_SIZE: i64 = 128;
const MAX_ACTIVE_OPAQUE_STATES: i32 = 4096;
const MAX_ACTIVE_OPAQUE_STATES_PER_IDENTIFIER: i32 = 32;
pub const NATIVE_CLIENT_ID: &str = "taskveil-native";

type TaskveilServerSetup = ServerSetup<TaskveilCipherSuite>;
type TaskveilServerRegistration = ServerRegistration<TaskveilCipherSuite>;
type TaskveilServerLogin = ServerLogin<TaskveilCipherSuite>;

#[derive(Debug, Deserialize)]
pub struct OpaqueStartRequest {
    pub email: String,
    pub device_name: Option<String>,
    pub opaque_suite_id: u16,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegistrationStartResponse {
    pub state_id: Uuid,
    pub opaque_suite_id: u16,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Uuid,
    pub device_challenge: String,
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginStartResponse {
    pub state_id: Uuid,
    pub opaque_suite_id: u16,
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct OpaqueFinishRequest {
    pub state_id: Uuid,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterFinishRequest {
    pub state_id: Uuid,
    pub message: String,
    pub key_bundle: AccountKeyBundleDto,
    pub device_enrollment: DeviceEnrollmentDto,
}

#[derive(Debug, Deserialize)]
pub struct LoginFinishRequest {
    pub state_id: Uuid,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct SessionResponse {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Uuid,
    #[serde(flatten)]
    pub tokens: TokenResponse,
}

#[derive(Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_token_expires_in: u64,
    pub refresh_expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub refresh_token: String,
    pub client_id: String,
}

#[derive(Deserialize)]
pub struct RevocationRequest {
    pub token: String,
    pub token_type_hint: Option<String>,
    pub client_id: String,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub token_endpoint: String,
    pub revocation_endpoint: String,
    pub grant_types_supported: Vec<&'static str>,
    pub token_endpoint_auth_methods_supported: Vec<&'static str>,
    pub revocation_endpoint_auth_methods_supported: Vec<&'static str>,
}

#[derive(Serialize, Deserialize)]
pub struct LoginSessionResponse {
    #[serde(flatten)]
    pub session: SessionResponse,
    pub key_bundle: AccountKeyBundleDto,
    pub device_challenge: String,
    pub device_challenge_expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogoutResponse {}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub device_id: Uuid,
}

pub async fn register_start(
    pool: &PgPool,
    request: OpaqueStartRequest,
    identifier_key: &[u8; 32],
) -> Result<RegistrationStartResponse, AppError> {
    validate_opaque_suite(request.opaque_suite_id)?;
    let email = normalize_email(&request.email)?;
    let device_name = normalize_device_name(request.device_name);
    let client_message = decode_opaque_message(&request.message)?;
    let registration_request =
        RegistrationRequest::<TaskveilCipherSuite>::deserialize(&client_message)
            .map_err(|_| AppError::bad_request("invalid opaque message"))?;
    cleanup_expired_opaque_states(pool).await?;
    ensure_opaque_state_capacity_available(pool, identifier_key).await?;
    let server_setup = get_or_create_server_setup(pool).await?;
    let server_start =
        ServerRegistration::start(&server_setup, registration_request, email.as_bytes())
            .map_err(|_| AppError::bad_request("invalid opaque message"))?;
    let state_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let tenant_id = Uuid::now_v7();
    let device_id = Uuid::now_v7();
    let device_challenge = random_device_challenge();
    let expires_at = Utc::now() + Duration::minutes(OPAQUE_STATE_TTL_MINUTES);

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO opaque_registration_states
            (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
             opaque_suite_id, expires_at, identifier_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(state_id)
    .bind(user_id)
    .bind(tenant_id)
    .bind(device_id)
    .bind(device_challenge.as_slice())
    .bind(&email)
    .bind(&device_name)
    .bind(i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?)
    .bind(expires_at)
    .bind(identifier_key.as_slice())
    .execute(&mut *tx)
    .await
    .map_err(map_opaque_state_insert_error)?;
    tx.commit().await?;

    Ok(RegistrationStartResponse {
        state_id,
        opaque_suite_id: CRYPTO_SUITE_ID,
        user_id,
        tenant_id,
        device_id,
        device_challenge: STANDARD.encode(device_challenge),
        message: STANDARD.encode(server_start.message.serialize()),
        expires_at,
    })
}

pub async fn register_finish(
    pool: &PgPool,
    request: RegisterFinishRequest,
) -> Result<SessionResponse, AppError> {
    let upload = decode_opaque_message(&request.message)?;
    let registration_upload = RegistrationUpload::<TaskveilCipherSuite>::deserialize(&upload)
        .map_err(|_| AppError::bad_request("invalid opaque message"))?;
    let server_record = ServerRegistration::finish(registration_upload);
    let server_record_bytes = server_record.serialize().to_vec();
    let key_bundle = decode_account_key_bundle(&request.key_bundle)?;

    let mut tx = pool.begin().await?;
    let state = consume_registration_state(&mut tx, request.state_id).await?;
    let user_id = state.user_id;
    let tenant_id = state.tenant_id;
    let device_id = state.device_id;
    let enrollment = verify_device_enrollment(
        &request.device_enrollment,
        user_id,
        device_id,
        &state.device_challenge,
        Utc::now().timestamp_millis(),
    )?;
    if key_bundle.account_root_public != enrollment.account_root_public {
        return Err(AppError::bad_request("account root mismatch"));
    }

    sqlx::query!(
        "INSERT INTO users
            (id, email, opaque_suite_id, opaque_record, account_root_public)
         VALUES ($1, $2, $3, $4, $5)",
        user_id,
        &state.email,
        i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?,
        &server_record_bytes,
        &enrollment.account_root_public,
    )
    .execute(&mut *tx)
    .await
    .map_err(map_insert_user_error)?;

    sqlx::query!(
        "INSERT INTO billing_customers (user_id) VALUES ($1)",
        user_id
    )
    .execute(&mut *tx)
    .await?;

    db::set_user_context(&mut tx, user_id).await?;
    db::set_tenant_context(&mut tx, tenant_id).await?;

    sqlx::query!(
        "INSERT INTO tenants (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
        tenant_id,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO tenant_members
            (tenant_id, user_id, role, verification_state, verified_at)
         VALUES ($1, $2, 'owner', 'verified', now())",
        tenant_id,
        user_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO tenant_seq (tenant_id, last_seq) VALUES ($1, 0)",
        tenant_id
    )
    .execute(&mut *tx)
    .await?;
    insert_account_key_bundle(&mut tx, user_id, tenant_id, key_bundle).await?;
    insert_certified_device(&mut tx, device_id, user_id, &state.device_name, &enrollment).await?;
    let tokens = create_session(&mut tx, user_id, device_id).await?;
    tx.commit().await?;

    Ok(SessionResponse {
        user_id,
        tenant_id,
        device_id,
        tokens,
    })
}

pub async fn login_start(
    pool: &PgPool,
    request: OpaqueStartRequest,
    identifier_key: &[u8; 32],
) -> Result<LoginStartResponse, AppError> {
    validate_opaque_suite(request.opaque_suite_id)?;
    let email = normalize_email(&request.email)?;
    let device_name = normalize_device_name(request.device_name);
    let client_message = decode_opaque_message(&request.message)?;
    let credential_request = CredentialRequest::<TaskveilCipherSuite>::deserialize(&client_message)
        .map_err(|_| AppError::bad_request("invalid opaque message"))?;
    cleanup_expired_opaque_states(pool).await?;
    ensure_opaque_state_capacity_available(pool, identifier_key).await?;

    let account = sqlx::query!(
        "SELECT u.id, u.opaque_record, u.opaque_suite_id
             FROM users u WHERE lower(u.email) = lower($1)",
        &email,
    )
    .fetch_optional(pool)
    .await?;
    // Run the same membership lookup for known and unknown accounts. A random
    // user context gives the RLS-protected query the same database shape
    // without introducing a persistent decoy identity.
    let lookup_user_id = account.as_ref().map_or_else(Uuid::now_v7, |row| row.id);
    let mut membership_tx = pool.begin().await?;
    db::set_user_context(&mut membership_tx, lookup_user_id).await?;
    let tenant_id = sqlx::query_scalar!(
        "SELECT tenant_id FROM tenant_members
         WHERE user_id = $1 ORDER BY joined_at ASC LIMIT 1",
        lookup_user_id,
    )
    .fetch_optional(&mut *membership_tx)
    .await?;
    membership_tx.commit().await?;

    let expected_suite = i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?;
    let identity = account
        .and_then(|row| {
            (row.opaque_suite_id == expected_suite)
                .then(|| TaskveilServerRegistration::deserialize(&row.opaque_record).ok())
                .flatten()
                .map(|record| (row.id, record))
        })
        .zip(tenant_id)
        .map(|((user_id, record), tenant_id)| (user_id, tenant_id, record));
    let (user_id, tenant_id, server_record) = match identity {
        Some((user_id, tenant_id, record)) => (Some(user_id), Some(tenant_id), Some(record)),
        None => (None, None, None),
    };
    let server_setup = get_or_create_server_setup(pool).await?;
    let mut rng = OsRng;
    let login_start = ServerLogin::start(
        &mut rng,
        &server_setup,
        server_record,
        credential_request,
        email.as_bytes(),
        ServerLoginParameters::default(),
    )
    .map_err(|_| AppError::bad_request("invalid opaque message"))?;

    let state_id = Uuid::now_v7();
    let device_id = Uuid::now_v7();
    let device_challenge = random_device_challenge();
    let expires_at = Utc::now() + Duration::minutes(OPAQUE_STATE_TTL_MINUTES);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO opaque_login_states
            (id, user_id, tenant_id, device_id, device_challenge, device_name,
             opaque_suite_id, server_login_state, expires_at, identifier_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(state_id)
    .bind(user_id)
    .bind(tenant_id)
    .bind(device_id)
    .bind(device_challenge.as_slice())
    .bind(&device_name)
    .bind(i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?)
    .bind(login_start.state.serialize().to_vec())
    .bind(expires_at)
    .bind(identifier_key.as_slice())
    .execute(&mut *tx)
    .await
    .map_err(map_opaque_state_insert_error)?;
    tx.commit().await?;

    Ok(LoginStartResponse {
        state_id,
        opaque_suite_id: CRYPTO_SUITE_ID,
        message: STANDARD.encode(login_start.message.serialize()),
        expires_at,
    })
}

pub async fn login_finish(
    pool: &PgPool,
    request: LoginFinishRequest,
) -> Result<LoginSessionResponse, AppError> {
    // DELETE ... RETURNING is committed before parsing or verifying the proof
    // so every finish attempt consumes its state exactly once.
    let state = consume_login_state(pool, request.state_id).await?;
    let finalization = decode_opaque_message(&request.message)?;
    let credential_finalization =
        CredentialFinalization::<TaskveilCipherSuite>::deserialize(&finalization)
            .map_err(|_| AppError::bad_request("invalid opaque message"))?;

    let server_login = TaskveilServerLogin::deserialize(&state.server_login_state)
        .map_err(|_| AppError::internal())?;
    server_login
        .finish(credential_finalization, ServerLoginParameters::default())
        .map_err(|_| AppError::unauthorized())?;

    let (Some(user_id), Some(tenant_id)) = (state.user_id, state.tenant_id) else {
        return Err(AppError::unauthorized());
    };

    let mut tx = pool.begin().await?;
    db::set_user_context(&mut tx, user_id).await?;
    db::set_tenant_context(&mut tx, tenant_id).await?;
    let key_bundle = load_account_key_bundle(&mut tx, user_id, tenant_id).await?;
    let device_id = state.device_id;
    let device_challenge_expires_at = insert_pending_device(
        &mut tx,
        device_id,
        user_id,
        &state.device_name,
        &state.device_challenge,
    )
    .await?;
    let tokens = create_session(&mut tx, user_id, device_id).await?;
    tx.commit().await?;

    Ok(LoginSessionResponse {
        session: SessionResponse {
            user_id,
            tenant_id,
            device_id,
            tokens,
        },
        key_bundle,
        device_challenge: STANDARD.encode(state.device_challenge),
        device_challenge_expires_at,
    })
}

pub async fn certify_device(
    pool: &PgPool,
    bearer_token: &str,
    enrollment: DeviceEnrollmentDto,
) -> Result<LogoutResponse, AppError> {
    let token_hash = hash_token(bearer_token);
    let mut tx = pool.begin().await?;
    let row = sqlx::query!(
        "SELECT sf.user_id, sf.device_id, d.enrollment_challenge,
                d.enrollment_challenge_expires_at, d.certificate,
                d.certificate_fingerprint, u.account_root_public
         FROM access_tokens at
         JOIN session_families sf ON sf.id = at.family_id
         JOIN devices d ON d.id = sf.device_id AND d.user_id = sf.user_id
         JOIN users u ON u.id = sf.user_id
         WHERE at.token_hash = $1 AND at.expires_at > now()
           AND at.revoked_at IS NULL
           AND sf.revoked_at IS NULL AND sf.absolute_expires_at > now()
           AND d.revoked_at IS NULL",
        token_hash.as_slice(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(AppError::invalid_bearer_token)?;
    let user_id = row.user_id;
    let device_id = row.device_id;
    if let Some(stored_certificate) = row.certificate {
        let submitted_root = STANDARD
            .decode(&enrollment.account_root_public)
            .map_err(|_| AppError::bad_request("invalid account root"))?;
        let submitted_certificate = STANDARD
            .decode(&enrollment.device_certificate)
            .map_err(|_| AppError::bad_request("invalid device certificate"))?;
        let submitted_fingerprint = STANDARD
            .decode(&enrollment.certificate_fingerprint)
            .map_err(|_| AppError::bad_request("invalid device fingerprint"))?;
        if submitted_root == row.account_root_public
            && submitted_certificate == stored_certificate
            && row.certificate_fingerprint.as_deref() == Some(submitted_fingerprint.as_slice())
        {
            return Ok(LogoutResponse {});
        }
        return Err(AppError::conflict("device enrollment changed"));
    }
    if row
        .enrollment_challenge_expires_at
        .is_none_or(|expires_at| expires_at <= Utc::now())
    {
        return Err(AppError::invalid_bearer_token());
    }
    let challenge: [u8; DEVICE_CHALLENGE_LEN] = row
        .enrollment_challenge
        .ok_or_else(AppError::internal)?
        .try_into()
        .map_err(|_| AppError::internal())?;
    let verified = verify_device_enrollment(
        &enrollment,
        user_id,
        device_id,
        &challenge,
        Utc::now().timestamp_millis(),
    )?;
    let stored_root = row.account_root_public;
    if stored_root != verified.account_root_public {
        return Err(AppError::bad_request("account root mismatch"));
    }
    db::set_user_context(&mut tx, user_id).await?;
    let updated = sqlx::query!(
        "UPDATE devices
         SET certificate = $3, certificate_fingerprint = $4,
             key_expires_at = $5, certified_at = now(),
             enrollment_challenge = NULL,
             enrollment_challenge_expires_at = NULL
         WHERE id = $1 AND user_id = $2 AND certificate IS NULL
           AND revoked_at IS NULL",
        device_id,
        user_id,
        &verified.certificate,
        verified.certificate_fingerprint.as_slice(),
        verified.expires_at,
    )
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("device enrollment changed"));
    }
    tx.commit().await?;
    Ok(LogoutResponse {})
}

pub async fn update_key_wrappers(
    pool: &PgPool,
    bearer_token: &str,
    request: UpdateKeyWrappersRequest,
) -> Result<LogoutResponse, AppError> {
    if request.suite_id != CRYPTO_SUITE_ID
        || request.generation == 0
        || request.expected_wrapper_revision == 0
        || request.wrapper_revision != request.expected_wrapper_revision + 1
    {
        return Err(AppError::bad_request("invalid wrapper revision"));
    }
    let wrapped_password = STANDARD
        .decode(&request.wrapped_master_key_by_password)
        .map_err(|_| AppError::bad_request("invalid key wrapper"))?;
    let wrapped_recovery = STANDARD
        .decode(&request.wrapped_master_key_by_recovery)
        .map_err(|_| AppError::bad_request("invalid key wrapper"))?;
    if wrapped_password.is_empty() || wrapped_recovery.is_empty() {
        return Err(AppError::bad_request("invalid key wrapper"));
    }
    let token_hash = hash_token(bearer_token);
    let mut tx = pool.begin().await?;
    let session = sqlx::query!(
        "SELECT sf.user_id
         FROM access_tokens at
         JOIN session_families sf ON sf.id = at.family_id
         JOIN devices d ON d.id = sf.device_id AND d.user_id = sf.user_id
         WHERE at.token_hash = $1 AND at.expires_at > now()
           AND at.revoked_at IS NULL
           AND sf.revoked_at IS NULL AND sf.absolute_expires_at > now()
           AND d.revoked_at IS NULL
           AND d.certificate IS NOT NULL AND d.certified_at IS NOT NULL
           AND (d.key_expires_at IS NULL OR d.key_expires_at > now())",
        token_hash.as_slice(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(AppError::invalid_bearer_token)?;
    let user_id = session.user_id;
    db::set_user_context(&mut tx, user_id).await?;
    let generation = i64::try_from(request.generation)
        .map_err(|_| AppError::bad_request("invalid generation"))?;
    let suite_id =
        i16::try_from(request.suite_id).map_err(|_| AppError::bad_request("invalid suite"))?;
    let wrapper_revision = i64::try_from(request.wrapper_revision)
        .map_err(|_| AppError::bad_request("invalid wrapper revision"))?;
    let expected_wrapper_revision = i64::try_from(request.expected_wrapper_revision)
        .map_err(|_| AppError::bad_request("invalid wrapper revision"))?;
    let updated = sqlx::query!(
        "UPDATE user_key_generations
         SET wrapper_revision = $4, wrapped_mk_by_password = $5,
             wrapped_mk_by_recovery = $6, updated_at = now()
         WHERE user_id = $1 AND generation = $2 AND suite_id = $3
           AND status = 'active' AND wrapper_revision = $7",
        user_id,
        generation,
        suite_id,
        wrapper_revision,
        wrapped_password,
        wrapped_recovery,
        expected_wrapper_revision,
    )
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("stale wrapper revision"));
    }
    tx.commit().await?;
    Ok(LogoutResponse {})
}

pub async fn authenticate(
    pool: &PgPool,
    bearer_token: &str,
    tenant_id: Uuid,
) -> Result<AuthContext, AppError> {
    let mut tx = pool.begin().await?;
    let context = authenticate_in_transaction(&mut tx, bearer_token, tenant_id).await?;
    tx.commit().await?;
    Ok(context)
}

pub(crate) async fn authenticate_in_transaction(
    tx: &mut PgTransaction<'_>,
    bearer_token: &str,
    tenant_id: Uuid,
) -> Result<AuthContext, AppError> {
    let token_hash = hash_token(bearer_token);
    let row = sqlx::query!(
        "SELECT sf.user_id, sf.device_id, sf.id AS family_id
         FROM access_tokens at
         JOIN session_families sf ON sf.id = at.family_id
         JOIN devices d ON d.id = sf.device_id AND d.user_id = sf.user_id
         WHERE at.token_hash = $1
           AND at.expires_at > now()
           AND at.revoked_at IS NULL
           AND sf.absolute_expires_at > now()
           AND sf.revoked_at IS NULL
           AND d.revoked_at IS NULL
           AND d.certificate IS NOT NULL AND d.certified_at IS NOT NULL
           AND (d.key_expires_at IS NULL OR d.key_expires_at > now())",
        token_hash.as_slice(),
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(AppError::invalid_bearer_token)?;

    let user_id = row.user_id;
    let device_id = row.device_id;
    db::set_user_context(tx, user_id).await?;
    let membership = sqlx::query_scalar!(
        "SELECT 1
         FROM tenant_members
         WHERE tenant_id = $1 AND user_id = $2",
        tenant_id,
        user_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if membership.is_none() {
        return Err(AppError::invalid_bearer_token());
    }
    db::set_tenant_context(tx, tenant_id).await?;
    sqlx::query!(
        "UPDATE access_tokens SET last_seen_at = now() WHERE token_hash = $1",
        token_hash.as_slice(),
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "UPDATE session_families SET last_seen_at = now() WHERE id = $1",
        row.family_id,
    )
    .execute(&mut **tx)
    .await?;

    Ok(AuthContext { user_id, device_id })
}

pub fn authorization_server_metadata(issuer: &str) -> AuthorizationServerMetadata {
    let issuer = issuer.trim_end_matches('/').to_string();
    AuthorizationServerMetadata {
        token_endpoint: format!("{issuer}/v1/auth/token"),
        revocation_endpoint: format!("{issuer}/v1/auth/revoke"),
        issuer,
        grant_types_supported: vec!["refresh_token"],
        token_endpoint_auth_methods_supported: vec!["none"],
        revocation_endpoint_auth_methods_supported: vec!["none"],
    }
}

pub async fn refresh_session(
    pool: &PgPool,
    request: TokenRequest,
) -> Result<TokenResponse, AppError> {
    if request.grant_type != "refresh_token" {
        return Err(AppError::bad_request("unsupported_grant_type"));
    }
    validate_native_client(&request.client_id)?;
    cleanup_expired_auth_state(pool).await?;

    let token_hash = hash_token(&request.refresh_token);
    let mut tx = pool.begin().await?;
    let row = sqlx::query!(
        r#"SELECT rt.id, rt.family_id, rt.generation, rt.expires_at,
                  rt.consumed_at AS "consumed_at?", rt.revoked_at AS "token_revoked_at?",
                  sf.absolute_expires_at,
                  sf.revoked_at AS "family_revoked_at?",
                  d.revoked_at AS "device_revoked_at?",
                  d.certificate AS "device_certificate?",
                  d.certified_at AS "device_certified_at?",
                  d.key_expires_at AS "device_key_expires_at?"
           FROM refresh_tokens rt
           JOIN session_families sf ON sf.id = rt.family_id
           JOIN devices d ON d.id = sf.device_id AND d.user_id = sf.user_id
           WHERE rt.token_hash = $1 AND sf.client_id = $2
           FOR UPDATE OF rt, sf"#,
        token_hash.as_slice(),
        NATIVE_CLIENT_ID,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(AppError::invalid_grant)?;

    if row.consumed_at.is_some() {
        if row.family_revoked_at.is_none() {
            revoke_family(&mut tx, row.family_id, "refresh_reuse").await?;
        }
        tx.commit().await?;
        return Err(AppError::invalid_grant());
    }

    let now = Utc::now();
    if row.family_revoked_at.is_some() || row.token_revoked_at.is_some() || row.expires_at <= now {
        tx.commit().await?;
        return Err(AppError::invalid_grant());
    }
    if row.absolute_expires_at <= now {
        revoke_family(&mut tx, row.family_id, "absolute_expiry").await?;
        tx.commit().await?;
        return Err(AppError::invalid_grant());
    }
    if row.device_revoked_at.is_some() {
        revoke_family(&mut tx, row.family_id, "device_revocation").await?;
        tx.commit().await?;
        return Err(AppError::invalid_grant());
    }
    if row.device_certificate.is_none()
        || row.device_certified_at.is_none()
        || row
            .device_key_expires_at
            .is_some_and(|expires_at| expires_at <= now)
    {
        if row.device_certificate.is_some() {
            revoke_family(&mut tx, row.family_id, "device_key_expiry").await?;
        }
        tx.commit().await?;
        return Err(AppError::invalid_grant());
    }

    let refresh_expires_at = std::cmp::min(
        now + Duration::days(REFRESH_TOKEN_IDLE_TTL_DAYS),
        row.absolute_expires_at,
    );
    let tokens = insert_token_pair(
        &mut tx,
        row.family_id,
        row.generation + 1,
        refresh_expires_at,
        Some(row.id),
        now,
    )
    .await?;
    sqlx::query!(
        "UPDATE session_families SET last_seen_at = $2 WHERE id = $1",
        row.family_id,
        now,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(tokens)
}

pub async fn revoke_token(
    pool: &PgPool,
    request: RevocationRequest,
) -> Result<LogoutResponse, AppError> {
    validate_native_client(&request.client_id)?;
    if request.token.is_empty() {
        return Err(AppError::bad_request("invalid_request"));
    }
    if let Some(hint) = request.token_type_hint.as_deref() {
        if hint != "access_token" && hint != "refresh_token" {
            return Err(AppError::bad_request("unsupported_token_type"));
        }
    }

    let token_hash = hash_token(&request.token);
    let mut tx = pool.begin().await?;
    let family_id = sqlx::query_scalar!(
        "SELECT family_id FROM refresh_tokens WHERE token_hash = $1",
        token_hash.as_slice(),
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(family_id) = family_id {
        revoke_family(&mut tx, family_id, "client_revocation").await?;
        tx.commit().await?;
        return Ok(LogoutResponse {});
    }

    sqlx::query!(
        "UPDATE access_tokens
         SET revoked_at = coalesce(revoked_at, now())
         WHERE token_hash = $1",
        token_hash.as_slice(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(LogoutResponse {})
}

async fn revoke_family(
    tx: &mut PgTransaction<'_>,
    family_id: Uuid,
    reason: &'static str,
) -> Result<(), AppError> {
    sqlx::query!(
        "UPDATE session_families
         SET revoked_at = coalesce(revoked_at, now()),
             revocation_reason = coalesce(revocation_reason, $2)
         WHERE id = $1",
        family_id,
        reason,
    )
    .execute(&mut **tx)
    .await?;
    // Every authorization and refresh query checks the family row. Keeping
    // family revocation O(1) avoids unbounded fan-out across a long rotation
    // history; bounded GC removes child rows later.
    Ok(())
}

fn validate_native_client(client_id: &str) -> Result<(), AppError> {
    if client_id != NATIVE_CLIENT_ID {
        return Err(AppError::bad_request("invalid_client"));
    }
    Ok(())
}

async fn ensure_opaque_state_capacity_available(
    pool: &PgPool,
    identifier_key: &[u8; 32],
) -> Result<(), AppError> {
    let available = sqlx::query_scalar::<_, bool>(
        "SELECT
             global.active_count < $1
             AND coalesce(identifier.active_count, 0) < $2
         FROM opaque_state_global_capacity global
         LEFT JOIN opaque_state_identifier_capacity identifier
           ON identifier.identifier_key = $3
         WHERE global.singleton = TRUE",
    )
    .bind(MAX_ACTIVE_OPAQUE_STATES)
    .bind(MAX_ACTIVE_OPAQUE_STATES_PER_IDENTIFIER)
    .bind(identifier_key.as_slice())
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    if !available {
        return Err(AppError::rate_limited(None));
    }
    Ok(())
}

pub async fn cleanup_expired_opaque_states(pool: &PgPool) -> Result<u64, AppError> {
    let mut tx = pool.begin().await?;
    let registration_ids = sqlx::query_scalar::<_, Uuid>(
        "WITH expired AS (
             SELECT id FROM opaque_registration_states
             WHERE expires_at <= now()
             ORDER BY expires_at, id
             LIMIT $1
         )
         DELETE FROM opaque_registration_states
         USING expired
         WHERE opaque_registration_states.id = expired.id
         RETURNING opaque_registration_states.id",
    )
    .bind(AUTH_GC_OPAQUE_STATE_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await?;
    let login_ids = sqlx::query_scalar::<_, Uuid>(
        "WITH expired AS (
             SELECT id FROM opaque_login_states
             WHERE expires_at <= now()
             ORDER BY expires_at, id
             LIMIT $1
         )
         DELETE FROM opaque_login_states
         USING expired
         WHERE opaque_login_states.id = expired.id
         RETURNING opaque_login_states.id",
    )
    .bind(AUTH_GC_OPAQUE_STATE_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await?;
    let removed = registration_ids.len() + login_ids.len();
    tx.commit().await?;
    u64::try_from(removed).map_err(|_| AppError::internal())
}

fn map_opaque_state_insert_error(error: sqlx_core::Error) -> AppError {
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

pub async fn cleanup_expired_auth_state(pool: &PgPool) -> Result<u64, AppError> {
    let expired_access_tokens = sqlx::query!(
        "WITH expired AS (
             SELECT at.id, at.expires_at
             FROM access_tokens at
             JOIN session_families sf ON sf.id = at.family_id
             JOIN devices d ON d.id = sf.device_id
             WHERE at.expires_at <= now()
                OR sf.absolute_expires_at <= now()
                OR (
                    d.certificate IS NULL
                    AND d.certified_at IS NULL
                    AND d.enrollment_challenge_expires_at <= now()
                )
             ORDER BY at.expires_at, at.id
             LIMIT $1
         )
         DELETE FROM access_tokens
         USING expired
         WHERE access_tokens.id = expired.id",
        AUTH_GC_ACCESS_TOKEN_BATCH_SIZE,
    )
    .execute(pool)
    .await?
    .rows_affected();
    let expired_refresh_tokens = sqlx::query!(
        "WITH expired AS (
             SELECT rt.id, sf.absolute_expires_at
             FROM refresh_tokens rt
             JOIN session_families sf ON sf.id = rt.family_id
             JOIN devices d ON d.id = sf.device_id
             WHERE sf.absolute_expires_at <= now()
                OR (
                    d.certificate IS NULL
                    AND d.certified_at IS NULL
                    AND d.enrollment_challenge_expires_at <= now()
                )
             ORDER BY sf.absolute_expires_at, rt.id
             LIMIT $1
         )
         DELETE FROM refresh_tokens
         USING expired
         WHERE refresh_tokens.id = expired.id",
        AUTH_GC_REFRESH_TOKEN_BATCH_SIZE,
    )
    .execute(pool)
    .await?
    .rows_affected();
    let expired_families = sqlx::query!(
        "WITH expired AS (
             SELECT sf.id
             FROM session_families sf
             JOIN devices d ON d.id = sf.device_id
             WHERE (
                   sf.absolute_expires_at <= now()
                   OR (
                       d.certificate IS NULL
                       AND d.certified_at IS NULL
                       AND d.enrollment_challenge_expires_at <= now()
                   )
               )
               AND NOT EXISTS (
                   SELECT 1 FROM access_tokens at WHERE at.family_id = sf.id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM refresh_tokens rt WHERE rt.family_id = sf.id
               )
             ORDER BY sf.absolute_expires_at, sf.id
             LIMIT $1
         )
         DELETE FROM session_families
         USING expired
         WHERE session_families.id = expired.id",
        AUTH_GC_SESSION_FAMILY_BATCH_SIZE,
    )
    .execute(pool)
    .await?
    .rows_affected();
    let expired_pending_devices = sqlx::query!(
        "WITH expired AS (
             SELECT d.id
             FROM devices d
             WHERE d.certificate IS NULL
               AND d.certified_at IS NULL
               AND d.enrollment_challenge_expires_at <= now()
               AND NOT EXISTS (
                   SELECT 1 FROM session_families sf WHERE sf.device_id = d.id
               )
             ORDER BY d.enrollment_challenge_expires_at, d.id
             LIMIT $1
         )
         DELETE FROM devices
         USING expired
         WHERE devices.id = expired.id",
        AUTH_GC_PENDING_DEVICE_BATCH_SIZE,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(expired_access_tokens
        + expired_refresh_tokens
        + expired_families
        + expired_pending_devices
        + cleanup_expired_opaque_states(pool).await?)
}

pub(crate) fn normalize_email(email: &str) -> Result<String, AppError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > 320 || !email.is_ascii() || !email.contains('@') {
        return Err(AppError::bad_request("invalid email"));
    }
    Ok(email)
}

fn normalize_device_name(device_name: Option<String>) -> String {
    let trimmed = device_name
        .unwrap_or_else(|| "Taskveil device".to_string())
        .trim()
        .to_string();
    if trimmed.is_empty() {
        "Taskveil device".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn validate_opaque_suite(suite_id: u16) -> Result<(), AppError> {
    if suite_id != CRYPTO_SUITE_ID {
        return Err(AppError::bad_request("unsupported opaque suite"));
    }
    Ok(())
}

fn decode_opaque_message(message: &str) -> Result<Vec<u8>, AppError> {
    STANDARD
        .decode(message)
        .map_err(|_| AppError::bad_request("invalid base64 message"))
}

fn decode_bytes_field(message: &str, error: &'static str) -> Result<Vec<u8>, AppError> {
    STANDARD
        .decode(message)
        .map_err(|_| AppError::bad_request(error))
}

fn decode_account_root_public(message: &str, error: &'static str) -> Result<Vec<u8>, AppError> {
    let bytes = decode_bytes_field(message, error)?;
    AccountRootPublicKeys::decode(&bytes).map_err(|_| AppError::bad_request(error))?;
    Ok(bytes)
}

fn random_device_challenge() -> [u8; DEVICE_CHALLENGE_LEN] {
    let mut challenge = [0u8; DEVICE_CHALLENGE_LEN];
    OsRng.fill_bytes(&mut challenge);
    challenge
}

async fn get_or_create_server_setup(pool: &PgPool) -> Result<TaskveilServerSetup, AppError> {
    let mut rng = OsRng;
    let generated = TaskveilServerSetup::new(&mut rng).serialize().to_vec();
    sqlx::query!(
        "INSERT INTO opaque_server_setup (singleton, opaque_suite_id, setup)
         VALUES (TRUE, $1, $2)
         ON CONFLICT (singleton) DO NOTHING",
        i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?,
        &generated,
    )
    .execute(pool)
    .await?;

    let bytes = sqlx::query_scalar!(
        "SELECT setup FROM opaque_server_setup
             WHERE singleton = TRUE AND opaque_suite_id = $1",
        i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?,
    )
    .fetch_one(pool)
    .await?;
    TaskveilServerSetup::deserialize(&bytes).map_err(|_| AppError::internal())
}

struct RegistrationState {
    user_id: Uuid,
    tenant_id: Uuid,
    device_id: Uuid,
    device_challenge: [u8; DEVICE_CHALLENGE_LEN],
    email: String,
    device_name: String,
}

async fn consume_registration_state(
    tx: &mut PgTransaction<'_>,
    state_id: Uuid,
) -> Result<RegistrationState, AppError> {
    let row = sqlx::query!(
        "DELETE FROM opaque_registration_states
         WHERE id = $1 AND expires_at > now()
         RETURNING user_id, tenant_id, device_id, device_challenge, email, device_name,
                   opaque_suite_id",
        state_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("invalid or expired opaque state"))?;
    let suite_id = row.opaque_suite_id;
    if suite_id != i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())? {
        return Err(AppError::bad_request("unsupported opaque suite"));
    }
    Ok(RegistrationState {
        user_id: row.user_id,
        tenant_id: row.tenant_id,
        device_id: row.device_id,
        device_challenge: row
            .device_challenge
            .try_into()
            .map_err(|_| AppError::internal())?,
        email: row.email,
        device_name: row.device_name,
    })
}

struct LoginState {
    user_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    device_id: Uuid,
    device_challenge: [u8; DEVICE_CHALLENGE_LEN],
    device_name: String,
    server_login_state: Vec<u8>,
}

async fn consume_login_state(pool: &PgPool, state_id: Uuid) -> Result<LoginState, AppError> {
    let row = sqlx::query(
        "DELETE FROM opaque_login_states
         WHERE id = $1 AND expires_at > now()
         RETURNING user_id, tenant_id, device_id, device_challenge, device_name,
                   opaque_suite_id, server_login_state",
    )
    .bind(state_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(AppError::unauthorized)?;
    let suite_id: i16 = row.try_get("opaque_suite_id")?;
    if suite_id != i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())? {
        return Err(AppError::unauthorized());
    }
    Ok(LoginState {
        user_id: row.try_get("user_id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        device_challenge: row
            .try_get::<Vec<u8>, _>("device_challenge")?
            .try_into()
            .map_err(|_| AppError::internal())?,
        device_name: row.try_get("device_name")?,
        server_login_state: row.try_get("server_login_state")?,
    })
}

struct DecodedAccountKeyBundle {
    suite_id: i16,
    generation: i64,
    tenant_generation: i64,
    wrapper_revision: i64,
    wrapped_master_key_by_password: Vec<u8>,
    wrapped_master_key_by_recovery: Vec<u8>,
    account_root_public: Vec<u8>,
    wrapped_account_root_private: Vec<u8>,
    wrapped_tenant_root_dek: Vec<u8>,
    tenant_key_manifest: Vec<u8>,
}

fn decode_account_key_bundle(
    bundle: &AccountKeyBundleDto,
) -> Result<DecodedAccountKeyBundle, AppError> {
    if bundle.suite_id != CRYPTO_SUITE_ID
        || bundle.generation != INITIAL_KEY_GENERATION
        || bundle.tenant_generation != INITIAL_KEY_GENERATION
        || bundle.wrapper_revision == 0
    {
        return Err(AppError::bad_request("invalid key bundle"));
    }
    Ok(DecodedAccountKeyBundle {
        suite_id: i16::try_from(bundle.suite_id)
            .map_err(|_| AppError::bad_request("invalid key bundle"))?,
        generation: i64::try_from(bundle.generation)
            .map_err(|_| AppError::bad_request("invalid key bundle"))?,
        tenant_generation: i64::try_from(bundle.tenant_generation)
            .map_err(|_| AppError::bad_request("invalid key bundle"))?,
        wrapper_revision: i64::try_from(bundle.wrapper_revision)
            .map_err(|_| AppError::bad_request("invalid key bundle"))?,
        wrapped_master_key_by_password: decode_bytes_field(
            &bundle.wrapped_master_key_by_password,
            "invalid key bundle",
        )?,
        wrapped_master_key_by_recovery: decode_bytes_field(
            &bundle.wrapped_master_key_by_recovery,
            "invalid key bundle",
        )?,
        account_root_public: decode_account_root_public(
            &bundle.account_root_public,
            "invalid key bundle",
        )?,
        wrapped_account_root_private: decode_bytes_field(
            &bundle.wrapped_account_root_private,
            "invalid key bundle",
        )?,
        wrapped_tenant_root_dek: decode_bytes_field(
            &bundle.wrapped_tenant_root_dek,
            "invalid key bundle",
        )?,
        tenant_key_manifest: decode_bytes_field(&bundle.tenant_key_manifest, "invalid key bundle")?,
    })
}

async fn insert_account_key_bundle(
    tx: &mut PgTransaction<'_>,
    user_id: Uuid,
    tenant_id: Uuid,
    bundle: DecodedAccountKeyBundle,
) -> Result<(), AppError> {
    sqlx::query!(
        "INSERT INTO user_key_generations (
            user_id,
            status,
            suite_id,
            generation,
            wrapper_revision,
            wrapped_mk_by_password,
            wrapped_mk_by_recovery,
            account_root_public,
            wrapped_account_root_private
         ) VALUES ($1, 'active', $2, $3, $4, $5, $6, $7, $8)",
        user_id,
        bundle.suite_id,
        bundle.generation,
        bundle.wrapper_revision,
        &bundle.wrapped_master_key_by_password,
        &bundle.wrapped_master_key_by_recovery,
        &bundle.account_root_public,
        &bundle.wrapped_account_root_private,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "INSERT INTO tenant_key_generations
            (tenant_id, suite_id, generation, status, minimum_write_generation,
             signed_manifest, wrapped_tenant_root_dek, activated_at)
         VALUES ($1, $2, $3, 'active', $3, $4, $5, now())",
        tenant_id,
        bundle.suite_id,
        bundle.tenant_generation,
        &bundle.tenant_key_manifest,
        &bundle.wrapped_tenant_root_dek,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn load_account_key_bundle(
    tx: &mut PgTransaction<'_>,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Result<AccountKeyBundleDto, AppError> {
    let user = sqlx::query!(
        "SELECT
            suite_id,
            generation,
            wrapper_revision,
            wrapped_mk_by_password AS wrapped_master_key_by_password,
            wrapped_mk_by_recovery AS wrapped_master_key_by_recovery,
            account_root_public,
            wrapped_account_root_private
         FROM user_key_generations
         WHERE user_id = $1 AND status = 'active'",
        user_id,
    )
    .fetch_one(&mut **tx)
    .await?;
    let tenant = sqlx::query!(
        "SELECT suite_id, generation, signed_manifest, wrapped_tenant_root_dek
         FROM tenant_key_generations
         WHERE tenant_id = $1 AND status = 'active'",
        tenant_id,
    )
    .fetch_one(&mut **tx)
    .await?;
    let expected_suite = user.suite_id;
    let tenant_suite = tenant.suite_id;
    let tenant_generation = tenant.generation;
    if expected_suite != i16::try_from(CRYPTO_SUITE_ID).map_err(|_| AppError::internal())?
        || expected_suite != tenant_suite
    {
        return Err(AppError::internal());
    }

    Ok(AccountKeyBundleDto {
        suite_id: u16::try_from(user.suite_id).map_err(|_| AppError::internal())?,
        generation: u64::try_from(user.generation).map_err(|_| AppError::internal())?,
        tenant_generation: u64::try_from(tenant_generation).map_err(|_| AppError::internal())?,
        wrapper_revision: u64::try_from(user.wrapper_revision).map_err(|_| AppError::internal())?,
        wrapped_master_key_by_password: STANDARD.encode(user.wrapped_master_key_by_password),
        wrapped_master_key_by_recovery: STANDARD.encode(user.wrapped_master_key_by_recovery),
        account_root_public: STANDARD.encode(user.account_root_public),
        wrapped_account_root_private: STANDARD.encode(user.wrapped_account_root_private),
        wrapped_tenant_root_dek: STANDARD.encode(tenant.wrapped_tenant_root_dek),
        tenant_key_manifest: STANDARD.encode(tenant.signed_manifest),
    })
}

struct VerifiedEnrollment {
    account_root_public: Vec<u8>,
    certificate: Vec<u8>,
    certificate_fingerprint: [u8; DEVICE_FINGERPRINT_LEN],
    expires_at: DateTime<Utc>,
}

fn verify_device_enrollment(
    enrollment: &DeviceEnrollmentDto,
    user_id: Uuid,
    device_id: Uuid,
    challenge: &[u8; DEVICE_CHALLENGE_LEN],
    now_ms: i64,
) -> Result<VerifiedEnrollment, AppError> {
    if enrollment.suite_id != CRYPTO_SUITE_ID {
        return Err(AppError::bad_request("unsupported device suite"));
    }
    let account_root_public =
        decode_account_root_public(&enrollment.account_root_public, "invalid account root")?;
    let root = AccountRootPublicKeys::decode(&account_root_public)
        .map_err(|_| AppError::bad_request("invalid account root"))?;
    let certificate =
        decode_bytes_field(&enrollment.device_certificate, "invalid device certificate")?;
    let certificate_value = DeviceCertificate::decode(&certificate)
        .map_err(|_| AppError::bad_request("invalid device certificate"))?;
    if root.user_id != user_id
        || certificate_value.user_id != user_id
        || certificate_value.device_id != device_id
    {
        return Err(AppError::bad_request("device identity mismatch"));
    }
    verify_device_certificate(&certificate_value, &root, now_ms, false)
        .map_err(|_| AppError::bad_request("invalid device certificate"))?;
    let certificate_fingerprint: [u8; DEVICE_FINGERPRINT_LEN] = STANDARD
        .decode(&enrollment.certificate_fingerprint)
        .map_err(|_| AppError::bad_request("invalid device proof"))?
        .try_into()
        .map_err(|_| AppError::bad_request("invalid device proof"))?;
    let proof_signature: [u8; ED25519_SIGNATURE_LEN] = STANDARD
        .decode(&enrollment.proof_signature)
        .map_err(|_| AppError::bad_request("invalid device proof"))?
        .try_into()
        .map_err(|_| AppError::bad_request("invalid device proof"))?;
    let proof = DeviceProofOfPossession {
        certificate_fingerprint,
        signature: proof_signature,
    };
    verify_device_proof(&certificate_value, challenge, &proof)
        .map_err(|_| AppError::bad_request("invalid device proof"))?;
    let expires_at = DateTime::from_timestamp_millis(certificate_value.expires_at_ms)
        .ok_or_else(|| AppError::bad_request("invalid device certificate"))?;
    Ok(VerifiedEnrollment {
        account_root_public,
        certificate,
        certificate_fingerprint,
        expires_at,
    })
}

async fn insert_certified_device(
    tx: &mut PgTransaction<'_>,
    device_id: Uuid,
    user_id: Uuid,
    device_name: &str,
    enrollment: &VerifiedEnrollment,
) -> Result<(), AppError> {
    sqlx::query!(
        "INSERT INTO devices
            (id, user_id, device_name, certificate, certificate_fingerprint,
             key_expires_at, certified_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())",
        device_id,
        user_id,
        device_name,
        &enrollment.certificate,
        enrollment.certificate_fingerprint.as_slice(),
        enrollment.expires_at,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_pending_device(
    tx: &mut PgTransaction<'_>,
    device_id: Uuid,
    user_id: Uuid,
    device_name: &str,
    challenge: &[u8; DEVICE_CHALLENGE_LEN],
) -> Result<DateTime<Utc>, AppError> {
    let challenge_expires_at = Utc::now() + Duration::minutes(10);
    sqlx::query!(
        "INSERT INTO devices
            (id, user_id, device_name, enrollment_challenge,
             enrollment_challenge_expires_at)
         VALUES ($1, $2, $3, $4, $5)",
        device_id,
        user_id,
        device_name,
        challenge.as_slice(),
        challenge_expires_at,
    )
    .execute(&mut **tx)
    .await?;
    Ok(challenge_expires_at)
}

async fn create_session(
    tx: &mut PgTransaction<'_>,
    user_id: Uuid,
    device_id: Uuid,
) -> Result<TokenResponse, AppError> {
    let now = Utc::now();
    let family_id = Uuid::now_v7();
    let absolute_expires_at = now + Duration::days(SESSION_FAMILY_TTL_DAYS);
    sqlx::query!(
        "INSERT INTO session_families
            (id, user_id, device_id, client_id, absolute_expires_at, last_seen_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
        family_id,
        user_id,
        device_id,
        NATIVE_CLIENT_ID,
        absolute_expires_at,
        now,
    )
    .execute(&mut **tx)
    .await?;
    insert_token_pair(
        tx,
        family_id,
        1,
        now + Duration::days(REFRESH_TOKEN_IDLE_TTL_DAYS),
        None,
        now,
    )
    .await
}

async fn insert_token_pair(
    tx: &mut PgTransaction<'_>,
    family_id: Uuid,
    generation: i64,
    refresh_expires_at: DateTime<Utc>,
    replaced_token_id: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<TokenResponse, AppError> {
    let access_token = generate_token();
    let refresh_token = generate_token();
    let access_hash = hash_token(&access_token);
    let refresh_hash = hash_token(&refresh_token);
    let access_expires_at = std::cmp::min(
        now + Duration::minutes(ACCESS_TOKEN_TTL_MINUTES),
        refresh_expires_at,
    );
    let access_token_id = Uuid::now_v7();
    let refresh_token_id = Uuid::now_v7();

    sqlx::query!(
        "INSERT INTO access_tokens
            (id, family_id, token_hash, expires_at, last_seen_at)
         VALUES ($1, $2, $3, $4, $5)",
        access_token_id,
        family_id,
        access_hash.as_slice(),
        access_expires_at,
        now,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO refresh_tokens
            (id, family_id, generation, token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
        refresh_token_id,
        family_id,
        generation,
        refresh_hash.as_slice(),
        refresh_expires_at,
    )
    .execute(&mut **tx)
    .await?;
    if let Some(replaced_token_id) = replaced_token_id {
        let updated = sqlx::query!(
            "UPDATE refresh_tokens
             SET consumed_at = $2, replaced_by_id = $3
             WHERE id = $1 AND consumed_at IS NULL AND revoked_at IS NULL",
            replaced_token_id,
            now,
            refresh_token_id,
        )
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::invalid_grant());
        }
    }

    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: u64::try_from((access_expires_at - now).num_seconds().max(0))
            .map_err(|_| AppError::internal())?,
        access_expires_at,
        refresh_token,
        refresh_token_expires_in: u64::try_from((refresh_expires_at - now).num_seconds().max(0))
            .map_err(|_| AppError::internal())?,
        refresh_expires_at,
    })
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn map_insert_user_error(error: sqlx_core::Error) -> AppError {
    if let sqlx_core::Error::Database(db_error) = &error {
        if db_error.constraint() == Some("users_email_lower_unique") {
            return AppError::conflict("account already exists");
        }
    }
    AppError::from(error)
}

#[cfg(test)]
mod tests {
    use super::normalize_email;

    #[test]
    fn email_canonicalization_rejects_unicode_case_variants() {
        assert_eq!(
            normalize_email(" Alice@Example.COM ").expect("ASCII email"),
            "alice@example.com"
        );
        assert!(normalize_email("élise@example.com").is_err());
        assert!(normalize_email("ÉLISE@example.com").is_err());
    }
}
