use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::IntoResponse,
    Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::Utc;
use opaque_ke::{ClientLogin, ClientRegistration, CredentialResponse};
use rand::rngs::OsRng;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::{PgPool, Postgres};
use taskveil_crypto::{opaque_login_parameters, TaskveilCipherSuite, CRYPTO_SUITE_ID};
use taskveil_server::{
    auth,
    auth_protection::AuthProtection,
    billing::{BillingEnvironment, BillingService},
    build_router, db,
    email_verification::{
        EmailVerificationConfig, EmailVerificationService, RegistrationRequest,
        RegistrationResendRequest, RegistrationStatusRequest, RegistrationVerifyRequest,
    },
    AppState,
};
use taskveil_sync::account::{
    unwrap_login_key_bundle, AccountClient, AccountClientError, AccountKeyBundleDto,
    AccountRegisterOutcome, AccountRegistrationReconcile, AccountRegistrationRequestPrepared,
    AccountRegistrationStartPrepared,
};
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tower::ServiceExt;
use uuid::Uuid;

struct TestApp {
    app: Router,
    pool: PgPool,
    application_pool: PgPool,
    _postgres: ContainerAsync<postgres::Postgres>,
}

async fn setup() -> TestApp {
    let postgres = postgres::Postgres::default().start().await.unwrap();
    let host = postgres.get_host().await.unwrap();
    let port = postgres.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = db::connect(&database_url).await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    raw_sql(
        "CREATE ROLE taskveil_runtime_test LOGIN PASSWORD 'taskveil-runtime-test'
         NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS",
    )
    .execute(&pool)
    .await
    .unwrap();
    raw_sql("GRANT taskveil_app TO taskveil_runtime_test")
        .execute(&pool)
        .await
        .unwrap();
    let application_url =
        format!("postgres://taskveil_runtime_test:taskveil-runtime-test@{host}:{port}/postgres");
    let application_pool = db::connect_application(&application_url).await.unwrap();
    let app = build_router(AppState {
        pool: application_pool.clone(),
        billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
        auth_issuer: "http://localhost".to_string(),
        resync_tokens: taskveil_server::resync_token::ResyncTokenKeyring::for_tests(),
        auth_protection: AuthProtection::new([0xA7; 32]),
        trust_source_ip_header: false,
        email_verification:
            taskveil_server::email_verification::EmailVerificationService::for_tests(),
    });
    TestApp {
        app,
        pool,
        application_pool,
        _postgres: postgres,
    }
}

fn manual_email_service() -> EmailVerificationService {
    EmailVerificationService::for_tests_with_config(EmailVerificationConfig {
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

async fn staged_register(
    client: &AccountClient,
    email: &str,
    password: &str,
    device_name: Option<&str>,
    device_key: &[u8; 32],
    otp: &str,
) -> Result<AccountRegisterOutcome, AccountClientError> {
    let mailbox = client.begin_registration(email).await?;
    let verified = client.verify_registration_otp(&mailbox, otp).await?;
    let prepared = client
        .prepare_registration(&verified, password, device_name, device_key)
        .await?;
    client.finish_registration(&prepared).await
}

#[tokio::test]
async fn decoy_registration_recovery_is_not_attributed_to_existing_account_login() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = AccountClient::new(&server_url).unwrap();
    let original = staged_register(
        &client,
        "existing@example.com",
        "existing password",
        Some("original device"),
        &[0x31; 32],
        "00000001",
    )
    .await
    .unwrap();

    let mailbox = client
        .begin_registration("existing@example.com")
        .await
        .unwrap();
    let verified = client
        .verify_registration_otp(&mailbox, "00000001")
        .await
        .unwrap();
    let decoy_start = client
        .prepare_registration_start(&verified, "existing password", Some("decoy device"))
        .unwrap();
    let decoy_prepared = client
        .send_registration_start(&decoy_start, "existing password", &[0x32; 32])
        .await
        .unwrap();
    let login = client
        .begin_login(
            "existing@example.com",
            "existing password",
            Some("recovery login"),
            &[0x33; 32],
        )
        .await
        .unwrap();

    assert!(
        !AccountClient::registration_matches_account_keys(&decoy_prepared, &login.keys).unwrap()
    );
    assert_eq!(
        original.keys.account_root_public.encode().unwrap(),
        login.keys.account_root_public.encode().unwrap()
    );
}

#[tokio::test]
async fn registration_start_replays_only_the_exact_idempotent_request() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let http = reqwest::Client::new();
    let handoff_secret = [0x33_u8; 32];
    let handoff_challenge: [u8; 32] = Sha256::digest(handoff_secret).into();
    let requested: Value = http
        .post(format!("{server_url}/v1/auth/register/request"))
        .header("idempotency-key", Uuid::now_v7().to_string())
        .json(&serde_json::json!({
            "email": "idempotent@example.com",
            "handoff_challenge": URL_SAFE_NO_PAD.encode(handoff_challenge)
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ticket: Value = http
        .post(format!("{server_url}/v1/auth/register/verify"))
        .header("idempotency-key", Uuid::now_v7().to_string())
        .json(&serde_json::json!({
            "request_id": requested["request_id"],
            "handoff_secret": URL_SAFE_NO_PAD.encode(handoff_secret),
            "otp": "00000001"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ticket["registration_ticket"].is_string());

    let client_start =
        ClientRegistration::<TaskveilCipherSuite>::start(&mut OsRng, b"test password").unwrap();
    let request = serde_json::json!({
        "registration_ticket": ticket["registration_ticket"],
        "device_name": "idempotency test",
        "opaque_suite_id": CRYPTO_SUITE_ID,
        "message": STANDARD.encode(client_start.message.serialize())
    });
    let idempotency_key = Uuid::now_v7().to_string();
    let first = http
        .post(format!("{server_url}/v1/auth/register/start"))
        .header("idempotency-key", &idempotency_key)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = first.json().await.unwrap();
    let replay = http
        .post(format!("{server_url}/v1/auth/register/start"))
        .header("idempotency-key", &idempotency_key)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.json::<Value>().await.unwrap(), first_body);

    let mut mismatched = request;
    mismatched["device_name"] = Value::String("changed request".to_string());
    let rejected = http
        .post(format!("{server_url}/v1/auth/register/start"))
        .header("idempotency-key", idempotency_key)
        .json(&mismatched)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let state_count: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM opaque_registration_states
         WHERE id = $1",
    )
    .bind(Uuid::parse_str(first_body["state_id"].as_str().unwrap()).unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(state_count, 1);
}

#[tokio::test]
async fn registration_request_and_start_replay_after_response_loss_and_restart() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = AccountClient::new(&server_url).unwrap();

    let request_prepared = client
        .prepare_registration_request("restart@example.com")
        .unwrap();
    let request_journal = request_prepared.encode().unwrap();
    let lost_response = client
        .send_registration_request(&request_prepared)
        .await
        .unwrap();
    let restored_request = AccountRegistrationRequestPrepared::decode(&request_journal).unwrap();
    let mailbox = client
        .send_registration_request(&restored_request)
        .await
        .unwrap();
    assert_eq!(mailbox.request_id(), lost_response.request_id());
    let challenges: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_registration_challenges")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(challenges, 1);

    let verified = client
        .verify_registration_otp(&mailbox, "00000001")
        .await
        .unwrap();
    let start_prepared = client
        .prepare_registration_start(&verified, "restart-only password", Some("restart device"))
        .unwrap();
    let start_journal = start_prepared.encode().unwrap();
    assert!(!String::from_utf8_lossy(&start_journal).contains("restart-only password"));
    let _lost_response = client
        .send_registration_start(&start_prepared, "restart-only password", &[0x74; 32])
        .await
        .unwrap();
    let restored_start = AccountRegistrationStartPrepared::decode(&start_journal).unwrap();
    let finish = client
        .send_registration_start(&restored_start, "restart-only password", &[0x74; 32])
        .await
        .unwrap();
    let outcome = client.finish_registration(&finish).await.unwrap();
    assert_eq!(outcome.session.email, "restart@example.com");

    let users: i64 = query::<Postgres>("SELECT count(*) AS count FROM users")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    let request_replays: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM registration_request_idempotency")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    let start_replays: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM registration_start_idempotency")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!((users, request_replays, start_replays), (1, 1, 1));
}

#[tokio::test]
async fn unverified_request_cannot_reserve_a_mailbox_before_the_owner_verifies() {
    let test = setup().await;
    let attacker = manual_email_service()
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "Owner@BÜCHER.example".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode([0x31; 32]),
            },
            "attacker-request",
        )
        .await
        .unwrap();
    let attacker_replay = manual_email_service()
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "Owner@BÜCHER.example".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode([0x31; 32]),
            },
            "attacker-request",
        )
        .await
        .unwrap();
    assert_eq!(attacker_replay, attacker);
    assert!(manual_email_service()
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "changed@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode([0x31; 32]),
            },
            "attacker-request",
        )
        .await
        .is_err());
    let owner_secret = [0x32; 32];
    let owner_handoff_challenge: [u8; 32] = Sha256::digest(owner_secret).into();
    let owner_service = EmailVerificationService::for_tests();
    let owner = owner_service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "Owner@xn--bcher-kva.example".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(owner_handoff_challenge),
            },
            "owner-request",
        )
        .await
        .unwrap();
    let owner_verification = owner_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: owner.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(owner_secret),
                otp: "00000001".to_string(),
            },
            "owner-verify",
        )
        .await
        .unwrap();
    let restarted_service = EmailVerificationService::for_tests();
    let replayed_verification = restarted_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: owner.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(owner_secret),
                otp: "00000001".to_string(),
            },
            "owner-verify",
        )
        .await
        .unwrap();
    assert_eq!(replayed_verification, owner_verification);
    assert!(restarted_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: owner.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(owner_secret),
                otp: "99999999".to_string(),
            },
            "owner-verify",
        )
        .await
        .is_err());
    assert!(restarted_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: owner.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(owner_secret),
                otp: "00000001".to_string(),
            },
            "owner-verify-different-key",
        )
        .await
        .is_err());
    let reservation_owner: Uuid =
        query::<Postgres>("SELECT challenge_id FROM email_registration_reservations")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("challenge_id")
            .unwrap();
    assert_eq!(reservation_owner, owner.request_id);
    assert_ne!(reservation_owner, attacker.request_id);
}

#[tokio::test]
async fn concurrent_canonical_delivery_suppression_returns_same_shape_and_stays_bounded() {
    let test = setup().await;
    let service = manual_email_service();
    let mut requests = tokio::task::JoinSet::new();
    for index in 0_u8..20 {
        let service = service.clone();
        let pool = test.application_pool.clone();
        requests.spawn(async move {
            service
                .request_registration(
                    &pool,
                    RegistrationRequest {
                        email: "Cooldown@BÜCHER.example".to_string(),
                        handoff_challenge: URL_SAFE_NO_PAD.encode([index; 32]),
                    },
                    &format!("cooldown-request-{index}"),
                )
                .await
        });
    }
    let mut responses = Vec::new();
    while let Some(result) = requests.join_next().await {
        responses.push(result.unwrap().unwrap());
    }
    assert_eq!(responses.len(), 20);
    assert!(responses
        .iter()
        .all(|response| response.expires_at > Utc::now()));
    let outbox: i64 = query::<Postgres>("SELECT count(*) AS count FROM email_delivery_outbox")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(outbox, 1);
    let active: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_registration_challenges")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(active, 20);
    let global_capacity: i32 = query::<Postgres>(
        "SELECT active_count FROM email_registration_global_capacity WHERE singleton = TRUE",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("active_count")
    .unwrap();
    let identifier_capacity: i16 =
        query::<Postgres>("SELECT active_count FROM email_registration_identifier_capacity")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("active_count")
            .unwrap();
    assert_eq!((global_capacity, identifier_capacity), (20, 4));
}

#[tokio::test]
async fn concurrent_verified_registrations_create_exactly_one_account() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let first = AccountClient::new(&server_url).unwrap();
    let second = AccountClient::new(&server_url).unwrap();
    let first_mailbox = first
        .begin_registration("parallel-owner@example.com")
        .await
        .unwrap();
    let second_mailbox = second
        .begin_registration("parallel-owner@example.com")
        .await
        .unwrap();
    let (first_verified, second_verified) = tokio::join!(
        first.verify_registration_otp(&first_mailbox, "00000001"),
        second.verify_registration_otp(&second_mailbox, "00000001")
    );
    let first_prepared = first
        .prepare_registration(
            &first_verified.unwrap(),
            "correct horse battery staple",
            Some("first"),
            &[0x61; 32],
        )
        .await
        .unwrap();
    let second_prepared = second
        .prepare_registration(
            &second_verified.unwrap(),
            "correct horse battery staple",
            Some("second"),
            &[0x62; 32],
        )
        .await
        .unwrap();
    let (first_result, second_result) = tokio::join!(
        first.finish_registration(&first_prepared),
        second.finish_registration(&second_prepared)
    );
    assert_eq!(
        first_result.is_ok() as usize + second_result.is_ok() as usize,
        1
    );
    let registered = first_result
        .as_ref()
        .ok()
        .or_else(|| second_result.as_ref().ok())
        .unwrap();
    let users: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM users
         WHERE canonical_email = 'parallel-owner@example.com'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(users, 1);
    let replay_rows: i64 = query::<Postgres>(
        "SELECT count(*) AS count
         FROM registration_finish_idempotency
         WHERE response_ciphertext IS NOT NULL",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(replay_rows, 1);
    let families: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM session_families
         WHERE user_id = $1",
    )
    .bind(Uuid::parse_str(&registered.session.user_id).unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(families, 1);
}

#[tokio::test]
async fn registration_finish_replays_the_encrypted_response_after_response_loss() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = AccountClient::new(&server_url).unwrap();
    let mailbox = client
        .begin_registration("finish-replay@example.com")
        .await
        .unwrap();
    let verified = client
        .verify_registration_otp(&mailbox, "00000001")
        .await
        .unwrap();
    let prepared = client
        .prepare_registration(
            &verified,
            "correct horse battery staple",
            Some("replay"),
            &[0x63; 32],
        )
        .await
        .unwrap();
    assert!(matches!(
        client.reconcile_registration(&prepared).await.unwrap(),
        AccountRegistrationReconcile::Pending
    ));
    let first = client.finish_registration(&prepared).await.unwrap();
    let reconciled = client.reconcile_registration(&prepared).await.unwrap();
    let AccountRegistrationReconcile::Committed(reconciled) = reconciled else {
        panic!("committed registration must be reconcilable");
    };
    assert_eq!(reconciled.session.user_id, first.session.user_id);
    let replay = client.finish_registration(&prepared).await.unwrap();
    assert_eq!(replay.session.user_id, first.session.user_id);
    assert_eq!(
        replay.session.tokens.access_token.as_str(),
        first.session.tokens.access_token.as_str()
    );
    assert_eq!(
        replay.session.tokens.refresh_token.as_str(),
        first.session.tokens.refresh_token.as_str()
    );
    let users: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM users
         WHERE canonical_email = 'finish-replay@example.com'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(users, 1);
    let replay_rows: i64 = query::<Postgres>(
        "SELECT count(*) AS count
         FROM registration_finish_idempotency
         WHERE response_ciphertext IS NOT NULL",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(replay_rows, 1);
    let families: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM session_families
         WHERE user_id = $1",
    )
    .bind(Uuid::parse_str(&first.session.user_id).unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(families, 1);
}

#[tokio::test]
async fn registration_status_waits_for_the_matching_finish_transaction() {
    let test = setup().await;
    let service = manual_email_service();
    let request_id = Uuid::now_v7();
    let handoff_secret = [0x3a; 32];
    let handoff_digest: [u8; 32] = Sha256::digest(handoff_secret).into();
    let start_key = Uuid::now_v7().to_string();
    let finish_key = Uuid::now_v7().to_string();
    let start_digest: [u8; 32] = Sha256::digest(start_key.as_bytes()).into();
    let finish_digest: [u8; 32] = Sha256::digest(finish_key.as_bytes()).into();
    query::<Postgres>(
        "INSERT INTO registration_reconciliation_authorizations
            (challenge_id, handoff_challenge, start_idempotency_key_digest, expires_at)
         VALUES ($1, $2, $3, now() + interval '1 hour')",
    )
    .bind(request_id)
    .bind(handoff_digest.as_slice())
    .bind(start_digest.as_slice())
    .execute(&test.application_pool)
    .await
    .unwrap();

    let mut finish_tx = test.application_pool.begin().await.unwrap();
    query::<Postgres>(
        "SELECT pg_advisory_xact_lock(
            hashtextextended(encode($1::bytea, 'hex'), 1)
         )",
    )
    .bind(finish_digest.as_slice())
    .execute(&mut *finish_tx)
    .await
    .unwrap();

    let pool = test.application_pool.clone();
    let status = tokio::spawn(async move {
        service
            .registration_status(
                &pool,
                RegistrationStatusRequest {
                    request_id,
                    handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                    start_idempotency_key: start_key,
                    finish_idempotency_key: finish_key,
                },
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !status.is_finished(),
        "status must not race a matching finish transaction"
    );
    finish_tx.commit().await.unwrap();
    let response = status.await.unwrap().unwrap();
    assert_eq!(response.status, "pending");
    assert!(response.result.is_none());
}

#[tokio::test]
async fn email_request_resend_budget_and_delivery_retry_are_atomic() {
    let test = setup().await;
    let service = EmailVerificationService::new(EmailVerificationConfig {
        token_key_current_version: 1,
        token_key_current: [0x41; 32],
        token_key_previous: None,
        state_key_current_version: 1,
        state_key_current: [0x42; 32],
        state_key_previous: None,
        delivery_key_current_version: 1,
        delivery_key_current: [0x45; 32],
        delivery_key_previous: None,
        delivery_endpoint: "http://127.0.0.1:1/v1/enqueue".to_string(),
        delivery_signing_key_id: "test-v1".to_string(),
        delivery_signing_key: [0x43; 32],
        dispatch_trigger_key: [0x44; 32],
    })
    .unwrap();
    let handoff_secret = [0x51_u8; 32];
    let handoff_challenge: [u8; 32] = Sha256::digest(handoff_secret).into();
    let second_secret = [0x52_u8; 32];
    let second_challenge: [u8; 32] = Sha256::digest(second_secret).into();
    let (first, second) = tokio::join!(
        service.request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "Case@BÜCHER.example".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(handoff_challenge),
            },
            "case-request-one",
        ),
        service.request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "Case@xn--bcher-kva.example".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(second_challenge),
            },
            "case-request-two",
        )
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(second.request_id, first.request_id);
    let suppressed = service
        .resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
            },
            "suppressed-resend",
        )
        .await
        .unwrap();
    assert_eq!(suppressed.request_id, first.request_id);
    assert_eq!(suppressed.next_retry_at, first.next_retry_at);
    let durable_next_retry_at: chrono::DateTime<Utc> =
        query::<Postgres>("SELECT next_retry_at FROM email_registration_challenges WHERE id = $1")
            .bind(first.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("next_retry_at")
            .unwrap();
    assert_eq!(durable_next_retry_at, suppressed.next_retry_at);

    let reservation_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_registration_reservations")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(reservation_count, 0);
    let outbox_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_delivery_outbox")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(outbox_count, 1);
    let generation: i32 =
        query::<Postgres>("SELECT generation FROM email_registration_challenges WHERE id = $1")
            .bind(first.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("generation")
            .unwrap();
    assert_eq!(generation, 1);

    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET last_delivery_at = now() - interval '2 minutes',
             next_retry_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(first.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_delivery_limits
         SET last_delivery_at = now() - interval '2 minutes'",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    let (resent, concurrent_suppressed) = tokio::join!(
        service.resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
            },
            "concurrent-resend-one",
        ),
        service.resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
            },
            "concurrent-resend-two",
        )
    );
    let resent = resent.unwrap();
    concurrent_suppressed.unwrap();
    assert_eq!(resent.request_id, first.request_id);
    for _ in 0..2 {
        query::<Postgres>(
            "UPDATE email_registration_challenges
             SET last_delivery_at = now() - interval '2 minutes',
                 next_retry_at = now() - interval '1 second'
             WHERE id = $1",
        )
        .bind(first.request_id)
        .execute(&test.pool)
        .await
        .unwrap();
        query::<Postgres>(
            "UPDATE email_registration_delivery_limits
             SET last_delivery_at = now() - interval '2 minutes'",
        )
        .execute(&test.pool)
        .await
        .unwrap();
        service
            .resend_registration(
                &test.application_pool,
                RegistrationResendRequest {
                    request_id: first.request_id,
                    handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                },
                &Uuid::now_v7().to_string(),
            )
            .await
            .unwrap();
    }
    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET last_delivery_at = now() - interval '2 minutes',
             next_retry_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(first.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    service
        .resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
            },
            "bounded-resend",
        )
        .await
        .unwrap();
    let bounded_generation: i32 =
        query::<Postgres>("SELECT generation FROM email_registration_challenges WHERE id = $1")
            .bind(first.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("generation")
            .unwrap();
    assert_eq!(bounded_generation, 4);

    assert!(service
        .resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode([0x99_u8; 32]),
            },
            "wrong-handoff-resend",
        )
        .await
        .is_err());
    let failed_attempts: i16 = query::<Postgres>(
        "SELECT handoff_failed_attempts
         FROM email_registration_challenges WHERE id = $1",
    )
    .bind(first.request_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("handoff_failed_attempts")
    .unwrap();
    assert_eq!(failed_attempts, 1);

    let summary = service
        .dispatch_email_batch(&test.application_pool)
        .await
        .unwrap();
    assert_eq!(summary.claimed, 4);
    assert_eq!(summary.retryable, 4);
    let attempted: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM email_delivery_outbox
         WHERE attempt_count = 1 AND accepted_at IS NULL AND terminal_at IS NULL
           AND encrypted_command IS NOT NULL",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(attempted, 4);
    let users: i64 = query::<Postgres>("SELECT count(*) AS count FROM users")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(users, 0);

    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET expires_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(second.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .cleanup_expired_registration_state(&test.application_pool)
            .await
            .unwrap(),
        1
    );
    let expired_graph: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM email_registration_challenges WHERE id = $1",
    )
    .bind(second.request_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(expired_graph, 0);
}

#[tokio::test]
async fn rotated_resend_promotes_and_releases_the_challenge_stored_digest() {
    let test = setup().await;
    let arbitrary_promotion: bool =
        query::<Postgres>("SELECT taskveil_promote_email_registration_capacity($1)")
            .bind(Uuid::now_v7())
            .fetch_one(&test.application_pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert!(!arbitrary_promotion);
    let capacity_rows: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_registration_identifier_capacity")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(capacity_rows, 0);

    let old_service = manual_email_service();
    let handoff_secret = [0x5f_u8; 32];
    let requested = old_service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "rotation@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(handoff_secret)),
            },
            "rotation-request",
        )
        .await
        .unwrap();
    let old_digest: Vec<u8> = query::<Postgres>(
        "SELECT canonical_email_digest
         FROM email_registration_challenges WHERE id = $1",
    )
    .bind(requested.request_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("canonical_email_digest")
    .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET capacity_claimed = FALSE, next_retry_at = now() - interval '1 second',
             last_delivery_at = NULL
         WHERE id = $1",
    )
    .bind(requested.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    query::<Postgres>(
        "DELETE FROM email_registration_identifier_capacity
         WHERE canonical_email_digest = $1 AND active_count = 1",
    )
    .bind(&old_digest)
    .execute(&test.pool)
    .await
    .unwrap();

    let rotated_service =
        EmailVerificationService::for_tests_with_config(EmailVerificationConfig {
            token_key_current_version: 2,
            token_key_current: [0x76; 32],
            token_key_previous: Some((1, [0x71; 32])),
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
        });
    rotated_service
        .resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: requested.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
            },
            "rotation-resend",
        )
        .await
        .unwrap();

    let claimed_digests = query::<Postgres>(
        "SELECT canonical_email_digest, active_count
         FROM email_registration_identifier_capacity",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(claimed_digests.len(), 1);
    assert_eq!(
        claimed_digests[0]
            .try_get::<Vec<u8>, _>("canonical_email_digest")
            .unwrap(),
        old_digest
    );
    assert_eq!(
        claimed_digests[0]
            .try_get::<i16, _>("active_count")
            .unwrap(),
        1
    );

    let second_secret = [0x60_u8; 32];
    let second = rotated_service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "rotation@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(second_secret)),
            },
            "rotation-second-request",
        )
        .await
        .unwrap();
    rotated_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: requested.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                otp: "00000001".to_string(),
            },
            "rotation-first-verify",
        )
        .await
        .unwrap();
    rotated_service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: second.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
                otp: "00000001".to_string(),
            },
            "rotation-second-verify",
        )
        .await
        .unwrap();
    let second_is_decoy: bool =
        query::<Postgres>("SELECT is_decoy FROM email_registration_challenges WHERE id = $1")
            .bind(second.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("is_decoy")
            .unwrap();
    assert!(second_is_decoy);

    let missing_previous =
        EmailVerificationService::for_tests_with_config(EmailVerificationConfig {
            token_key_current_version: 2,
            token_key_current: [0x76; 32],
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
        });
    assert!(missing_previous
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "unrelated@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest([0x63_u8; 32])),
            },
            "rotation-missing-previous",
        )
        .await
        .is_err());

    query::<Postgres>("DELETE FROM email_registration_challenges WHERE id = $1")
        .bind(requested.request_id)
        .execute(&test.pool)
        .await
        .unwrap();
    query::<Postgres>("DELETE FROM email_registration_challenges WHERE id = $1")
        .bind(second.request_id)
        .execute(&test.pool)
        .await
        .unwrap();
    let remaining: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM email_registration_identifier_capacity")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(remaining, 0);
    query::<Postgres>("DELETE FROM email_registration_delivery_limits")
        .execute(&test.pool)
        .await
        .unwrap();
    missing_previous
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "rotation-clean@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest([0x65_u8; 32])),
            },
            "rotation-after-cleanup",
        )
        .await
        .expect("removing every old-version row permits previous-key retirement");
}

#[tokio::test]
async fn expired_verified_ticket_releases_reservation_for_immediate_reregistration() {
    let test = setup().await;
    let service = EmailVerificationService::for_tests();
    let first_secret = [0x61_u8; 32];
    let first = service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "retry-after-ticket@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(first_secret)),
            },
            "ticket-expiry-first-request",
        )
        .await
        .unwrap();
    service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: first.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(first_secret),
                otp: "00000001".to_string(),
            },
            "ticket-expiry-first-verify",
        )
        .await
        .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET ticket_expires_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(first.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_reservations
         SET expires_at = now() - interval '1 second'
         WHERE challenge_id = $1",
    )
    .bind(first.request_id)
    .execute(&test.pool)
    .await
    .unwrap();

    let second_secret = [0x62_u8; 32];
    let second = service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "retry-after-ticket@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(second_secret)),
            },
            "ticket-expiry-second-request",
        )
        .await
        .unwrap();
    service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: second.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
                otp: "00000001".to_string(),
            },
            "ticket-expiry-second-verify",
        )
        .await
        .unwrap();
    let is_decoy: bool =
        query::<Postgres>("SELECT is_decoy FROM email_registration_challenges WHERE id = $1")
            .bind(second.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("is_decoy")
            .unwrap();
    assert!(!is_decoy);
}

#[tokio::test]
async fn concurrent_identical_verify_replays_one_committed_ticket() {
    let test = setup().await;
    let service = EmailVerificationService::for_tests();
    let handoff_secret = [0x64_u8; 32];
    let requested = service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "concurrent-verify@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(handoff_secret)),
            },
            "concurrent-verify-request",
        )
        .await
        .unwrap();
    let verify = || {
        service.verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: requested.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                otp: "00000001".to_string(),
            },
            "concurrent-verify-key",
        )
    };
    let (first, second) = tokio::join!(verify(), verify());
    assert_eq!(first.unwrap(), second.unwrap());
    let rows: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM registration_verify_idempotency
         WHERE challenge_id = $1",
    )
    .bind(requested.request_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn otp_attempt_limit_resend_rotation_and_response_replay_are_enforced() {
    let test = setup().await;
    let service = EmailVerificationService::for_tests();
    let handoff_secret = [0x81_u8; 32];
    let handoff_challenge: [u8; 32] = Sha256::digest(handoff_secret).into();
    let requested = service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "otp-limit@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(handoff_challenge),
            },
            "otp-limit-request",
        )
        .await
        .unwrap();

    for attempt in 0..5 {
        let idempotency_key = format!("otp-limit-invalid-{attempt}");
        assert!(service
            .verify_registration(
                &test.application_pool,
                RegistrationVerifyRequest {
                    request_id: requested.request_id,
                    handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                    otp: "99999999".to_string(),
                },
                &idempotency_key,
            )
            .await
            .is_err());
        if attempt == 0 {
            assert!(service
                .verify_registration(
                    &test.application_pool,
                    RegistrationVerifyRequest {
                        request_id: requested.request_id,
                        handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                        otp: "99999999".to_string(),
                    },
                    &idempotency_key,
                )
                .await
                .is_err());
            let replayed_attempts: i16 = query::<Postgres>(
                "SELECT otp_failed_attempts
                 FROM email_registration_challenges WHERE id = $1",
            )
            .bind(requested.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("otp_failed_attempts")
            .unwrap();
            assert_eq!(replayed_attempts, 1);
        }
    }
    assert!(service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: requested.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(handoff_secret),
                otp: "00000001".to_string(),
            },
            "otp-limit-valid",
        )
        .await
        .is_err());
    let attempts: i16 = query::<Postgres>(
        "SELECT otp_failed_attempts
         FROM email_registration_challenges WHERE id = $1",
    )
    .bind(requested.request_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("otp_failed_attempts")
    .unwrap();
    assert_eq!(attempts, 5);

    let second_secret = [0x82_u8; 32];
    let second_challenge: [u8; 32] = Sha256::digest(second_secret).into();
    let second = service
        .request_registration(
            &test.application_pool,
            RegistrationRequest {
                email: "otp-rotate@example.com".to_string(),
                handoff_challenge: URL_SAFE_NO_PAD.encode(second_challenge),
            },
            "otp-rotate-request",
        )
        .await
        .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_challenges
         SET last_delivery_at = now() - interval '2 minutes',
             next_retry_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(second.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    query::<Postgres>(
        "UPDATE email_registration_delivery_limits
         SET last_delivery_at = now() - interval '2 minutes'
         WHERE canonical_email_digest = (
             SELECT canonical_email_digest
             FROM email_registration_challenges WHERE id = $1
         )",
    )
    .bind(second.request_id)
    .execute(&test.pool)
    .await
    .unwrap();
    let resend_request = RegistrationResendRequest {
        request_id: second.request_id,
        handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
    };
    let resent = service
        .resend_registration(&test.application_pool, resend_request, "otp-rotate-resend")
        .await
        .unwrap();
    let replay = service
        .resend_registration(
            &test.application_pool,
            RegistrationResendRequest {
                request_id: second.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
            },
            "otp-rotate-resend",
        )
        .await
        .unwrap();
    assert_eq!(replay, resent);
    let generation: i32 =
        query::<Postgres>("SELECT generation FROM email_registration_challenges WHERE id = $1")
            .bind(second.request_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("generation")
            .unwrap();
    assert_eq!(generation, 2);
    assert!(service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: second.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
                otp: "00000001".to_string(),
            },
            "resend-old-verify",
        )
        .await
        .is_err());
    service
        .verify_registration(
            &test.application_pool,
            RegistrationVerifyRequest {
                request_id: second.request_id,
                handoff_secret: URL_SAFE_NO_PAD.encode(second_secret),
                otp: "00000002".to_string(),
            },
            "resend-current-verify",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn suppressed_request_never_evicts_an_existing_challenge() {
    let test = setup().await;
    let service = EmailVerificationService::for_tests();
    let handoff_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest([0x91_u8; 32]));
    let mut request_ids = Vec::new();
    for index in 0..5 {
        let response = service
            .request_registration(
                &test.application_pool,
                RegistrationRequest {
                    email: "capacity@example.com".to_string(),
                    handoff_challenge: handoff_challenge.clone(),
                },
                &format!("capacity-request-{index}"),
            )
            .await
            .unwrap();
        request_ids.push(response.request_id);
    }
    let existing: i64 = query::<Postgres>(
        "SELECT count(*) AS count
         FROM email_registration_challenges
         WHERE id = ANY($1)",
    )
    .bind(&request_ids)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(existing, 5);
    let claimed: i64 = query::<Postgres>(
        "SELECT count(*) AS count
         FROM email_registration_challenges
         WHERE id = ANY($1) AND capacity_claimed = TRUE",
    )
    .bind(&request_ids)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(claimed, 4);
}

#[tokio::test]
async fn account_register_login_refresh_reuse_and_revocation_are_enforced() {
    let test = setup().await;
    let health = test
        .app
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let ready = test
        .app
        .clone()
        .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        request_status(
            &test.app,
            Method::POST,
            "/v1/auth/register/request".to_string(),
            None,
            Some(serde_json::json!({
                "email": "pending@example.com",
                "handoff_challenge": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            })),
        )
        .await,
        StatusCode::ACCEPTED
    );
    let users_before_opaque: i64 = query::<Postgres>("SELECT count(*) AS count FROM users")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(users_before_opaque, 0);
    assert_eq!(
        request_status(
            &test.app,
            Method::POST,
            "/v1/auth/register/start".to_string(),
            None,
            Some(serde_json::json!({
                "email": "downgrade@example.com",
                "device_name": "downgrade",
                "opaque_suite_id": 1,
                "message": "invalid-but-suite-is-checked-first"
            })),
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = AccountClient::new(&server_url).unwrap();
    let metadata = reqwest::get(format!(
        "{server_url}/.well-known/oauth-authorization-server"
    ))
    .await
    .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);
    let metadata: Value = metadata.json().await.unwrap();
    assert_eq!(metadata["issuer"], "http://localhost");
    assert_eq!(metadata["token_endpoint"], "http://localhost/v1/auth/token");
    assert_eq!(
        metadata["revocation_endpoint"],
        "http://localhost/v1/auth/revoke"
    );
    let registered = staged_register(
        &client,
        "account-v2@example.com",
        "correct horse battery staple",
        Some("first device"),
        &[0x51; 32],
        "00000001",
    )
    .await
    .unwrap();
    let now_ms = Utc::now().timestamp_millis();
    assert_eq!(registered.session.tokens.access_token.len(), 43);
    assert_eq!(registered.session.tokens.refresh_token.len(), 43);
    assert!(
        registered.session.tokens.access_expires_at_ms > now_ms + 14 * 60 * 1_000
            && registered.session.tokens.access_expires_at_ms <= now_ms + 15 * 60 * 1_000
    );
    assert!(
        registered.session.tokens.refresh_expires_at_ms > now_ms + 29 * 24 * 60 * 60 * 1_000
            && registered.session.tokens.refresh_expires_at_ms
                <= now_ms + 30 * 24 * 60 * 60 * 1_000
    );
    assert_eq!(registered.recovery_key.split_whitespace().count(), 24);
    let reset_marker_count: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM taskveil_pre_release_resets
         WHERE reset_key = 'email-verification-stable-opaque-credential-v1'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(reset_marker_count, 1);
    db::run_migrations(&test.pool)
        .await
        .expect("re-running the migrator must not reset registered accounts");
    let registered_user_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM users")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(registered_user_count, 1);
    assert!(staged_register(
        &client,
        "account-v2@example.com",
        "correct horse battery staple",
        Some("duplicate device"),
        &[0x52; 32],
        "00000002",
    )
    .await
    .is_err());

    let user_id = Uuid::parse_str(&registered.session.user_id).unwrap();
    let tenant_id = Uuid::parse_str(&registered.session.tenant_id).unwrap();
    let tenant = query::<Postgres>(
        "SELECT kind, owner_user_id,
                (SELECT count(*) FROM tenant_members WHERE tenant_id = tenants.id) AS member_count,
                (SELECT count(*) FROM tenant_members
                 WHERE tenant_id = tenants.id AND user_id = $2 AND role = 'owner') AS owner_count
         FROM tenants WHERE id = $1",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(tenant.try_get::<String, _>("kind").unwrap(), "personal");
    assert_eq!(tenant.try_get::<Uuid, _>("owner_user_id").unwrap(), user_id);
    assert_eq!(tenant.try_get::<i64, _>("member_count").unwrap(), 1);
    assert_eq!(tenant.try_get::<i64, _>("owner_count").unwrap(), 1);
    let stored = stored_key_bundle(&test.pool, user_id, tenant_id).await;
    assert!(unwrap_login_key_bundle(&stored, user_id, tenant_id, b"wrong export key").is_err());

    let logged_in = client
        .begin_login(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("second device"),
            &[0x53; 32],
        )
        .await
        .unwrap();
    client.certify_login(&logged_in).await.unwrap();
    client
        .certify_login(&logged_in)
        .await
        .expect("device certification retry is idempotent");
    assert_eq!(*registered.keys.master_key, *logged_in.keys.master_key);
    assert_eq!(
        registered.keys.account_root_public,
        logged_in.keys.account_root_public
    );
    assert_eq!(
        *registered.keys.tenant_root_dek,
        *logged_in.keys.tenant_root_dek
    );
    assert!(client
        .begin_login(
            "account-v2@example.com",
            "wrong password",
            Some("wrong device"),
            &[0x54; 32],
        )
        .await
        .is_err());

    let original_refresh = logged_in.session.tokens.refresh_token.to_string();
    assert_eq!(
        request_status(
            &test.app,
            Method::GET,
            format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
            Some(&original_refresh),
            None,
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(matches!(
        client.refresh(&logged_in.session.tokens.access_token).await,
        Err(AccountClientError::InvalidGrant)
    ));
    let near_absolute_now_ms = Utc::now().timestamp_millis();
    query::<Postgres>(
        "UPDATE session_families
         SET absolute_expires_at = now() + interval '5 minutes'
         WHERE device_id = $1",
    )
    .bind(Uuid::parse_str(&logged_in.session.device_id).unwrap())
    .execute(&test.pool)
    .await
    .unwrap();
    let rotated = reqwest::Client::new()
        .post(format!("{server_url}/v1/auth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", original_refresh.as_str()),
            ("client_id", "taskveil-native"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated: Value = rotated.json().await.unwrap();
    let rotated_expires_in = rotated["expires_in"].as_u64().unwrap();
    assert!((240..=300).contains(&rotated_expires_in));
    let rotated_access_expires_at =
        chrono::DateTime::parse_from_rfc3339(rotated["access_expires_at"].as_str().unwrap())
            .unwrap()
            .timestamp_millis();
    assert!(rotated_access_expires_at > near_absolute_now_ms + 4 * 60 * 1_000);
    assert!(rotated_access_expires_at <= near_absolute_now_ms + 5 * 60 * 1_000 + 5_000);
    let rotated_refresh = rotated["refresh_token"].as_str().unwrap();
    let rotated_access = rotated["access_token"].as_str().unwrap();
    assert!(rotated_refresh != original_refresh);
    assert!(rotated_access != logged_in.session.tokens.access_token.as_str());
    assert_eq!(
        request_status(
            &test.app,
            Method::GET,
            format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
            Some(rotated_access),
            None,
        )
        .await,
        StatusCode::PAYMENT_REQUIRED
    );
    assert!(matches!(
        client.refresh(&original_refresh).await,
        Err(AccountClientError::InvalidGrant)
    ));
    assert_eq!(
        request_status(
            &test.app,
            Method::GET,
            format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
            Some(rotated_access),
            None,
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(matches!(
        client.refresh(rotated_refresh).await,
        Err(AccountClientError::InvalidGrant)
    ));

    client.logout("unknown-token").await.unwrap();
    client
        .logout(&registered.session.tokens.refresh_token)
        .await
        .unwrap();
    assert_eq!(
        request_status(
            &test.app,
            Method::GET,
            format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
            Some(&registered.session.tokens.access_token),
            None,
        )
        .await,
        StatusCode::UNAUTHORIZED
    );

    let concurrent = client
        .begin_login(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("concurrent refresh device"),
            &[0x55; 32],
        )
        .await
        .unwrap();
    client.certify_login(&concurrent).await.unwrap();
    let concurrent_refresh = concurrent.session.tokens.refresh_token.to_string();
    let http = reqwest::Client::new();
    let refresh_request = || {
        http.post(format!("{server_url}/v1/auth/token")).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", concurrent_refresh.as_str()),
            ("client_id", "taskveil-native"),
        ])
    };
    let (first, second) = tokio::join!(refresh_request().send(), refresh_request().send());
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(
        (first.status() == StatusCode::OK && second.status() == StatusCode::BAD_REQUEST)
            || (second.status() == StatusCode::OK && first.status() == StatusCode::BAD_REQUEST)
    );
    let successful_rotation: Value = if first.status() == StatusCode::OK {
        first.json().await.unwrap()
    } else {
        second.json().await.unwrap()
    };
    assert_eq!(
        request_status(
            &test.app,
            Method::GET,
            format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
            successful_rotation["access_token"].as_str(),
            None,
        )
        .await,
        StatusCode::UNAUTHORIZED
    );

    let refresh_revoke = client
        .begin_login(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("refresh revoke race device"),
            &[0x56; 32],
        )
        .await
        .unwrap();
    client.certify_login(&refresh_revoke).await.unwrap();
    let refresh_revoke_token = refresh_revoke.session.tokens.refresh_token.to_string();
    let (refresh_result, revoke_result) = tokio::join!(
        client.refresh(&refresh_revoke_token),
        client.logout(&refresh_revoke_token)
    );
    revoke_result.unwrap();
    if let Ok(tokens) = refresh_result {
        assert_eq!(
            request_status(
                &test.app,
                Method::GET,
                format!("/v2/tenants/{tenant_id}/pull?since=0&limit=1"),
                Some(&tokens.access_token),
                None,
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(matches!(
        client.refresh(&refresh_revoke_token).await,
        Err(AccountClientError::InvalidGrant)
    ));

    let abandoned = client
        .begin_login(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("abandoned provisional device"),
            &[0x57; 32],
        )
        .await
        .unwrap();
    let abandoned_device_id = Uuid::parse_str(&abandoned.session.device_id).unwrap();
    let device_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM devices WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(device_count, 5);
    let certified_device_count: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM devices
         WHERE user_id = $1 AND certificate IS NOT NULL AND certified_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(certified_device_count, 4);
    let revoked_device_count: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM devices WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(revoked_device_count, 0);
    let refresh_reuse_families: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM session_families
         WHERE user_id = $1 AND revocation_reason = 'refresh_reuse'",
    )
    .bind(user_id)
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(refresh_reuse_families, 2);
    query::<Postgres>(
        "UPDATE devices
         SET enrollment_challenge_expires_at = now() - interval '1 second'
         WHERE id = $1",
    )
    .bind(abandoned_device_id)
    .execute(&test.pool)
    .await
    .unwrap();
    for _ in 0..4 {
        if query::<Postgres>("SELECT count(*) AS count FROM devices WHERE id = $1")
            .bind(abandoned_device_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get::<i64, _>("count")
            .unwrap()
            == 0
        {
            break;
        }
        assert!(auth::cleanup_expired_auth_state(&test.pool).await.unwrap() > 0);
    }
    let abandoned_device_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM devices WHERE id = $1")
            .bind(abandoned_device_id)
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(abandoned_device_count, 0);
    query::<Postgres>(
        "UPDATE access_tokens
         SET expires_at = now() - interval '1 second'
         WHERE family_id IN (
             SELECT id FROM session_families WHERE device_id = $1
         )",
    )
    .bind(Uuid::parse_str(&registered.session.device_id).unwrap())
    .execute(&test.pool)
    .await
    .unwrap();
    let registered_family_id: Uuid =
        query::<Postgres>("SELECT id FROM session_families WHERE device_id = $1")
            .bind(Uuid::parse_str(&registered.session.device_id).unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("id")
            .unwrap();
    for index in 0u64..130 {
        let mut token_hash = vec![0xA5; 32];
        token_hash[..8].copy_from_slice(&index.to_be_bytes());
        query::<Postgres>(
            "INSERT INTO access_tokens (id, family_id, token_hash, expires_at)
             VALUES ($1, $2, $3, now() - interval '1 second')",
        )
        .bind(Uuid::now_v7())
        .bind(registered_family_id)
        .bind(token_hash)
        .execute(&test.pool)
        .await
        .unwrap();
    }
    assert_eq!(
        auth::cleanup_expired_auth_state(&test.pool).await.unwrap(),
        128
    );
    assert_eq!(
        auth::cleanup_expired_auth_state(&test.pool).await.unwrap(),
        3
    );
    query::<Postgres>(
        "UPDATE session_families
         SET absolute_expires_at = now() - interval '1 second'
         WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&test.pool)
    .await
    .unwrap();
    for _ in 0..4 {
        let family_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM session_families")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
        if family_count == 0 {
            break;
        }
        assert!(auth::cleanup_expired_auth_state(&test.pool).await.unwrap() > 0);
    }
    let remaining_access_tokens: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM access_tokens")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    let remaining_refresh_tokens: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM refresh_tokens")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    let remaining_session_families: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM session_families")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(remaining_access_tokens, 0);
    assert_eq!(remaining_refresh_tokens, 0);
    assert_eq!(remaining_session_families, 0);
    let obsolete_public_key_columns: i64 = query::<Postgres>(
        "SELECT count(*) AS count FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND table_name = 'devices'
           AND column_name = 'public_key'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(obsolete_public_key_columns, 0);
    assert_opaque_capacity_matches_states(&test.pool).await;
}

#[tokio::test]
async fn opaque_capacity_claims_rollback_serialize_and_cleanup_in_bounded_batches() {
    let test = setup().await;
    let direct_counter_write = query::<Postgres>(
        "UPDATE opaque_state_global_capacity SET active_count = 1 WHERE singleton = TRUE",
    )
    .execute(&test.application_pool)
    .await
    .expect_err("runtime role must not write capacity counters directly");
    assert_eq!(
        direct_counter_write
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    let function_privileges = query::<Postgres>(
        "SELECT
             bool_and(owner.rolname <> 'taskveil_app') AS owner_is_not_app,
             bool_and(NOT has_function_privilege(
                 'taskveil_app',
                 format('%I.%I()', namespace.nspname, function.proname),
                 'EXECUTE'
             )) AS app_cannot_execute
         FROM pg_proc function
         JOIN pg_namespace namespace ON namespace.oid = function.pronamespace
         JOIN pg_roles owner ON owner.oid = function.proowner
         WHERE namespace.nspname = 'public'
           AND function.proname IN (
               'taskveil_claim_opaque_state_capacity',
               'taskveil_release_opaque_state_capacity'
           )",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(function_privileges
        .try_get::<bool, _>("owner_is_not_app")
        .unwrap());
    assert!(function_privileges
        .try_get::<bool, _>("app_cannot_execute")
        .unwrap());
    raw_sql("DROP TRIGGER opaque_login_state_capacity_claim ON opaque_login_states")
        .execute(&test.application_pool)
        .await
        .expect_err("runtime role must not tamper with capacity triggers");

    let legacy_state_id = Uuid::now_v7();
    query::<Postgres>(
        "INSERT INTO opaque_login_states
            (id, user_id, tenant_id, device_id, device_challenge, device_name,
             opaque_suite_id, server_login_state, expires_at)
         VALUES ($1, NULLIF($2, $2), NULLIF($3, $3), $4, $5, 'legacy deployment', 2, decode('00', 'hex'),
                 now() + interval '10 minutes')",
    )
    .bind(legacy_state_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(vec![0x91u8; 32])
    .execute(&test.application_pool)
    .await
    .expect("old code insert without identifier_key must remain globally bounded");
    assert_eq!(global_opaque_capacity(&test.pool).await, 1);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 1);
    query::<Postgres>("DELETE FROM opaque_login_states WHERE id = $1")
        .bind(legacy_state_id)
        .execute(&test.application_pool)
        .await
        .expect("old code delete must release its database-owned lease");
    assert_eq!(global_opaque_capacity(&test.pool).await, 0);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 0);
    query::<Postgres>("UPDATE opaque_login_states SET identifier_key = $2 WHERE id = $1")
        .bind(Uuid::now_v7())
        .bind([0x95u8; 32].as_slice())
        .execute(&test.application_pool)
        .await
        .expect_err("runtime role must not mutate trigger-accounted state");

    let saturated_identifier = [0x61; 32];
    query::<Postgres>(
        "INSERT INTO opaque_state_identifier_capacity (identifier_key, active_count)
         VALUES ($1, 32)",
    )
    .bind(saturated_identifier.as_slice())
    .execute(&test.pool)
    .await
    .unwrap();

    let identifier_limited = auth::login_start(
        &test.application_pool,
        registration_start_request("identifier-limit@example.com"),
        &saturated_identifier,
    )
    .await
    .expect_err("identifier capacity must reject");
    let identifier_limited = identifier_limited.into_response();
    assert_eq!(identifier_limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(identifier_limited
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .is_none());
    assert_eq!(global_opaque_capacity(&test.pool).await, 0);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 0);
    query::<Postgres>("DELETE FROM opaque_state_identifier_capacity")
        .execute(&test.pool)
        .await
        .unwrap();

    let keyed_identifier = [0x65u8; 32];
    for index in 0..32 {
        query::<Postgres>(
            "INSERT INTO opaque_login_states
                (id, user_id, tenant_id, device_id, device_challenge, device_name,
                 opaque_suite_id, server_login_state, expires_at, identifier_key)
             VALUES ($1, NULLIF($2, $2), NULLIF($3, $3), $4, $5, $6, 2, decode('00', 'hex'),
                     now() + interval '10 minutes', $7)",
        )
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(vec![0x92u8; 32])
        .bind(format!("keyed-cap-{index}@example.com"))
        .bind(keyed_identifier.as_slice())
        .execute(&test.application_pool)
        .await
        .unwrap();
    }
    let keyed_cap = query::<Postgres>(
        "INSERT INTO opaque_login_states
            (id, user_id, tenant_id, device_id, device_challenge, device_name,
             opaque_suite_id, server_login_state, expires_at, identifier_key)
         VALUES ($1, NULLIF($2, $2), NULLIF($3, $3), $4, $5, 'keyed cap', 2, decode('00', 'hex'),
                 now() + interval '10 minutes', $6)",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(vec![0x93u8; 32])
    .bind(keyed_identifier.as_slice())
    .execute(&test.application_pool)
    .await
    .expect_err("database trigger must enforce the keyed cap");
    assert_eq!(
        keyed_cap
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("P0429")
    );
    query::<Postgres>("DELETE FROM opaque_login_states WHERE device_name LIKE 'keyed-cap-%'")
        .execute(&test.application_pool)
        .await
        .unwrap();
    assert_opaque_capacity_matches_states(&test.pool).await;

    raw_sql(
        "CREATE FUNCTION reject_opaque_login_state() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             RAISE EXCEPTION 'injected insert failure';
         END;
         $$;
         CREATE TRIGGER reject_opaque_login_state
         BEFORE INSERT ON opaque_login_states
         FOR EACH ROW EXECUTE FUNCTION reject_opaque_login_state();",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    let failed_insert = auth::login_start(
        &test.application_pool,
        registration_start_request("rollback@example.com"),
        &[0x62; 32],
    )
    .await
    .expect_err("injected state insert must fail");
    assert_eq!(
        failed_insert.into_response().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(global_opaque_capacity(&test.pool).await, 0);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 0);
    let identifier_capacity_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM opaque_state_identifier_capacity")
            .fetch_one(&test.pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(identifier_capacity_count, 0);
    raw_sql(
        "DROP TRIGGER reject_opaque_login_state ON opaque_login_states;
         DROP FUNCTION reject_opaque_login_state();",
    )
    .execute(&test.pool)
    .await
    .unwrap();

    raw_sql(
        "INSERT INTO opaque_login_states
            (id, user_id, tenant_id, device_id, device_challenge, device_name,
             opaque_suite_id, server_login_state, expires_at)
         SELECT
             lpad(to_hex(sequence), 32, '0')::uuid,
             NULL,
             NULL,
             '00000000-0000-0000-0000-000000000003'::uuid,
             decode(repeat('94', 32), 'hex'),
             'legacy concurrency fill',
             2,
             decode('00', 'hex'),
             now() + interval '10 minutes'
         FROM generate_series(1, 4095) AS sequence",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    assert_opaque_capacity_matches_states(&test.pool).await;
    let first = auth::login_start(
        &test.application_pool,
        registration_start_request("concurrent-a@example.com"),
        &[0x63; 32],
    );
    let second = auth::login_start(
        &test.application_pool,
        registration_start_request("concurrent-b@example.com"),
        &[0x64; 32],
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let rejected = if let Err(error) = first {
        error
    } else {
        second.expect_err("one concurrent claim must reject")
    };
    let rejected = rejected.into_response();
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(rejected
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .is_none());
    assert_eq!(global_opaque_capacity(&test.pool).await, 4096);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 4096);
    assert_opaque_capacity_matches_states(&test.pool).await;

    raw_sql(
        "DELETE FROM opaque_registration_states;
         DELETE FROM opaque_login_states;
         DELETE FROM opaque_state_capacity_leases;
         DELETE FROM opaque_state_identifier_capacity;
         UPDATE opaque_state_global_capacity SET active_count = 0 WHERE singleton = TRUE;",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    for index in 0u16..130 {
        let state_id = Uuid::now_v7();
        let mut identifier_key = [0u8; 32];
        identifier_key[..2].copy_from_slice(&index.to_be_bytes());
        query::<Postgres>(
            "INSERT INTO opaque_login_states
                (id, user_id, tenant_id, device_id, device_challenge, device_name,
                 opaque_suite_id, server_login_state, expires_at, identifier_key)
             VALUES ($1, NULLIF($2, $2), NULLIF($3, $3), $4, $5, $6, 2, decode('00', 'hex'),
                     now() - interval '1 second', $7)",
        )
        .bind(state_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(vec![0xA5u8; 32])
        .bind(format!("cleanup-{index}@example.com"))
        .bind(identifier_key.as_slice())
        .execute(&test.pool)
        .await
        .unwrap();
    }

    assert_eq!(
        auth::cleanup_expired_opaque_states(&test.application_pool)
            .await
            .unwrap(),
        128
    );
    assert_eq!(global_opaque_capacity(&test.pool).await, 2);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 2);
    assert_eq!(
        auth::cleanup_expired_opaque_states(&test.application_pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(global_opaque_capacity(&test.pool).await, 0);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 0);
    assert_eq!(
        auth::cleanup_expired_opaque_states(&test.application_pool)
            .await
            .unwrap(),
        0
    );

    let inconsistent_identifier = [0x7Au8; 32];
    for index in 0..2 {
        let state_id = Uuid::now_v7();
        query::<Postgres>(
            "INSERT INTO opaque_login_states
                (id, user_id, tenant_id, device_id, device_challenge, device_name,
                 opaque_suite_id, server_login_state, expires_at, identifier_key)
             VALUES ($1, NULLIF($2, $2), NULLIF($3, $3), $4, $5, $6, 2, decode('00', 'hex'),
                     now() - interval '1 second', $7)",
        )
        .bind(state_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(vec![0x5Au8; 32])
        .bind(format!("underflow-{index}@example.com"))
        .bind(inconsistent_identifier.as_slice())
        .execute(&test.pool)
        .await
        .unwrap();
    }
    query::<Postgres>(
        "UPDATE opaque_state_identifier_capacity
         SET active_count = 1 WHERE identifier_key = $1",
    )
    .bind(inconsistent_identifier.as_slice())
    .execute(&test.pool)
    .await
    .unwrap();

    let underflow = auth::cleanup_expired_opaque_states(&test.application_pool)
        .await
        .expect_err("counter underflow must fail closed");
    assert_eq!(
        underflow.into_response().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(global_opaque_capacity(&test.pool).await, 2);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 2);
    let state_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM opaque_login_states")
        .fetch_one(&test.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(state_count, 2);

    query::<Postgres>(
        "UPDATE opaque_state_identifier_capacity
         SET active_count = 2 WHERE identifier_key = $1",
    )
    .bind(inconsistent_identifier.as_slice())
    .execute(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        auth::cleanup_expired_opaque_states(&test.application_pool)
            .await
            .unwrap(),
        2
    );
    assert_opaque_capacity_matches_states(&test.pool).await;
}

fn registration_start_request(email: &str) -> auth::OpaqueStartRequest {
    let client_start =
        ClientLogin::<TaskveilCipherSuite>::start(&mut OsRng, b"test password").unwrap();
    auth::OpaqueStartRequest {
        email: email.to_string(),
        device_name: Some("capacity test".to_string()),
        opaque_suite_id: CRYPTO_SUITE_ID,
        message: STANDARD.encode(client_start.message.serialize()),
    }
}

async fn global_opaque_capacity(pool: &PgPool) -> i32 {
    query::<Postgres>(
        "SELECT active_count FROM opaque_state_global_capacity WHERE singleton = TRUE",
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .try_get("active_count")
    .unwrap()
}

async fn opaque_capacity_lease_count(pool: &PgPool) -> i64 {
    query::<Postgres>("SELECT count(*) AS count FROM opaque_state_capacity_leases")
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap()
}

async fn assert_opaque_capacity_matches_states(pool: &PgPool) {
    let row = query::<Postgres>(
        "SELECT
             (SELECT active_count FROM opaque_state_global_capacity
              WHERE singleton = TRUE) AS global_count,
             (SELECT count(*) FROM opaque_state_capacity_leases) AS lease_count,
             (SELECT count(*) FROM opaque_registration_states)
               + (SELECT count(*) FROM opaque_login_states) AS state_count,
             coalesce(
                 (SELECT sum(active_count) FROM opaque_state_identifier_capacity),
                 0
             ) AS identifier_count",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let global_count = i64::from(row.try_get::<i32, _>("global_count").unwrap());
    let lease_count = row.try_get::<i64, _>("lease_count").unwrap();
    let state_count = row.try_get::<i64, _>("state_count").unwrap();
    let identifier_count = row.try_get::<i64, _>("identifier_count").unwrap();
    assert_eq!(global_count, lease_count);
    assert_eq!(lease_count, state_count);
    assert_eq!(identifier_count, lease_count);
}

#[tokio::test]
async fn opaque_login_hides_account_existence_and_consumes_every_state_once() {
    let test = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let app = test.app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = AccountClient::new(&server_url).unwrap();
    staged_register(
        &client,
        "privacy@example.com",
        "correct horse battery staple",
        Some("registration device"),
        &[0x61; 32],
        "00000001",
    )
    .await
    .unwrap();

    let known = begin_raw_login(
        &server_url,
        "privacy@example.com",
        "correct horse battery staple",
    )
    .await;
    let unknown = begin_raw_login(
        &server_url,
        "unknown@example.com",
        "correct horse battery staple",
    )
    .await;

    assert_eq!(known.status, StatusCode::OK);
    assert_eq!(unknown.status, StatusCode::OK);
    let known_fields = response_fields(&known.start_body);
    let unknown_fields = response_fields(&unknown.start_body);
    assert_eq!(known_fields, unknown_fields);
    assert_eq!(
        known_fields,
        vec!["expires_at", "message", "opaque_suite_id", "state_id"]
    );
    assert_eq!(
        STANDARD
            .decode(known.start_body["message"].as_str().unwrap())
            .unwrap()
            .len(),
        STANDARD
            .decode(unknown.start_body["message"].as_str().unwrap())
            .unwrap()
            .len()
    );

    let unknown_identity =
        query::<Postgres>("SELECT user_id, tenant_id FROM opaque_login_states WHERE id = $1")
            .bind(unknown.state_id)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(
        unknown_identity
            .try_get::<Option<Uuid>, _>("user_id")
            .unwrap(),
        None
    );
    assert_eq!(
        unknown_identity
            .try_get::<Option<Uuid>, _>("tenant_id")
            .unwrap(),
        None
    );

    let successful = finish_raw_login(&server_url, &known).await;
    assert_eq!(successful.0, StatusCode::OK);
    assert!(successful.1.get("user_id").is_some());
    assert!(successful.1.get("tenant_id").is_some());
    assert!(successful.1.get("key_bundle").is_some());

    let replayed = finish_raw_login(&server_url, &known).await;

    let wrong_password =
        begin_raw_login(&server_url, "privacy@example.com", "wrong password").await;
    assert!(wrong_password.finalization.is_none());
    let wrong_password = finish_raw_login_with_message(
        &server_url,
        wrong_password.state_id,
        known.finalization.as_deref().unwrap(),
    )
    .await;

    assert!(unknown.finalization.is_none());
    let unknown = finish_raw_login_with_message(
        &server_url,
        unknown.state_id,
        known.finalization.as_deref().unwrap(),
    )
    .await;

    let expired = begin_raw_login(
        &server_url,
        "privacy@example.com",
        "correct horse battery staple",
    )
    .await;
    query::<Postgres>(
        "UPDATE opaque_login_states SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(expired.state_id)
    .execute(&test.pool)
    .await
    .unwrap();
    let expired = finish_raw_login(&server_url, &expired).await;

    let malformed = begin_raw_login(
        &server_url,
        "privacy@example.com",
        "correct horse battery staple",
    )
    .await;
    let malformed_attempt =
        finish_raw_login_with_message(&server_url, malformed.state_id, "not-base64").await;
    assert_eq!(malformed_attempt.0, StatusCode::BAD_REQUEST);
    let malformed_replayed = finish_raw_login(&server_url, &malformed).await;

    for failure in [
        &replayed,
        &wrong_password,
        &unknown,
        &expired,
        &malformed_replayed,
    ] {
        assert_eq!(failure.0, StatusCode::UNAUTHORIZED);
        assert_eq!(failure.1, serde_json::json!({"error": "unauthorized"}));
    }

    assert!(matches!(
        client
            .begin_login(
                "privacy@example.com",
                "wrong password",
                Some("wrong-password device"),
                &[0x62; 32],
            )
            .await,
        Err(AccountClientError::Opaque)
    ));
    assert!(matches!(
        client
            .begin_login(
                "unknown@example.com",
                "correct horse battery staple",
                Some("unknown-account device"),
                &[0x63; 32],
            )
            .await,
        Err(AccountClientError::Opaque)
    ));

    let regular_login = client
        .begin_login(
            "privacy@example.com",
            "correct horse battery staple",
            Some("normal login device"),
            &[0x64; 32],
        )
        .await
        .unwrap();
    client.certify_login(&regular_login).await.unwrap();
}

struct RawLogin {
    status: StatusCode,
    start_body: Value,
    state_id: Uuid,
    finalization: Option<String>,
}

async fn begin_raw_login(server_url: &str, email: &str, password: &str) -> RawLogin {
    let mut rng = OsRng;
    let client_start =
        ClientLogin::<TaskveilCipherSuite>::start(&mut rng, password.as_bytes()).unwrap();
    let response = reqwest::Client::new()
        .post(format!("{server_url}/v1/auth/login/start"))
        .json(&serde_json::json!({
            "email": email,
            "device_name": "privacy test device",
            "opaque_suite_id": CRYPTO_SUITE_ID,
            "message": STANDARD.encode(client_start.message.serialize())
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let start_body: Value = response.json().await.unwrap();
    let state_id = Uuid::parse_str(start_body["state_id"].as_str().unwrap()).unwrap();
    let server_message = CredentialResponse::<TaskveilCipherSuite>::deserialize(
        &STANDARD
            .decode(start_body["message"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let finalization = client_start
        .state
        .finish(
            &mut rng,
            password.as_bytes(),
            server_message,
            opaque_login_parameters(),
        )
        .ok()
        .map(|finish| STANDARD.encode(finish.message.serialize()));

    RawLogin {
        status,
        start_body,
        state_id,
        finalization,
    }
}

async fn finish_raw_login(server_url: &str, login: &RawLogin) -> (StatusCode, Value) {
    finish_raw_login_with_message(
        server_url,
        login.state_id,
        login.finalization.as_deref().unwrap(),
    )
    .await
}

async fn finish_raw_login_with_message(
    server_url: &str,
    state_id: Uuid,
    finalization: &str,
) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{server_url}/v1/auth/login/finish"))
        .json(&serde_json::json!({
            "state_id": state_id,
            "message": finalization
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.json().await.unwrap();
    (status, body)
}

fn response_fields(value: &Value) -> Vec<&str> {
    let mut fields: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    fields
}

async fn request_status(
    app: &Router,
    method: Method,
    uri: String,
    token: Option<&str>,
    body: Option<Value>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(uri);
    if builder
        .uri_ref()
        .is_some_and(|uri| uri.path() == "/v1/auth/register/request")
    {
        builder = builder.header("idempotency-key", Uuid::now_v7().to_string());
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let body = body
        .map(|value| Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap_or_else(Body::empty);
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

async fn stored_key_bundle(pool: &PgPool, user_id: Uuid, tenant_id: Uuid) -> AccountKeyBundleDto {
    let user = query::<Postgres>(
        "SELECT generation, wrapper_revision,
                wrapped_mk_by_password AS wrapped_master_key_by_password,
                wrapped_mk_by_recovery AS wrapped_master_key_by_recovery,
                account_root_public, wrapped_account_root_private
         FROM user_key_generations
         WHERE user_id = $1 AND status = 'active'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let tenant = query::<Postgres>(
        "SELECT generation, signed_manifest, wrapped_tenant_root_dek
         FROM tenant_key_generations
         WHERE tenant_id = $1 AND status = 'active'",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    AccountKeyBundleDto {
        suite_id: 2,
        generation: u64::try_from(user.try_get::<i64, _>("generation").unwrap()).unwrap(),
        tenant_generation: u64::try_from(tenant.try_get::<i64, _>("generation").unwrap()).unwrap(),
        wrapper_revision: u64::try_from(user.try_get::<i64, _>("wrapper_revision").unwrap())
            .unwrap(),
        wrapped_master_key_by_password: STANDARD.encode(
            user.try_get::<Vec<u8>, _>("wrapped_master_key_by_password")
                .unwrap(),
        ),
        wrapped_master_key_by_recovery: STANDARD.encode(
            user.try_get::<Vec<u8>, _>("wrapped_master_key_by_recovery")
                .unwrap(),
        ),
        account_root_public: STANDARD
            .encode(user.try_get::<Vec<u8>, _>("account_root_public").unwrap()),
        wrapped_account_root_private: STANDARD.encode(
            user.try_get::<Vec<u8>, _>("wrapped_account_root_private")
                .unwrap(),
        ),
        wrapped_tenant_root_dek: STANDARD.encode(
            tenant
                .try_get::<Vec<u8>, _>("wrapped_tenant_root_dek")
                .unwrap(),
        ),
        tenant_key_manifest: STANDARD
            .encode(tenant.try_get::<Vec<u8>, _>("signed_manifest").unwrap()),
    }
}
