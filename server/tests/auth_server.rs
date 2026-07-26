use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::IntoResponse,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use opaque_ke::{ClientLogin, ClientRegistration, CredentialResponse};
use rand::rngs::OsRng;
use serde_json::Value;
use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::{PgPool, Postgres};
use taskveil_crypto::{opaque_login_parameters, TaskveilCipherSuite, CRYPTO_SUITE_ID};
use taskveil_server::{
    auth,
    auth_protection::AuthProtection,
    billing::{BillingEnvironment, BillingService},
    build_router, db, AppState,
};
use taskveil_sync::account::{
    unwrap_login_key_bundle, AccountClient, AccountClientError, AccountKeyBundleDto,
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
    });
    TestApp {
        app,
        pool,
        application_pool,
        _postgres: postgres,
    }
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
    let registered = client
        .register(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("first device"),
            &[0x51; 32],
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
    assert!(client
        .register(
            "account-v2@example.com",
            "correct horse battery staple",
            Some("duplicate device"),
            &[0x52; 32],
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
    raw_sql("DROP TRIGGER opaque_registration_state_capacity_claim ON opaque_registration_states")
        .execute(&test.application_pool)
        .await
        .expect_err("runtime role must not tamper with capacity triggers");

    let legacy_state_id = Uuid::now_v7();
    query::<Postgres>(
        "INSERT INTO opaque_registration_states
            (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
             opaque_suite_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, 'legacy@example.com', 'legacy deployment', 2,
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
    query::<Postgres>("DELETE FROM opaque_registration_states WHERE id = $1")
        .bind(legacy_state_id)
        .execute(&test.application_pool)
        .await
        .expect("old code delete must release its database-owned lease");
    assert_eq!(global_opaque_capacity(&test.pool).await, 0);
    assert_eq!(opaque_capacity_lease_count(&test.pool).await, 0);
    query::<Postgres>("UPDATE opaque_registration_states SET identifier_key = $2 WHERE id = $1")
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

    let identifier_limited = auth::register_start(
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
            "INSERT INTO opaque_registration_states
                (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
                 opaque_suite_id, expires_at, identifier_key)
             VALUES ($1, $2, $3, $4, $5, $6, 'keyed cap', 2,
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
        "INSERT INTO opaque_registration_states
            (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
             opaque_suite_id, expires_at, identifier_key)
         VALUES ($1, $2, $3, $4, $5, 'keyed-cap-rejected@example.com', 'keyed cap', 2,
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
    query::<Postgres>("DELETE FROM opaque_registration_states WHERE device_name = 'keyed cap'")
        .execute(&test.application_pool)
        .await
        .unwrap();
    assert_opaque_capacity_matches_states(&test.pool).await;

    raw_sql(
        "CREATE FUNCTION reject_opaque_registration_state() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             RAISE EXCEPTION 'injected insert failure';
         END;
         $$;
         CREATE TRIGGER reject_opaque_registration_state
         BEFORE INSERT ON opaque_registration_states
         FOR EACH ROW EXECUTE FUNCTION reject_opaque_registration_state();",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    let failed_insert = auth::register_start(
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
        "DROP TRIGGER reject_opaque_registration_state ON opaque_registration_states;
         DROP FUNCTION reject_opaque_registration_state();",
    )
    .execute(&test.pool)
    .await
    .unwrap();

    raw_sql(
        "INSERT INTO opaque_registration_states
            (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
             opaque_suite_id, expires_at)
         SELECT
             lpad(to_hex(sequence), 32, '0')::uuid,
             '00000000-0000-0000-0000-000000000001'::uuid,
             '00000000-0000-0000-0000-000000000002'::uuid,
             '00000000-0000-0000-0000-000000000003'::uuid,
             decode(repeat('94', 32), 'hex'),
             'concurrency-fill-' || sequence || '@example.com',
             'legacy concurrency fill',
             2,
             now() + interval '10 minutes'
         FROM generate_series(1, 4095) AS sequence",
    )
    .execute(&test.pool)
    .await
    .unwrap();
    assert_opaque_capacity_matches_states(&test.pool).await;
    let first = auth::register_start(
        &test.application_pool,
        registration_start_request("concurrent-a@example.com"),
        &[0x63; 32],
    );
    let second = auth::register_start(
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
            "INSERT INTO opaque_registration_states
                (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
                 opaque_suite_id, expires_at, identifier_key)
             VALUES ($1, $2, $3, $4, $5, $6, 'cleanup test', 2,
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
            "INSERT INTO opaque_registration_states
                (id, user_id, tenant_id, device_id, device_challenge, email, device_name,
                 opaque_suite_id, expires_at, identifier_key)
             VALUES ($1, $2, $3, $4, $5, $6, 'underflow test', 2,
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
    let state_count: i64 =
        query::<Postgres>("SELECT count(*) AS count FROM opaque_registration_states")
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
        ClientRegistration::<TaskveilCipherSuite>::start(&mut OsRng, b"test password").unwrap();
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
    client
        .register(
            "privacy@example.com",
            "correct horse battery staple",
            Some("registration device"),
            &[0x61; 32],
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
