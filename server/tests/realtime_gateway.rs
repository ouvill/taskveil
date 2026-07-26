use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx_core::{query::query, raw_sql::raw_sql};
use sqlx_postgres::PgPool;
use taskveil_server::{
    auth_protection::AuthProtection,
    billing::{BillingEnvironment, BillingService},
    build_router, build_router_with_realtime, db,
    realtime::{RealtimeGateway, RealtimeSettings, RealtimeTicketResponse},
    AppState,
};
use taskveil_sync::{
    protocol::{
        PushOp, PushRequest, PushResponse, PushStatus, SyncCollection, SyncRecordState,
        SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION_HEADER,
    },
    Hlc,
};
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    application_pool: PgPool,
    admin_pool: PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    token: String,
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
            "CREATE ROLE taskveil_realtime_test LOGIN PASSWORD 'taskveil-realtime-test'
             NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS",
        )
        .execute(&admin_pool)
        .await
        .unwrap();
        raw_sql("GRANT taskveil_app TO taskveil_realtime_test")
            .execute(&admin_pool)
            .await
            .unwrap();

        let user_id = Uuid::now_v7();
        let tenant_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        let token = "realtime-gateway-test-token".to_owned();
        query(
            "INSERT INTO users (id, email, opaque_suite_id, opaque_record, account_root_public)
             VALUES ($1, $2, 2, $3, '\\x00'::bytea)",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .bind(vec![1_u8])
        .execute(&admin_pool)
        .await
        .unwrap();
        query("INSERT INTO billing_customers (user_id) VALUES ($1)")
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        query(
            "INSERT INTO billing_subscriptions
                (user_id, provider, environment, provider_subscription_id,
                 store_product_identifier, provider_product_id, status, gives_access,
                 current_period_ends_at, access_expires_at, will_renew,
                 provider_observed_at, last_seen_at)
             VALUES ($1, 'revenuecat', 'sandbox', $2,
                     'com.taskveil.app.pro.monthly', 'test-product', 'active', TRUE,
                     now() + interval '1 day', now() + interval '1 day', TRUE, now(), now())",
        )
        .bind(user_id)
        .bind(format!("test-{user_id}"))
        .execute(&admin_pool)
        .await
        .unwrap();
        query(
            "INSERT INTO account_entitlements
                (user_id, environment, lookup_key, status, gives_access,
                 source_subscription_id, store_product_identifier, expires_at,
                 will_renew, provider_observed_at)
             SELECT $1, 'sandbox', 'pro', 'active', TRUE, id,
                    store_product_identifier, access_expires_at, TRUE, now()
             FROM billing_subscriptions WHERE user_id = $1 AND environment = 'sandbox'",
        )
        .bind(user_id)
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
            "INSERT INTO devices
               (id, user_id, device_name, certificate, certified_at)
               VALUES ($1, $2, 'test', '\\x00'::bytea, now())",
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
             VALUES ($1, $2, $3, 'taskveil-native', $4)",
        )
        .bind(family_id)
        .bind(user_id)
        .bind(device_id)
        .bind(Utc::now() + Duration::days(1))
        .execute(&admin_pool)
        .await
        .unwrap();
        query(
            "INSERT INTO access_tokens (id, family_id, token_hash, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(family_id)
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .bind(Utc::now() + Duration::days(1))
        .execute(&admin_pool)
        .await
        .unwrap();
        query(
            "INSERT INTO tenant_device_continuity
                 (tenant_id, device_id, continuity_seq, continuity_generation, required_generation, initialized)
             VALUES ($1, $2, 0, 0, 0, true)",
        )
        .bind(tenant_id)
        .bind(device_id)
        .execute(&admin_pool)
        .await
        .unwrap();

        let application_url = format!(
            "postgres://taskveil_realtime_test:taskveil-realtime-test@{host}:{port}/postgres"
        );
        let application_pool = db::connect_application(&application_url).await.unwrap();
        Self {
            application_pool,
            admin_pool,
            tenant_id,
            device_id,
            token,
            _postgres: postgres,
        }
    }

    fn disabled_router(&self) -> Router {
        build_router(AppState {
            pool: self.application_pool.clone(),
            billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: taskveil_server::resync_token::ResyncTokenKeyring::for_tests(),
            auth_protection: AuthProtection::new([0xA7; 32]),
            trust_source_ip_header: false,
        })
    }

    fn enabled_router(&self) -> Router {
        build_router_with_realtime(
            AppState {
                pool: self.application_pool.clone(),
                billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
                auth_issuer: "http://localhost".to_string(),
                resync_tokens: taskveil_server::resync_token::ResyncTokenKeyring::for_tests(),
                auth_protection: AuthProtection::new([0xA7; 32]),
                trust_source_ip_header: false,
            },
            RealtimeGateway::from_settings(settings()).unwrap(),
        )
    }

    fn request(&self, path: &str, body: Body) -> Request<Body> {
        Request::post(path)
            .header("Authorization", format!("Bearer {}", self.token))
            .header(SYNC_PROTOCOL_VERSION_HEADER, SYNC_PROTOCOL_VERSION)
            .body(body)
            .unwrap()
    }
}

#[tokio::test]
async fn ticket_inherits_sync_auth_and_disabled_mode_returns_503() {
    let fixture = Fixture::setup().await;
    let path = format!("/v2/tenants/{}/realtime/ticket", fixture.tenant_id);

    let unauthorized = fixture
        .disabled_router()
        .oneshot(Request::post(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let disabled = fixture
        .disabled_router()
        .oneshot(fixture.request(&path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::SERVICE_UNAVAILABLE);

    let enabled = fixture
        .enabled_router()
        .oneshot(fixture.request(&path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = to_bytes(enabled.into_body(), 4096).await.unwrap();
    let ticket: RealtimeTicketResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(ticket.websocket_url, "wss://realtime.example/v1/connect");
    let segments: Vec<_> = ticket.ticket.split('.').collect();
    assert_eq!(segments.len(), 2);
    assert_eq!(URL_SAFE_NO_PAD.decode(segments[1]).unwrap().len(), 32);
    let payload = String::from_utf8(URL_SAFE_NO_PAD.decode(segments[0]).unwrap()).unwrap();
    let fields: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let expected_payload = format!(
        "{{\"kid\":\"ticket-current\",\"aud\":\"taskveil-realtime\",\"channel\":\"{}\",\"device\":\"{}\",\"iat\":{},\"exp\":{}}}",
        fields["channel"].as_str().unwrap(),
        fields["device"].as_str().unwrap(),
        fields["iat"].as_i64().unwrap(),
        fields["exp"].as_i64().unwrap()
    );
    assert_eq!(payload, expected_payload);
    assert_eq!(fields["channel"].as_str().unwrap().len(), 43);
    assert_eq!(fields["device"].as_str().unwrap().len(), 43);
    assert_eq!(
        fields["exp"].as_i64().unwrap() - fields["iat"].as_i64().unwrap(),
        300
    );
    assert!(!ticket.websocket_url.contains(&ticket.ticket));
    assert!((ticket.expires_at - Utc::now()).num_seconds() <= 300);
    assert!((ticket.expires_at - Utc::now()).num_seconds() >= 298);

    query("UPDATE devices SET revoked_at = now() WHERE id = $1")
        .bind(fixture.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
    let revoked = fixture
        .enabled_router()
        .oneshot(fixture.request(&path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepted_push_survives_provider_failure_and_no_op_keeps_wire_shape() {
    let fixture = Fixture::setup().await;
    let path = format!("/v2/tenants/{}/push", fixture.tenant_id);
    let now = Utc::now().timestamp_millis();
    let revision = Hlc {
        wall_ms: now,
        counter: 0,
        device_id: fixture.device_id.to_string(),
    }
    .encode()
    .unwrap();
    let request = PushRequest {
        ops: vec![PushOp {
            op_id: Uuid::now_v7(),
            record_id: Uuid::now_v7(),
            collection: SyncCollection::Tasks,
            base_revision_hlc: None,
            revision_hlc: revision.clone(),
            state: SyncRecordState::Live {
                mutation_hlc: revision,
                blob: STANDARD.encode(structural_envelope()),
            },
        }],
    };

    let accepted = fixture
        .enabled_router()
        .oneshot(
            Request::post(&path)
                .header("Authorization", format!("Bearer {}", fixture.token))
                .header(SYNC_PROTOCOL_VERSION_HEADER, SYNC_PROTOCOL_VERSION)
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let accepted_status = accepted.status();
    let body = to_bytes(accepted.into_body(), 16 * 1024).await.unwrap();
    assert_eq!(
        accepted_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let response: PushResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(response.results[0].status, PushStatus::Accepted);

    query(
        "UPDATE tenant_device_continuity SET continuity_seq = 1
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.device_id)
    .execute(&fixture.admin_pool)
    .await
    .unwrap();

    let no_op = fixture
        .enabled_router()
        .oneshot(
            Request::post(&path)
                .header("Authorization", format!("Bearer {}", fixture.token))
                .header(SYNC_PROTOCOL_VERSION_HEADER, SYNC_PROTOCOL_VERSION)
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_op.status(), StatusCode::OK);
    let body = to_bytes(no_op.into_body(), 16 * 1024).await.unwrap();
    let response: PushResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(response.results[0].status, PushStatus::NoOp);
}

fn structural_envelope() -> Vec<u8> {
    let mut envelope = Vec::new();
    envelope.extend_from_slice(b"TDE5");
    envelope.extend_from_slice(&2_u16.to_be_bytes());
    envelope.extend_from_slice(&1_u64.to_be_bytes());
    envelope.extend_from_slice(&[0_u8; 24]);
    envelope.extend_from_slice(&[0_u8; 16]);
    envelope
}

fn settings() -> RealtimeSettings {
    RealtimeSettings {
        websocket_url: "wss://realtime.example/v1/connect".to_owned(),
        publish_url: "https://provider-does-not-exist.invalid/v1/publish".to_owned(),
        channel_key: URL_SAFE_NO_PAD.encode([1_u8; 32]),
        ticket_key_current_id: "ticket-current".to_owned(),
        ticket_key_current: URL_SAFE_NO_PAD.encode([2_u8; 32]),
        ticket_key_previous_id: "ticket-previous".to_owned(),
        ticket_key_previous: URL_SAFE_NO_PAD.encode([3_u8; 32]),
        publish_key_current_id: "publish-current".to_owned(),
        publish_key_current: URL_SAFE_NO_PAD.encode([4_u8; 32]),
        publish_key_previous_id: "publish-previous".to_owned(),
        publish_key_previous: URL_SAFE_NO_PAD.encode([5_u8; 32]),
    }
}
