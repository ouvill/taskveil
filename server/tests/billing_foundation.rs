use std::sync::{Arc, Mutex};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::PgPool;
use taskveil_protocol::sync::{SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION_HEADER};
use taskveil_server::{
    auth_protection::AuthProtection,
    billing::{
        BillingEnvironment, BillingProvider, BillingService, ProviderError, ProviderFuture,
        ProviderSnapshot, ProviderSubscriptionSnapshot, SubscriptionStatus, MONTHLY_PRODUCT_ID,
    },
    build_router, db, AppState,
};
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION_MATRIX_ROUTE_COUNT: usize = 15;

#[derive(Clone)]
struct FakeProvider {
    snapshot: Arc<Mutex<Result<ProviderSnapshot, ProviderError>>>,
}

impl FakeProvider {
    fn new(snapshot: ProviderSnapshot) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(Ok(snapshot))),
        }
    }

    fn set(&self, snapshot: ProviderSnapshot) {
        *self.snapshot.lock().unwrap() = Ok(snapshot);
    }

    fn fail(&self) {
        *self.snapshot.lock().unwrap() = Err(ProviderError::Unavailable);
    }
}

impl BillingProvider for FakeProvider {
    fn snapshot<'a>(
        &'a self,
        _app_user_id: Uuid,
        _environment: BillingEnvironment,
    ) -> ProviderFuture<'a> {
        let value = self.snapshot.lock().unwrap().clone();
        Box::pin(async move { value })
    }
}

struct Fixture {
    app: Router,
    admin_pool: PgPool,
    user_id: Uuid,
    tenant_id: Uuid,
    device_id: Uuid,
    family_id: Uuid,
    provider_app_user_id: Uuid,
    token: String,
    provider: FakeProvider,
    _postgres: ContainerAsync<postgres::Postgres>,
}

impl Fixture {
    async fn setup() -> Self {
        let postgres = postgres::Postgres::default().start().await.unwrap();
        let host = postgres.get_host().await.unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let admin_pool = db::connect(&database_url).await.unwrap();
        db::run_migrations(&admin_pool).await.unwrap();
        raw_sql(
            "CREATE ROLE taskveil_billing_test LOGIN PASSWORD 'taskveil-billing-test'
             NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS",
        )
        .execute(&admin_pool)
        .await
        .unwrap();
        raw_sql("GRANT taskveil_app TO taskveil_billing_test")
            .execute(&admin_pool)
            .await
            .unwrap();

        let user_id = Uuid::now_v7();
        let tenant_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        let provider_app_user_id = Uuid::now_v7();
        let token = "billing-foundation-test-token".to_string();
        query(
            "INSERT INTO users
                (id, email, canonical_email, opaque_credential_id,
                 opaque_suite_id, opaque_record, account_root_public)
             VALUES ($1, $2, $2, $1, 2, $3, '\\x00'::bytea)",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .bind(vec![1_u8])
        .execute(&admin_pool)
        .await
        .unwrap();
        query("INSERT INTO billing_customers (user_id, provider_app_user_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(provider_app_user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        query("INSERT INTO tenants (id, kind, owner_user_id) VALUES ($1, 'personal', $2)")
            .bind(tenant_id)
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(tenant_id)
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        query("INSERT INTO tenant_seq (tenant_id, last_seq) VALUES ($1, 0)")
            .bind(tenant_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        query(
            "INSERT INTO tenant_key_generations
                (tenant_id, generation, suite_id, status, minimum_write_generation,
                 signed_manifest, wrapped_tenant_root_dek)
             VALUES ($1, 1, 2, 'active', 1, $2, $3)",
        )
        .bind(tenant_id)
        .bind(vec![0_u8; 124])
        .bind(vec![1_u8; 48])
        .execute(&admin_pool)
        .await
        .unwrap();
        query(
            "INSERT INTO devices (id, user_id, device_name, certificate, certified_at)
             VALUES ($1, $2, 'billing-test', '\\x00'::bytea, now())",
        )
        .bind(device_id)
        .bind(user_id)
        .execute(&admin_pool)
        .await
        .unwrap();
        let family_id = Uuid::now_v7();
        query(
            "INSERT INTO session_families
                (id, user_id, device_id, client_id, absolute_expires_at)
             VALUES ($1, $2, $3, 'taskveil-native', now() + interval '1 day')",
        )
        .bind(family_id)
        .bind(user_id)
        .bind(device_id)
        .execute(&admin_pool)
        .await
        .unwrap();
        query(
            "INSERT INTO access_tokens (id, family_id, token_hash, expires_at)
             VALUES ($1, $2, $3, now() + interval '1 day')",
        )
        .bind(Uuid::now_v7())
        .bind(family_id)
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .execute(&admin_pool)
        .await
        .unwrap();

        let application_url = format!(
            "postgres://taskveil_billing_test:taskveil-billing-test@{host}:{port}/postgres"
        );
        let application_pool = db::connect_application(&application_url).await.unwrap();
        let provider = FakeProvider::new(snapshot(SubscriptionStatus::Expired, false));
        let billing =
            BillingService::for_tests(BillingEnvironment::Sandbox, Arc::new(provider.clone()));
        let app = build_router(AppState {
            pool: application_pool,
            billing,
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: taskveil_server::resync_token::ResyncTokenKeyring::for_tests(),
            auth_protection: AuthProtection::new([0xA7; 32]),
            trust_source_ip_header: false,
            email_verification:
                taskveil_server::email_verification::EmailVerificationService::for_tests(),
        });
        Self {
            app,
            admin_pool,
            user_id,
            tenant_id,
            device_id,
            family_id,
            provider_app_user_id,
            token,
            provider,
            _postgres: postgres,
        }
    }

    async fn request(&self, method: Method, path: &str, body: Body) -> (StatusCode, Value) {
        self.request_with_policy_headers(
            method,
            path,
            body,
            Some(&self.token),
            Some(SYNC_PROTOCOL_VERSION),
        )
        .await
    }

    async fn request_with_policy_headers(
        &self,
        method: Method,
        path: &str,
        body: Body,
        bearer_token: Option<&str>,
        protocol_version: Option<u16>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", "application/json");
        if let Some(token) = bearer_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(version) = protocol_version {
            request = request.header(SYNC_PROTOCOL_VERSION_HEADER, version.to_string());
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    async fn refresh(&self) -> (StatusCode, Value) {
        self.request(
            Method::POST,
            &format!("/v2/tenants/{}/billing/refresh", self.tenant_id),
            Body::from("{}"),
        )
        .await
    }
}

fn snapshot(status: SubscriptionStatus, gives_access: bool) -> ProviderSnapshot {
    let observed_at = Utc::now();
    ProviderSnapshot {
        refresh_token: None,
        observed_at,
        subscriptions: vec![ProviderSubscriptionSnapshot {
            provider_subscription_id: "sub-test-pro".into(),
            provider_product_id: "prod-test-monthly".into(),
            store_transaction_identifier: Some("tx-current".into()),
            store_original_transaction_identifier: None,
            store_product_identifier: MONTHLY_PRODUCT_ID.into(),
            status,
            gives_access,
            current_period_ends_at: Some(observed_at + Duration::days(14)),
            access_expires_at: Some(observed_at + Duration::days(14)),
            will_renew: Some(gives_access),
            revocation_reason: (status == SubscriptionStatus::Revoked).then(|| "refund".into()),
        }],
    }
}

#[tokio::test]
async fn refresh_converges_states_and_sync_uses_request_time_402() {
    let fixture = Fixture::setup().await;
    let (status, free) = fixture
        .request(
            Method::GET,
            &format!("/v2/tenants/{}/billing", fixture.tenant_id),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(free["entitlement"]["status"], "free");

    for (provider_status, expected, allowed) in [
        (SubscriptionStatus::Trial, "trial", true),
        (SubscriptionStatus::Active, "active", true),
        (SubscriptionStatus::Grace, "grace", true),
        (SubscriptionStatus::Expired, "expired", false),
        (SubscriptionStatus::Revoked, "revoked", false),
    ] {
        fixture.provider.set(snapshot(provider_status, allowed));
        let (status, body) = fixture.refresh().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entitlement"]["status"], expected);
        assert_eq!(body["entitlement"]["sync_allowed"], allowed);

        let (sync_status, sync_body) = fixture
            .request(
                Method::GET,
                &format!("/v2/tenants/{}/preflight?since=0", fixture.tenant_id),
                Body::empty(),
            )
            .await;
        if allowed {
            assert_eq!(sync_status, StatusCode::OK);
        } else {
            assert_eq!(sync_status, StatusCode::PAYMENT_REQUIRED);
            assert_eq!(sync_body, json!({"error": "entitlement_required"}));
        }
    }
}

#[tokio::test]
async fn negative_authorization_matrix_covers_every_sync_and_realtime_route() {
    let fixture = Fixture::setup().await;
    let tenant = fixture.tenant_id;
    let device = Uuid::now_v7();
    let proof = Uuid::now_v7();
    let cases = vec![
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/preflight?since=0"),
            json!(null),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/push"),
            json!({"ops": []}),
        ),
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/pull?since=0"),
            json!(null),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/resync/start"),
            json!(null),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/resync/base"),
            json!({"page_token": "opaque", "limit": 100}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/resync/base/complete"),
            json!({"completion_token": "opaque"}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/continuity/ack"),
            json!({
                "proof": {
                    "proof_id": proof,
                    "tenant_id": tenant,
                    "device_id": device,
                    "high_water": 0,
                    "generation": 1
                }
            }),
        ),
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/key-rotation"),
            json!(null),
        ),
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/key-rotation/bundle"),
            json!(null),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/key-rotation/prepare"),
            json!({
                "suite_id": 2,
                "generation": 2,
                "signed_manifest": "AA==",
                "wrapped_tenant_root_dek": "AA=="
            }),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/key-rotation/activate"),
            json!({"generation": 2, "signed_manifest": "AA=="}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/key-rotation/ack"),
            json!({"generation": 1}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/key-rotation/retire"),
            json!({"generation": 1}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/devices/{device}/key-expiry"),
            json!({"expires_at": null}),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/realtime/ticket"),
            json!(null),
        ),
    ];
    assert_eq!(cases.len(), AUTHORIZATION_MATRIX_ROUTE_COUNT);

    let stale_protocol = SYNC_PROTOCOL_VERSION.checked_add(1).unwrap();
    assert_route_matrix(
        &fixture,
        &cases,
        Some("not-a-session"),
        Some(stale_protocol),
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "session",
    )
    .await;
    assert_policy_precedes_request_validation(&fixture).await;

    query("UPDATE access_tokens SET revoked_at = now() WHERE family_id = $1")
        .bind(fixture.family_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
    assert_route_matrix(
        &fixture,
        &cases,
        Some(&fixture.token),
        Some(stale_protocol),
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "revoked session",
    )
    .await;
    query("UPDATE access_tokens SET revoked_at = NULL WHERE family_id = $1")
        .bind(fixture.family_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();

    query("UPDATE devices SET revoked_at = now() WHERE id = $1")
        .bind(fixture.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
    assert_route_matrix(
        &fixture,
        &cases,
        Some(&fixture.token),
        Some(stale_protocol),
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "device",
    )
    .await;
    query("UPDATE devices SET revoked_at = NULL WHERE id = $1")
        .bind(fixture.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();

    query("DELETE FROM tenant_members WHERE tenant_id = $1 AND user_id = $2")
        .bind(fixture.tenant_id)
        .bind(fixture.user_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
    assert_route_matrix(
        &fixture,
        &cases,
        Some(&fixture.token),
        Some(stale_protocol),
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "membership",
    )
    .await;
    query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(fixture.tenant_id)
        .bind(fixture.user_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();

    assert_route_matrix(
        &fixture,
        &cases,
        Some(&fixture.token),
        Some(stale_protocol),
        StatusCode::PAYMENT_REQUIRED,
        "entitlement_required",
        "entitlement",
    )
    .await;

    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    assert_eq!(fixture.refresh().await.0, StatusCode::OK);
    assert_route_matrix(
        &fixture,
        &cases,
        Some(&fixture.token),
        Some(stale_protocol),
        StatusCode::CONFLICT,
        "sync protocol upgrade required",
        "protocol",
    )
    .await;
}

async fn assert_policy_precedes_request_validation(fixture: &Fixture) {
    let tenant = fixture.tenant_id;
    let malformed_cases = [
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/preflight?since=not-an-integer"),
            Body::empty(),
        ),
        (
            Method::GET,
            format!("/v2/tenants/{tenant}/pull?since=not-an-integer"),
            Body::empty(),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/resync/base"),
            Body::from("{not-json"),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/resync/base/complete"),
            Body::from("{not-json"),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/push"),
            Body::from("{not-json"),
        ),
        (
            Method::POST,
            format!("/v2/tenants/{tenant}/devices/not-a-uuid/key-expiry"),
            Body::from("{not-json"),
        ),
    ];

    for (method, path, body) in malformed_cases {
        let (status, response) = fixture
            .request_with_policy_headers(
                method,
                &path,
                body,
                Some("not-a-session"),
                Some(SYNC_PROTOCOL_VERSION.checked_add(1).unwrap()),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "validation order: {path}");
        assert_eq!(
            response,
            json!({"error": "unauthorized"}),
            "validation order: {path}"
        );
    }
}

async fn assert_route_matrix(
    fixture: &Fixture,
    cases: &[(Method, String, Value)],
    bearer_token: Option<&str>,
    protocol_version: Option<u16>,
    expected_status: StatusCode,
    expected_error: &str,
    gate: &str,
) {
    for (method, path, body) in cases {
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(serde_json::to_vec(body).unwrap())
        };
        let (status, response) = fixture
            .request_with_policy_headers(method.clone(), path, body, bearer_token, protocol_version)
            .await;
        assert_eq!(status, expected_status, "{gate}: {path}");
        assert_eq!(response, json!({"error": expected_error}), "{gate}: {path}");
    }
}

#[tokio::test]
async fn provider_failure_is_503_and_does_not_replace_the_last_aggregate() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    assert_eq!(fixture.refresh().await.0, StatusCode::OK);
    fixture.provider.fail();
    assert_eq!(fixture.refresh().await.0, StatusCode::SERVICE_UNAVAILABLE);
    let (_, body) = fixture
        .request(
            Method::GET,
            &format!("/v2/tenants/{}/billing", fixture.tenant_id),
            Body::empty(),
        )
        .await;
    assert_eq!(body["entitlement"]["status"], "active");
}

#[tokio::test]
async fn superseded_active_snapshot_cannot_restore_access_after_revocation() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Revoked, false));
    assert_eq!(fixture.refresh().await.0, StatusCode::OK);

    let mut stale = snapshot(SubscriptionStatus::Active, true);
    stale.refresh_token = Some(Uuid::now_v7());
    let error = taskveil_server::billing::apply_snapshot(
        &fixture.admin_pool,
        fixture.user_id,
        fixture.provider_app_user_id,
        BillingEnvironment::Sandbox,
        stale,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        axum::response::IntoResponse::into_response(error).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (_, body) = fixture
        .request(
            Method::GET,
            &format!("/v2/tenants/{}/billing", fixture.tenant_id),
            Body::empty(),
        )
        .await;
    assert_eq!(body["entitlement"]["status"], "revoked");
    assert_eq!(body["entitlement"]["sync_allowed"], false);
}

#[tokio::test]
async fn webhook_rejects_untrusted_delivery_before_persisting_an_event() {
    let fixture = Fixture::setup().await;
    let valid = json!({
        "event": {
            "id": "evt-untrusted",
            "type": "INITIAL_PURCHASE",
            "app_id": "test-sandbox-app",
            "environment": "SANDBOX",
            "app_user_id": fixture.provider_app_user_id,
            "aliases": []
        }
    });
    let now = Utc::now().timestamp();
    let cases = [
        (
            "wrong-authorization",
            valid.clone(),
            now,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "test-sandbox-authorization",
            valid.clone(),
            now - 301,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "test-sandbox-authorization",
            {
                let mut value = valid.clone();
                value["event"]["app_id"] = json!("other-app");
                value
            },
            now,
            StatusCode::FORBIDDEN,
        ),
        (
            "test-sandbox-authorization",
            {
                let mut value = valid;
                value["event"]["environment"] = json!("PRODUCTION");
                value
            },
            now,
            StatusCode::FORBIDDEN,
        ),
        (
            "test-sandbox-authorization",
            json!({
                "event": {
                    "id": "evt-missing-type",
                    "app_id": "test-sandbox-app",
                    "environment": "SANDBOX",
                    "app_user_id": fixture.provider_app_user_id,
                    "aliases": []
                }
            }),
            now,
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (authorization, value, timestamp, expected) in cases {
        let body = serde_json::to_vec(&value).unwrap();
        let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &body);
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::post("/v1/billing/webhooks/revenuecat")
                    .header("Authorization", authorization)
                    .header(
                        "X-RevenueCat-Webhook-Signature",
                        format!("t={timestamp},v1={signature}"),
                    )
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    let count: i64 = query("SELECT count(*) AS count FROM billing_events")
        .fetch_one(&fixture.admin_pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn signed_webhook_is_idempotent_and_unknown_customer_is_rejected() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    let event_id = "evt-billing-1";
    let body = serde_json::to_vec(&json!({
        "event": {
            "id": event_id,
            "type": "INITIAL_PURCHASE",
            "app_id": "test-sandbox-app",
            "environment": "SANDBOX",
            "app_user_id": fixture.provider_app_user_id,
            "aliases": [],
            "store": "APP_STORE",
            "product_id": MONTHLY_PRODUCT_ID,
            "transaction_id": "tx-current",
            "original_transaction_id": "tx-original",
            "price_in_purchased_currency": 4.99,
            "currency": "USD",
            "country_code": "US"
        }
    }))
    .unwrap();
    let timestamp = Utc::now().timestamp();
    let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &body);
    for _ in 0..2 {
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::post("/v1/billing/webhooks/revenuecat")
                    .header("Authorization", "test-sandbox-authorization")
                    .header(
                        "X-RevenueCat-Webhook-Signature",
                        format!("t={timestamp},v1={signature}"),
                    )
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let count: i64 =
        query("SELECT count(*) AS count FROM billing_events WHERE provider_event_id = $1")
            .bind(event_id)
            .fetch_one(&fixture.admin_pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(count, 1);
    let original: Option<String> = query(
        "SELECT store_original_transaction_identifier
         FROM billing_subscriptions
         WHERE provider_subscription_id = 'sub-test-pro'",
    )
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("store_original_transaction_identifier")
    .unwrap();
    assert_eq!(original.as_deref(), Some("tx-original"));

    let unknown = body.to_vec();
    let mut value: Value = serde_json::from_slice(&unknown).unwrap();
    value["event"]["id"] = json!("evt-unknown");
    value["event"]["app_user_id"] = json!(Uuid::now_v7());
    let unknown = serde_json::to_vec(&value).unwrap();
    let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &unknown);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/v1/billing/webhooks/revenuecat")
                .header("Authorization", "test-sandbox-authorization")
                .header(
                    "X-RevenueCat-Webhook-Signature",
                    format!("t={timestamp},v1={signature}"),
                )
                .body(Body::from(unknown))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refund_is_revoked_until_a_new_subscription_is_purchased() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    let body = serde_json::to_vec(&json!({
        "event": {
            "id": "evt-refund",
            "type": "CANCELLATION",
            "app_id": "test-sandbox-app",
            "environment": "SANDBOX",
            "app_user_id": fixture.provider_app_user_id,
            "aliases": [],
            "cancel_reason": "CUSTOMER_SUPPORT",
            "product_id": MONTHLY_PRODUCT_ID,
            "transaction_id": "tx-current",
            "original_transaction_id": "tx-original"
        }
    }))
    .unwrap();
    let timestamp = Utc::now().timestamp();
    let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &body);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/v1/billing/webhooks/revenuecat")
                .header("Authorization", "test-sandbox-authorization")
                .header(
                    "X-RevenueCat-Webhook-Signature",
                    format!("t={timestamp},v1={signature}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        fixture.refresh().await.1["entitlement"]["status"],
        "revoked"
    );

    let mut repurchase = snapshot(SubscriptionStatus::Active, true);
    repurchase.subscriptions[0].store_transaction_identifier = Some("tx-renewal".into());
    fixture.provider.set(repurchase);
    let (_, body) = fixture.refresh().await;
    assert_eq!(body["entitlement"]["status"], "active");
    assert_eq!(body["entitlement"]["sync_allowed"], true);

    let delayed = serde_json::to_vec(&json!({
        "event": {
            "id": "evt-refund-delayed",
            "type": "CANCELLATION",
            "app_id": "test-sandbox-app",
            "environment": "SANDBOX",
            "app_user_id": fixture.provider_app_user_id,
            "aliases": [],
            "cancel_reason": "CUSTOMER_SUPPORT",
            "product_id": MONTHLY_PRODUCT_ID,
            "transaction_id": "tx-current",
            "original_transaction_id": "tx-original"
        }
    }))
    .unwrap();
    let timestamp = Utc::now().timestamp();
    let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &delayed);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/v1/billing/webhooks/revenuecat")
                .header("Authorization", "test-sandbox-authorization")
                .header(
                    "X-RevenueCat-Webhook-Signature",
                    format!("t={timestamp},v1={signature}"),
                )
                .body(Body::from(delayed))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (_, body) = fixture
        .request(
            Method::GET,
            &format!("/v2/tenants/{}/billing", fixture.tenant_id),
            Body::empty(),
        )
        .await;
    assert_eq!(body["entitlement"]["status"], "active");
    assert_eq!(body["entitlement"]["sync_allowed"], true);
}

#[tokio::test]
async fn transfer_without_subscriber_identity_revokes_the_source_account() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    assert_eq!(fixture.refresh().await.0, StatusCode::OK);
    fixture.provider.set(ProviderSnapshot {
        refresh_token: None,
        observed_at: Utc::now(),
        subscriptions: vec![],
    });
    let body = serde_json::to_vec(&json!({
        "event": {
            "id": "evt-transfer",
            "type": "TRANSFER",
            "app_id": "test-sandbox-app",
            "environment": "SANDBOX",
            "transferred_from": [fixture.provider_app_user_id],
            "transferred_to": [Uuid::now_v7()]
        }
    }))
    .unwrap();
    let timestamp = Utc::now().timestamp();
    let signature = webhook_signature(b"test-sandbox-hmac", timestamp, &body);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/v1/billing/webhooks/revenuecat")
                .header("Authorization", "test-sandbox-authorization")
                .header(
                    "X-RevenueCat-Webhook-Signature",
                    format!("t={timestamp},v1={signature}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (_, billing) = fixture
        .request(
            Method::GET,
            &format!("/v2/tenants/{}/billing", fixture.tenant_id),
            Body::empty(),
        )
        .await;
    assert_eq!(billing["entitlement"]["status"], "revoked");
    assert_eq!(billing["entitlement"]["sync_allowed"], false);
}

#[tokio::test]
async fn subscription_id_cannot_be_replayed_to_another_account() {
    let fixture = Fixture::setup().await;
    fixture
        .provider
        .set(snapshot(SubscriptionStatus::Active, true));
    assert_eq!(fixture.refresh().await.0, StatusCode::OK);
    let other_user = Uuid::now_v7();
    let other_customer = Uuid::now_v7();
    query(
        "INSERT INTO users
            (id, email, canonical_email, opaque_credential_id,
             opaque_suite_id, opaque_record, account_root_public)
         VALUES ($1, $2, $2, $1, 2, $3, '\\x00'::bytea)",
    )
    .bind(other_user)
    .bind(format!("{other_user}@example.test"))
    .bind(vec![1_u8])
    .execute(&fixture.admin_pool)
    .await
    .unwrap();
    query("INSERT INTO billing_customers (user_id, provider_app_user_id) VALUES ($1, $2)")
        .bind(other_user)
        .bind(other_customer)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
    let error = taskveil_server::billing::apply_snapshot(
        &fixture.admin_pool,
        other_user,
        other_customer,
        BillingEnvironment::Sandbox,
        snapshot(SubscriptionStatus::Active, true),
        None,
    )
    .await
    .unwrap_err();
    let response = axum::response::IntoResponse::into_response(error);
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

fn webhook_signature(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
