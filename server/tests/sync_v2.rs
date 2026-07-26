use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    response::IntoResponse,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::PgPool;
use taskveil_client::test_support::{
    persist_local_crypto_context, LocalCryptoIdentity, LocalMutationContext, SqliteMutationService,
    SqliteSyncStore, UpdateTaskInput,
};
use taskveil_crypto::{key_hierarchy::wrap_tenant_root_dek_with_master_key, CRYPTO_SUITE_ID};
use taskveil_server::{
    auth::AuthContext,
    auth_protection::AuthProtection,
    billing::{BillingEnvironment, BillingService},
    build_router, db,
    sync::{self, gc_tombstones},
    AppState,
};
use taskveil_storage::{
    open_encrypted, ListRepository, NewSyncOutboxEntry, SqliteListRepository,
    SqliteSyncStateRepository, SqliteTaskRepository, SyncOutboxState, SyncQuarantineEntry,
    SyncStateRepository, TaskRepository,
};
use taskveil_sync::{
    account::AccountClient,
    decrypt_plaintext, encrypt_plaintext,
    protocol::{
        KeyManifestDescriptor, PushOp, PushRequest, PushStatus, SyncCapabilities, SyncCollection,
        SyncRecordState,
    },
    run_sync_now, run_sync_now_with_key_refresh, ActiveSyncContext, Hlc, KeyManifest,
    LocalMutationSyncStore, LocalSyncKeys, LocalSyncStore, RotationStatus, SyncKeyRefresher,
    SyncPlaintext, SYNC_CURSOR_NAME,
};
use tempfile::TempDir;
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tower::ServiceExt;
use uuid::Uuid;

fn active_manifest(tenant_id: Uuid) -> Vec<u8> {
    signed_manifest(tenant_id, 1, RotationStatus::Active, 1)
}

fn test_manifest_auth_key() -> zeroize::Zeroizing<[u8; 32]> {
    taskveil_sync::derive_personal_manifest_auth_key(&[0x41; 32]).unwrap()
}

fn signed_manifest(
    tenant_id: Uuid,
    generation: u64,
    status: RotationStatus,
    minimum_write_generation: u64,
) -> Vec<u8> {
    KeyManifest::authenticate_personal(
        tenant_id,
        generation,
        status,
        minimum_write_generation,
        [0; 32],
        Vec::new(),
        &[0x41; 32],
    )
    .unwrap()
    .authenticated_bytes()
    .unwrap()
}

fn signed_manifest_after(
    tenant_id: Uuid,
    generation: u64,
    status: RotationStatus,
    minimum_write_generation: u64,
    previous_manifest: &[u8],
) -> Vec<u8> {
    let previous_hash = KeyManifest::from_authenticated_bytes(previous_manifest)
        .unwrap()
        .authenticated_hash()
        .unwrap();
    KeyManifest::authenticate_personal(
        tenant_id,
        generation,
        status,
        minimum_write_generation,
        previous_hash,
        Vec::new(),
        &[0x41; 32],
    )
    .unwrap()
    .authenticated_bytes()
    .unwrap()
}

fn test_capabilities(tenant_id: Uuid, protocol_version: u16) -> SyncCapabilities {
    SyncCapabilities {
        protocol_version,
        envelope_version: taskveil_sync::ENVELOPE_VERSION,
        gc_horizon_seq: 0,
        continuity_seq: 0,
        continuity_generation: 0,
        required_generation: 0,
        full_resync_required: false,
        suite_id: CRYPTO_SUITE_ID,
        active_key_generation: 1,
        minimum_write_generation: 1,
        migrating_key_generation: None,
        key_manifests: vec![KeyManifestDescriptor {
            suite_id: CRYPTO_SUITE_ID,
            generation: 1,
            status: RotationStatus::Active,
            minimum_write_generation: 1,
            signed_manifest: STANDARD.encode(active_manifest(tenant_id)),
            predecessor_manifests: Vec::new(),
        }],
    }
}

struct Fixture {
    app: Router,
    pool: PgPool,
    admin_pool: PgPool,
    tenant_id: Uuid,
    auth: AuthContext,
    token: String,
    resync_tokens: taskveil_server::resync_token::ResyncTokenKeyring,
    _postgres: ContainerAsync<postgres::Postgres>,
}

impl Fixture {
    async fn setup() -> Self {
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

        let user_id = Uuid::now_v7();
        let tenant_id = Uuid::now_v7();
        let device_id = Uuid::now_v7();
        let token = "protocol-v2-test-token".to_string();
        query(
            "INSERT INTO users
                (id, email, canonical_email, opaque_credential_id,
                 opaque_suite_id, opaque_record, account_root_public)
             VALUES ($1, $2, $2, $1, $3, $4, '\\x00'::bytea)",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .bind(i16::try_from(CRYPTO_SUITE_ID).unwrap())
        .bind(vec![1_u8])
        .execute(&pool)
        .await
        .unwrap();
        query("INSERT INTO billing_customers (user_id) VALUES ($1)")
            .bind(user_id)
            .execute(&pool)
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
        .execute(&pool)
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
        .execute(&pool)
        .await
        .unwrap();
        query("INSERT INTO tenants (id, kind, owner_user_id) VALUES ($1, 'personal', $2)")
            .bind(tenant_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(tenant_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        query("INSERT INTO tenant_seq (tenant_id, last_seq) VALUES ($1, 0)")
            .bind(tenant_id)
            .execute(&pool)
            .await
            .unwrap();
        query(
            "INSERT INTO tenant_key_generations (
                 tenant_id, generation, suite_id, status, minimum_write_generation,
                 signed_manifest, wrapped_tenant_root_dek
             ) VALUES ($1, 1, $2, 'active', 1, $3, $4)",
        )
        .bind(tenant_id)
        .bind(i16::try_from(CRYPTO_SUITE_ID).unwrap())
        .bind(active_manifest(tenant_id))
        .bind(vec![0x42_u8; 64])
        .execute(&pool)
        .await
        .unwrap();
        query(
            "INSERT INTO devices
               (id, user_id, device_name, certificate, certified_at)
               VALUES ($1, $2, 'test', '\\x00'::bytea, now())",
        )
        .bind(device_id)
        .bind(user_id)
        .execute(&pool)
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
        .execute(&pool)
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
        .execute(&pool)
        .await
        .unwrap();

        let application_url = format!(
            "postgres://taskveil_runtime_test:taskveil-runtime-test@{host}:{port}/postgres"
        );
        let application_pool = db::connect_application(&application_url).await.unwrap();
        let resync_tokens = taskveil_server::resync_token::ResyncTokenKeyring::for_tests();
        let app = build_router(AppState {
            pool: application_pool.clone(),
            billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: resync_tokens.clone(),
            auth_protection: AuthProtection::new([0xA7; 32]),
            trust_source_ip_header: false,
            email_verification:
                taskveil_server::email_verification::EmailVerificationService::for_tests(),
        });
        Self {
            app,
            pool: application_pool,
            admin_pool: pool,
            tenant_id,
            auth: AuthContext { user_id, device_id },
            token,
            resync_tokens,
            _postgres: postgres,
        }
    }

    async fn push(&self, op: PushOp) -> taskveil_protocol::sync::PushResult {
        self.close_continuity().await;
        sync::push(
            &self.pool,
            self.tenant_id,
            self.auth.clone(),
            PushRequest { ops: vec![op] },
        )
        .await
        .unwrap()
        .results
        .pop()
        .unwrap()
    }

    fn revision_hlc(&self, delta: i64, counter: u32) -> String {
        hlc(delta, counter, &self.auth.device_id.to_string())
    }

    async fn register_device(&self) -> (Uuid, String) {
        let device_id = Uuid::now_v7();
        let token = format!("protocol-v2-test-token-{device_id}");
        query(
            "INSERT INTO devices
               (id, user_id, device_name, certificate, certified_at)
             VALUES ($1, $2, 'test-peer', '\\x00'::bytea, now())",
        )
        .bind(device_id)
        .bind(self.auth.user_id)
        .execute(&self.admin_pool)
        .await
        .unwrap();
        let family_id = Uuid::now_v7();
        query(
            "INSERT INTO session_families
                (id, user_id, device_id, client_id, absolute_expires_at)
             VALUES ($1, $2, $3, 'taskveil-native', $4)",
        )
        .bind(family_id)
        .bind(self.auth.user_id)
        .bind(device_id)
        .bind(Utc::now() + Duration::days(1))
        .execute(&self.admin_pool)
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
        .execute(&self.admin_pool)
        .await
        .unwrap();
        (device_id, token)
    }

    async fn close_continuity(&self) {
        query(
            "UPDATE tenant_device_continuity
             SET continuity_generation = required_generation, initialized = true
             WHERE tenant_id = $1 AND device_id = $2",
        )
        .bind(self.tenant_id)
        .bind(self.auth.device_id)
        .execute(&self.admin_pool)
        .await
        .unwrap();
        let since: i64 = query(
            "SELECT coalesce((
                 SELECT continuity_seq FROM tenant_device_continuity
                 WHERE tenant_id = $1 AND device_id = $2
             ), 0) AS continuity_seq",
        )
        .bind(self.tenant_id)
        .bind(self.auth.device_id)
        .fetch_one(&self.admin_pool)
        .await
        .unwrap()
        .try_get("continuity_seq")
        .unwrap();
        let page = sync::pull(
            &self.pool,
            self.tenant_id,
            self.auth.clone(),
            since,
            Some(100),
            None,
        )
        .await
        .unwrap();
        let proof = page.closure_proof.unwrap();
        sync::ack_continuity(
            &self.pool,
            self.tenant_id,
            self.auth.clone(),
            taskveil_protocol::sync::ContinuityAckRequest { proof },
        )
        .await
        .unwrap();
    }

    async fn serve(&self) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = self.app.clone();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }
}

async fn wait_for_lock_waiters(pool: &PgPool, expected: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: i64 = query(
                "SELECT count(*) AS count
                 FROM pg_stat_activity
                 WHERE datname = current_database()
                   AND pid <> pg_backend_pid()
                   AND wait_event_type = 'Lock'",
            )
            .fetch_one(pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
            if waiting >= expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("concurrent sync request did not reach the expected row-lock wait");
}

struct TestKeyRefresher {
    calls: usize,
    keys: LocalSyncKeys,
    fail: bool,
}

impl SyncKeyRefresher for TestKeyRefresher {
    fn refresh<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<LocalSyncKeys, String>> + Send + 'a>> {
        self.calls += 1;
        let result = if self.fail {
            Err("refresh failed".to_string())
        } else {
            Ok(self.keys.clone())
        };
        Box::pin(async move { result })
    }
}

#[tokio::test]
async fn production_pull_refreshes_once_then_atomically_applies_and_quarantines() {
    const DB_KEY: [u8; 32] = [0xc3; 32];
    let fixture = Fixture::setup().await;
    let server_url = fixture.serve().await;
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("quarantine.sqlite3");
    let now = Utc::now().timestamp_millis() - 20_000;
    let good = taskveil_domain::new_list(
        "Recovered".to_string(),
        "3fffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    let missing = taskveil_domain::new_list(
        "Waiting".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        now + 1,
    )
    .unwrap();
    let corrupt = taskveil_domain::new_list(
        "Corrupt".to_string(),
        "bfffffffffffffffffffffffffffffff".to_string(),
        now + 2,
    )
    .unwrap();
    let tenant_dek = [0x31; 32];
    for (index, (list, dek)) in [
        (&good, tenant_dek),
        (&missing, tenant_dek),
        (&corrupt, tenant_dek),
    ]
    .into_iter()
    .enumerate()
    {
        let mutation = Hlc {
            wall_ms: now + 100 + index as i64,
            counter: 0,
            device_id: "remote".to_string(),
        };
        let revision = Hlc {
            wall_ms: now + 200 + index as i64,
            counter: 0,
            device_id: fixture.auth.device_id.to_string(),
        };
        let plaintext = SyncPlaintext::from_list(list, mutation.clone()).unwrap();
        let blob = if list.id == corrupt.id {
            let mut blob =
                encrypt_plaintext(&dek, fixture.tenant_id, 1, "lists", list.id, &plaintext)
                    .unwrap();
            let last = blob.len() - 1;
            blob[last] ^= 0x40;
            blob
        } else {
            encrypt_plaintext(&dek, fixture.tenant_id, 1, "lists", list.id, &plaintext).unwrap()
        };
        fixture
            .push(PushOp {
                op_id: Uuid::now_v7(),
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: revision.encode().unwrap(),
                state: SyncRecordState::Live {
                    mutation_hlc: mutation.encode().unwrap(),
                    blob: STANDARD.encode(blob),
                },
            })
            .await;
    }

    let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
    let mut key_refresher = TestKeyRefresher {
        calls: 0,
        keys: LocalSyncKeys {
            tenant_id: fixture.tenant_id,
            tenant_root_dek: Some(tenant_dek.into()),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        },
        fail: false,
    };
    let context = ActiveSyncContext {
        server_url,
        tenant_id: fixture.tenant_id,
        device_id: fixture.auth.device_id.to_string(),
        session_token: taskveil_sync::SecretString::new(fixture.token.clone()),
        manifest_auth_key: test_manifest_auth_key(),
        keys: LocalSyncKeys {
            tenant_id: fixture.tenant_id,
            tenant_root_dek: None,
            ..LocalSyncKeys::default()
        },
    };
    let mut clock = now + 1_000;
    let mut ticking_now = || {
        clock += 1;
        Ok(clock)
    };
    let summary = run_sync_now_with_key_refresh(
        context.clone(),
        &mut store,
        &mut ticking_now,
        &mut key_refresher,
    )
    .await
    .unwrap();
    assert_eq!(key_refresher.calls, 1);
    assert_eq!(summary.applied_count, 2);
    assert_eq!(summary.missing_key_quarantined_count, 0);
    assert_eq!(summary.corruption_quarantined_count, 1);
    let repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    assert_eq!(
        repository
            .get_cursor(SYNC_CURSOR_NAME)
            .unwrap()
            .unwrap()
            .seq,
        3
    );
    let quarantined = repository.list_quarantine(10).unwrap();
    assert_eq!(quarantined.len(), 1);
    assert!(quarantined
        .iter()
        .any(|row| row.record_id == corrupt.id && row.reason == "authentication_failed"));
    assert_eq!(
        SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .get(good.id)
            .unwrap()
            .name,
        "Recovered"
    );

    let all_keys = LocalSyncKeys {
        tenant_id: fixture.tenant_id,
        tenant_root_dek: Some(tenant_dek.into()),
        tenant_generation: 1,
        historical_tenant_root_deks: Vec::new(),
    };

    let current_revision: String =
        query("SELECT revision_hlc FROM sync_records WHERE tenant_id = $1 AND record_id = $2")
            .bind(fixture.tenant_id)
            .bind(corrupt.id)
            .fetch_one(&fixture.admin_pool)
            .await
            .unwrap()
            .get("revision_hlc");
    let mutation = Hlc {
        wall_ms: now + 2_000,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let revision = Hlc {
        wall_ms: now + 2_001,
        counter: 0,
        device_id: fixture.auth.device_id.to_string(),
    };
    let corrected = encrypt_plaintext(
        &tenant_dek,
        fixture.tenant_id,
        1,
        "lists",
        corrupt.id,
        &SyncPlaintext::from_list(&corrupt, mutation.clone()).unwrap(),
    )
    .unwrap();
    let replacement = fixture
        .push(PushOp {
            op_id: Uuid::now_v7(),
            record_id: corrupt.id,
            collection: SyncCollection::Lists,
            base_revision_hlc: Some(current_revision),
            revision_hlc: revision.encode().unwrap(),
            state: SyncRecordState::Live {
                mutation_hlc: mutation.encode().unwrap(),
                blob: STANDARD.encode(corrected),
            },
        })
        .await;
    assert_eq!(replacement.status, PushStatus::Accepted);
    let supersede_summary = run_sync_now(
        ActiveSyncContext {
            keys: all_keys,
            ..context.clone()
        },
        &mut store,
        &mut ticking_now,
    )
    .await
    .unwrap();
    assert_eq!(supersede_summary.resolved_quarantine_count, 1);
    assert!(
        SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .list_quarantine(10)
            .unwrap()
            .is_empty()
    );

    let failed_path = temp.path().join("refresh-failure.sqlite3");
    let mut failed_store = SqliteSyncStore::new(failed_path.clone(), DB_KEY);
    let mut failed_refresher = TestKeyRefresher {
        calls: 0,
        keys: LocalSyncKeys::default(),
        fail: true,
    };
    assert_eq!(
        run_sync_now_with_key_refresh(
            context.clone(),
            &mut failed_store,
            &mut ticking_now,
            &mut failed_refresher,
        )
        .await,
        Err("refresh failed".to_string())
    );
    assert_eq!(failed_refresher.calls, 1);
    assert!(failed_store
        .get_cursor_seq(SYNC_CURSOR_NAME)
        .unwrap()
        .is_none());
    assert!(failed_store.list_quarantine(10).unwrap().is_empty());

    let matrix_corrupt = taskveil_domain::new_list(
        "Matrix corrupt".to_string(),
        "efffffffffffffffffffffffffffffff".to_string(),
        now + 4,
    )
    .unwrap();
    let matrix_mutation = Hlc {
        wall_ms: now + 2_500,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let matrix_revision = Hlc {
        wall_ms: now + 2_501,
        counter: 0,
        device_id: fixture.auth.device_id.to_string(),
    };
    let mut matrix_corrupt_blob = encrypt_plaintext(
        &tenant_dek,
        fixture.tenant_id,
        1,
        "lists",
        matrix_corrupt.id,
        &SyncPlaintext::from_list(&matrix_corrupt, matrix_mutation.clone()).unwrap(),
    )
    .unwrap();
    let last = matrix_corrupt_blob.len() - 1;
    matrix_corrupt_blob[last] ^= 0x40;
    assert_eq!(
        fixture
            .push(PushOp {
                op_id: Uuid::now_v7(),
                record_id: matrix_corrupt.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: matrix_revision.encode().unwrap(),
                state: SyncRecordState::Live {
                    mutation_hlc: matrix_mutation.encode().unwrap(),
                    blob: STANDARD.encode(matrix_corrupt_blob),
                },
            })
            .await
            .status,
        PushStatus::Accepted
    );

    for (name, trigger) in [
        (
            "state",
            "CREATE TRIGGER fail_page_state BEFORE INSERT ON sync_record_states
             BEGIN SELECT RAISE(ABORT, 'injected state failure'); END;"
                .to_string(),
        ),
        (
            "quarantine",
            format!(
                "CREATE TRIGGER fail_page_quarantine BEFORE INSERT ON sync_quarantine
                 WHEN NEW.record_id = '{}'
                 BEGIN SELECT RAISE(ABORT, 'injected quarantine failure'); END;",
                matrix_corrupt.id
            ),
        ),
        (
            "cursor",
            "CREATE TRIGGER fail_page_cursor BEFORE INSERT ON sync_cursors
             BEGIN SELECT RAISE(ABORT, 'injected cursor failure'); END;"
                .to_string(),
        ),
    ] {
        let matrix_path = temp.path().join(format!("failure-{name}.sqlite3"));
        open_encrypted(&matrix_path, &DB_KEY)
            .unwrap()
            .execute_batch(&trigger)
            .unwrap();
        let mut matrix_store = SqliteSyncStore::new(matrix_path.clone(), DB_KEY);
        let mut matrix_refresher = TestKeyRefresher {
            calls: 0,
            keys: LocalSyncKeys {
                tenant_id: fixture.tenant_id,
                tenant_root_dek: Some(tenant_dek.into()),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
            fail: false,
        };
        query(
            "DELETE FROM continuity_closure_proofs
             WHERE tenant_id = $1 AND device_id = $2",
        )
        .bind(fixture.tenant_id)
        .bind(fixture.auth.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
        query(
            "DELETE FROM device_resync_sessions
             WHERE tenant_id = $1 AND device_id = $2",
        )
        .bind(fixture.tenant_id)
        .bind(fixture.auth.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
        query(
            "UPDATE tenant_device_continuity
             SET initialized = false, continuity_seq = 0,
                 required_generation = continuity_generation
             WHERE tenant_id = $1 AND device_id = $2",
        )
        .bind(fixture.tenant_id)
        .bind(fixture.auth.device_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();
        assert_eq!(
            run_sync_now_with_key_refresh(
                ActiveSyncContext {
                    keys: LocalSyncKeys {
                        tenant_id: fixture.tenant_id,
                        tenant_root_dek: None,
                        ..LocalSyncKeys::default()
                    },
                    ..context.clone()
                },
                &mut matrix_store,
                &mut ticking_now,
                &mut matrix_refresher,
            )
            .await,
            Err("sync failed".to_string()),
            "failure stage {name}"
        );
        assert_eq!(matrix_refresher.calls, 1, "failure stage {name}");
        let connection = open_encrypted(&matrix_path, &DB_KEY).unwrap();
        if name == "cursor" {
            let phase: String = connection
                .query_row(
                    "SELECT phase FROM sync_full_resync_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(phase, "sweep");
            assert!(matrix_store
                .get_cursor_seq(SYNC_CURSOR_NAME)
                .unwrap()
                .is_none());
            drop(connection);
            open_encrypted(&matrix_path, &DB_KEY)
                .unwrap()
                .execute_batch("DROP TRIGGER fail_page_cursor;")
                .unwrap();
            run_sync_now_with_key_refresh(
                ActiveSyncContext {
                    keys: LocalSyncKeys {
                        tenant_id: fixture.tenant_id,
                        tenant_root_dek: Some(tenant_dek.into()),
                        tenant_generation: 1,
                        historical_tenant_root_deks: Vec::new(),
                    },
                    ..context.clone()
                },
                &mut matrix_store,
                &mut ticking_now,
                &mut matrix_refresher,
            )
            .await
            .unwrap();
            let high_water: i64 = query("SELECT last_seq FROM tenant_seq WHERE tenant_id = $1")
                .bind(fixture.tenant_id)
                .fetch_one(&fixture.admin_pool)
                .await
                .unwrap()
                .try_get("last_seq")
                .unwrap();
            assert_eq!(
                matrix_store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(),
                Some(high_water)
            );
            let connection = open_encrypted(&matrix_path, &DB_KEY).unwrap();
            let generation_count: i64 = connection
                .query_row("SELECT count(*) FROM sync_full_resync_state", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(generation_count, 0);
            continue;
        }
        for table in [
            "lists",
            "sync_record_states",
            "sync_outbox",
            "sync_quarantine",
            "sync_cursors",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} rollback at failure stage {name}");
        }
        let (hlc_count, hlc_value): (i64, String) = connection
            .query_row(
                "SELECT count(*), max(value) FROM internal_metadata
                 WHERE key = 'sync_local_hlc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            hlc_count, 1,
            "pre-network device rebind remains durable at failure stage {name}"
        );
        assert_eq!(
            Hlc::decode(&hlc_value).unwrap().device_id,
            context.device_id,
            "page rollback preserves the authenticated HLC node at failure stage {name}"
        );
    }

    let unknown = taskveil_domain::new_list(
        "Future".to_string(),
        "dfffffffffffffffffffffffffffffff".to_string(),
        now + 3,
    )
    .unwrap();
    let unknown_dek = [0x34; 32];
    let mutation = Hlc {
        wall_ms: now + 3_000,
        counter: 0,
        device_id: "future".to_string(),
    };
    let revision = Hlc {
        wall_ms: now + 3_001,
        counter: 0,
        device_id: fixture.auth.device_id.to_string(),
    };
    let mut unknown_blob = encrypt_plaintext(
        &unknown_dek,
        fixture.tenant_id,
        1,
        "lists",
        unknown.id,
        &SyncPlaintext::from_list(&unknown, mutation.clone()).unwrap(),
    )
    .unwrap();
    unknown_blob[0] = taskveil_sync::ENVELOPE_VERSION + 1;
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![PushOp {
                op_id: Uuid::now_v7(),
                record_id: unknown.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: revision.encode().unwrap(),
                state: SyncRecordState::Live {
                    mutation_hlc: mutation.encode().unwrap(),
                    blob: STANDARD.encode(unknown_blob),
                },
            }],
        },
    )
    .await
    .is_err());
}

#[tokio::test]
async fn replay_reaches_missing_key_after_one_hundred_corrupt_quarantine_rows() {
    const DB_KEY: [u8; 32] = [0xc4; 32];
    let fixture = Fixture::setup().await;
    let server_url = fixture.serve().await;
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("quarantine-starvation.sqlite3");
    let now = Utc::now().timestamp_millis() - 10_000;
    let waiting = taskveil_domain::new_list(
        "Recovered after corrupt rows".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    let waiting_dek = [0x41; 32];
    let mutation = Hlc {
        wall_ms: now + 1,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let revision = Hlc {
        wall_ms: now + 2,
        counter: 0,
        device_id: "remote".to_string(),
    };
    let waiting_blob = encrypt_plaintext(
        &waiting_dek,
        fixture.tenant_id,
        1,
        "lists",
        waiting.id,
        &SyncPlaintext::from_list(&waiting, mutation.clone()).unwrap(),
    )
    .unwrap();
    let mut repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    for seq in 1..=100 {
        repository
            .put_quarantine(SyncQuarantineEntry {
                record_id: Uuid::now_v7(),
                collection: "lists".to_string(),
                seq,
                revision_hlc: format!("corrupt-{seq}"),
                state: SyncOutboxState::Live {
                    mutation_hlc: format!("corrupt-mutation-{seq}"),
                    blob: vec![taskveil_sync::ENVELOPE_VERSION, 1],
                },
                reason: "authentication_failed".to_string(),
                required_list_id: None,
                first_failed_at: now,
                last_failed_at: now,
                attempt_count: 1,
            })
            .unwrap();
    }
    repository
        .put_quarantine(SyncQuarantineEntry {
            record_id: waiting.id,
            collection: "lists".to_string(),
            seq: 101,
            revision_hlc: revision.encode().unwrap(),
            state: SyncOutboxState::Live {
                mutation_hlc: mutation.encode().unwrap(),
                blob: waiting_blob,
            },
            reason: "missing_dek".to_string(),
            required_list_id: Some(waiting.id),
            first_failed_at: now,
            last_failed_at: now,
            attempt_count: 1,
        })
        .unwrap();
    drop(repository);

    let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
    let mut refresher = TestKeyRefresher {
        calls: 0,
        keys: LocalSyncKeys {
            tenant_id: fixture.tenant_id,
            tenant_root_dek: Some(waiting_dek.into()),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        },
        fail: false,
    };
    let mut clock = now + 100;
    let mut ticking_now = || {
        clock += 1;
        Ok(clock)
    };
    let summary = run_sync_now_with_key_refresh(
        ActiveSyncContext {
            server_url,
            tenant_id: fixture.tenant_id,
            device_id: "starvation-client".to_string(),
            session_token: taskveil_sync::SecretString::new(fixture.token),
            manifest_auth_key: test_manifest_auth_key(),
            keys: LocalSyncKeys {
                tenant_id: fixture.tenant_id,
                tenant_root_dek: None,
                ..LocalSyncKeys::default()
            },
        },
        &mut store,
        &mut ticking_now,
        &mut refresher,
    )
    .await
    .unwrap();

    assert_eq!(refresher.calls, 1);
    assert_eq!(summary.resolved_quarantine_count, 1);
    let repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    let remaining = repository.list_quarantine(200).unwrap();
    assert_eq!(remaining.len(), 100);
    assert!(remaining
        .iter()
        .all(|row| { row.reason == "authentication_failed" && row.attempt_count == 1 }));
    assert_eq!(
        SqliteListRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap())
            .get(waiting.id)
            .unwrap()
            .name,
        "Recovered after corrupt rows"
    );
}

#[tokio::test]
async fn unsupported_preflight_durably_blocks_outbox_before_push() {
    const DB_KEY: [u8; 32] = [0xd4; 32];
    let tenant_id = Uuid::now_v7();
    let preflight_count = Arc::new(AtomicUsize::new(0));
    let push_count = Arc::new(AtomicUsize::new(0));
    let preflight_counter = preflight_count.clone();
    let push_counter = push_count.clone();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(move || {
                let counter = preflight_counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::Json(test_capabilities(
                        tenant_id,
                        taskveil_protocol::sync::SYNC_PROTOCOL_VERSION + 1,
                    ))
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/push",
            axum::routing::post(move || {
                let counter = push_counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::Json(taskveil_protocol::sync::PushResponse { results: vec![] })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("upgrade.sqlite3");
    let record_id = Uuid::now_v7();
    let op_id = Uuid::now_v7();
    let revision = hlc(-100, 0, "local-revision");
    let mutation = hlc(-200, 0, "local-mutation");
    let mut repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    repository
        .put_outbox_head(NewSyncOutboxEntry {
            op_id,
            record_id,
            collection: "tasks".to_string(),
            base_revision_hlc: None,
            revision_hlc: revision,
            state: SyncOutboxState::Live {
                mutation_hlc: mutation,
                blob: vec![taskveil_sync::ENVELOPE_VERSION, 1],
            },
            created_at: Utc::now().timestamp_millis(),
        })
        .unwrap();
    drop(repository);
    let context = ActiveSyncContext {
        server_url: format!("http://{address}"),
        tenant_id,
        device_id: "upgrade-client".to_string(),
        session_token: taskveil_sync::SecretString::new("token"),
        manifest_auth_key: test_manifest_auth_key(),
        keys: LocalSyncKeys::default(),
    };
    let mut store = SqliteSyncStore::new(db_path.clone(), DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());
    assert_eq!(
        run_sync_now(context.clone(), &mut store, &mut now).await,
        Err("upgrade required".to_string())
    );
    assert_eq!(preflight_count.load(Ordering::SeqCst), 1);
    assert_eq!(push_count.load(Ordering::SeqCst), 0);
    assert!(store
        .has_outbox_head(SyncCollection::Tasks, record_id)
        .unwrap());
    assert!(store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap().is_none());
    assert!(store
        .get_setting(taskveil_sync::SYNC_UPGRADE_REQUIRED_METADATA_KEY)
        .unwrap()
        .is_some());

    assert_eq!(
        run_sync_now(context, &mut store, &mut now).await,
        Err("upgrade required".to_string())
    );
    assert_eq!(preflight_count.load(Ordering::SeqCst), 1);
    assert_eq!(push_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_preflight_fault_blocks_its_http_request_and_every_later_boundary() {
    const DB_KEY: [u8; 32] = [0xd5; 32];
    let tenant_id = Uuid::now_v7();
    let device_id = Uuid::now_v7();
    let preflight_count = Arc::new(AtomicUsize::new(0));
    let pull_count = Arc::new(AtomicUsize::new(0));
    let ack_count = Arc::new(AtomicUsize::new(0));

    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get({
                let counter = preflight_count.clone();
                move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        axum::Json(test_capabilities(
                            tenant_id,
                            taskveil_sync::protocol::SYNC_PROTOCOL_VERSION,
                        ))
                    }
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/pull",
            axum::routing::get({
                let counter = pull_count.clone();
                move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        axum::Json(taskveil_sync::protocol::PullResponse {
                            records: Vec::new(),
                            next_since: 0,
                            has_more: false,
                            high_water: 0,
                            closure_proof: Some(taskveil_sync::protocol::ClosureProof {
                                proof_id: Uuid::now_v7(),
                                tenant_id,
                                device_id,
                                high_water: 0,
                                generation: 0,
                            }),
                        })
                    }
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/continuity/ack",
            axum::routing::post({
                let counter = ack_count.clone();
                move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        axum::Json(taskveil_sync::protocol::ContinuityAckResponse {
                            continuity_seq: 0,
                            continuity_generation: 0,
                        })
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let expected_http_counts = [(0, 0, 0), (1, 0, 0), (1, 1, 0)];
    for (fail_on_call, expected) in (1..=3).zip(expected_http_counts) {
        preflight_count.store(0, Ordering::SeqCst);
        pull_count.store(0, Ordering::SeqCst);
        ack_count.store(0, Ordering::SeqCst);

        let temp = TempDir::new().unwrap();
        let mut store = SqliteSyncStore::new(temp.path().join("fault.sqlite3"), DB_KEY);
        store.fail_preflight_on_call(fail_on_call);
        let mut now = || Ok(Utc::now().timestamp_millis());
        let result = run_sync_now(
            ActiveSyncContext {
                server_url: format!("http://{address}"),
                tenant_id,
                device_id: device_id.to_string(),
                session_token: taskveil_sync::SecretString::new("token"),
                manifest_auth_key: test_manifest_auth_key(),
                keys: LocalSyncKeys {
                    tenant_id,
                    tenant_root_dek: Some([0xd5; 32].into()),
                    tenant_generation: 1,
                    historical_tenant_root_deks: Vec::new(),
                },
            },
            &mut store,
            &mut now,
        )
        .await;

        assert_eq!(result, Err("sync lease lost".to_string()));
        assert_eq!(store.preflight_call_count(), fail_on_call);
        assert_eq!(
            (
                preflight_count.load(Ordering::SeqCst),
                pull_count.load(Ordering::SeqCst),
                ack_count.load(Ordering::SeqCst),
            ),
            expected,
            "the exact faulted boundary and all later HTTP requests must be blocked",
        );
    }
}

#[tokio::test]
async fn continuity_410_still_enforces_protocol_upgrade_before_resync() {
    const DB_KEY: [u8; 32] = [0xd6; 32];
    let tenant_id = Uuid::now_v7();
    #[derive(serde::Deserialize)]
    struct SinceQuery {
        since: i64,
    }

    let preflight_count = Arc::new(AtomicUsize::new(0));
    let start_count = Arc::new(AtomicUsize::new(0));
    let preflight_counter = preflight_count.clone();
    let start_counter = start_count.clone();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(
                move |axum::extract::Query(query): axum::extract::Query<SinceQuery>| {
                    let counter = preflight_counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        if query.since == 1 {
                            let mut capabilities = test_capabilities(
                                tenant_id,
                                taskveil_protocol::sync::SYNC_PROTOCOL_VERSION + 1,
                            );
                            capabilities.gc_horizon_seq = 2;
                            capabilities.continuity_seq = 1;
                            capabilities.required_generation = 1;
                            capabilities.full_resync_required = true;
                            return (StatusCode::GONE, axum::Json(capabilities)).into_response();
                        }
                        let mut capabilities = test_capabilities(
                            tenant_id,
                            taskveil_protocol::sync::SYNC_PROTOCOL_VERSION + 1,
                        );
                        capabilities.gc_horizon_seq = 2;
                        axum::Json(capabilities).into_response()
                    }
                },
            ),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/start",
            axum::routing::post(move || {
                let counter = start_counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::Json(taskveil_protocol::sync::ResyncStartResponse {
                        base_seq: 2,
                        generation: 1,
                        page_token: "page".to_string(),
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("continuity-upgrade.sqlite3");
    let mut repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    repository
        .set_cursor(SYNC_CURSOR_NAME, 1, Utc::now().timestamp_millis())
        .unwrap();
    drop(repository);
    let mut store = SqliteSyncStore::new(db_path, DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());
    assert_eq!(
        run_sync_now(
            ActiveSyncContext {
                server_url: format!("http://{address}"),
                tenant_id,
                device_id: "continuity-upgrade-client".to_string(),
                session_token: taskveil_sync::SecretString::new("token"),
                manifest_auth_key: test_manifest_auth_key(),
                keys: LocalSyncKeys::default(),
            },
            &mut store,
            &mut now,
        )
        .await,
        Err("upgrade required".to_string())
    );
    assert_eq!(preflight_count.load(Ordering::SeqCst), 1);
    assert_eq!(start_count.load(Ordering::SeqCst), 0);
    assert!(store
        .get_setting(taskveil_sync::SYNC_UPGRADE_REQUIRED_METADATA_KEY)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn gc_horizon_full_resync_closes_before_local_outbox_push() {
    const DB_KEY: [u8; 32] = [0xd5; 32];
    let resync_closed = Arc::new(AtomicBool::new(false));
    let push_before_closure = Arc::new(AtomicBool::new(false));
    let start_count = Arc::new(AtomicUsize::new(0));
    let start_counter = start_count.clone();
    let closed_for_pull = resync_closed.clone();
    let closed_for_push = resync_closed.clone();
    let violation = push_before_closure.clone();
    let proof_tenant_id = Uuid::now_v7();
    let proof_device_id = Uuid::now_v7();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(move || async move {
                let mut capabilities = test_capabilities(
                    proof_tenant_id,
                    taskveil_protocol::sync::SYNC_PROTOCOL_VERSION,
                );
                capabilities.gc_horizon_seq = 2;
                capabilities.continuity_seq = 1;
                capabilities.required_generation = 1;
                capabilities.full_resync_required = true;
                (StatusCode::GONE, axum::Json(capabilities))
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/start",
            axum::routing::post(move || {
                let counter = start_counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    axum::Json(taskveil_protocol::sync::ResyncStartResponse {
                        base_seq: 2,
                        generation: 1,
                        page_token: "page".to_string(),
                    })
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/base",
            axum::routing::post(|| async {
                axum::Json(taskveil_protocol::sync::BaseScanResponse {
                    records: Vec::new(),
                    next_cursor: None,
                    has_more: false,
                    next_page_token: None,
                    completion_token: Some("complete".to_string()),
                })
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/base/complete",
            axum::routing::post(|| async {
                axum::Json(taskveil_protocol::sync::CompleteBaseResponse {
                    base_complete: true,
                })
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/pull",
            axum::routing::get(move || {
                let closed = closed_for_pull.clone();
                async move {
                    closed.store(true, Ordering::SeqCst);
                    axum::Json(taskveil_protocol::sync::PullResponse {
                        records: Vec::new(),
                        next_since: 2,
                        has_more: false,
                        high_water: 2,
                        closure_proof: Some(taskveil_protocol::sync::ClosureProof {
                            proof_id: Uuid::now_v7(),
                            tenant_id: proof_tenant_id,
                            device_id: proof_device_id,
                            high_water: 2,
                            generation: 1,
                        }),
                    })
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/push",
            axum::routing::post(
                move |axum::Json(request): axum::Json<taskveil_protocol::sync::PushRequest>| {
                    let closed = closed_for_push.clone();
                    let violation = violation.clone();
                    async move {
                        if !closed.load(Ordering::SeqCst) {
                            violation.store(true, Ordering::SeqCst);
                        }
                        axum::Json(taskveil_protocol::sync::PushResponse {
                            results: request
                                .ops
                                .into_iter()
                                .map(|op| taskveil_protocol::sync::PushResult {
                                    op_id: op.op_id,
                                    record_id: op.record_id,
                                    collection: op.collection,
                                    status: PushStatus::Accepted,
                                    seq: Some(3),
                                    current: None,
                                })
                                .collect(),
                        })
                    }
                },
            ),
        )
        .route(
            "/v2/tenants/{tenant_id}/continuity/ack",
            axum::routing::post(|| async {
                axum::Json(taskveil_protocol::sync::ContinuityAckResponse {
                    continuity_seq: 2,
                    continuity_generation: 1,
                })
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("horizon-before-push.sqlite3");
    let record_id = Uuid::now_v7();
    let revision_hlc = hlc(-100, 0, "horizon-local-revision");
    let mutation_hlc = hlc(-200, 0, "horizon-local-mutation");
    let mut repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    repository
        .set_cursor(SYNC_CURSOR_NAME, 1, Utc::now().timestamp_millis())
        .unwrap();
    repository
        .put_outbox_head(NewSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id,
            collection: "tasks".to_string(),
            base_revision_hlc: None,
            revision_hlc: revision_hlc.clone(),
            state: SyncOutboxState::Live {
                mutation_hlc: mutation_hlc.clone(),
                blob: vec![taskveil_sync::ENVELOPE_VERSION, 1],
            },
            created_at: Utc::now().timestamp_millis(),
        })
        .unwrap();
    repository
        .put_record_state(taskveil_storage::SyncRecordState {
            record_id,
            collection: "tasks".to_string(),
            current_revision_hlc: None,
            state: taskveil_storage::SyncRecordSemanticState::Live {
                mutation_hlc,
                plaintext_json: "{}".to_string(),
            },
            updated_at: Utc::now().timestamp_millis(),
        })
        .unwrap();
    drop(repository);

    let mut store = SqliteSyncStore::new(db_path, DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());
    run_sync_now(
        ActiveSyncContext {
            server_url: format!("http://{address}"),
            tenant_id: proof_tenant_id,
            device_id: "horizon-client".to_string(),
            session_token: taskveil_sync::SecretString::new("token"),
            manifest_auth_key: test_manifest_auth_key(),
            keys: LocalSyncKeys {
                tenant_id: proof_tenant_id,
                tenant_root_dek: Some([0xe7; 32].into()),
                ..LocalSyncKeys::default()
            },
        },
        &mut store,
        &mut now,
    )
    .await
    .unwrap();

    assert_eq!(start_count.load(Ordering::SeqCst), 1);
    assert!(resync_closed.load(Ordering::SeqCst));
    assert!(!push_before_closure.load(Ordering::SeqCst));
    assert!(!store
        .has_outbox_head(SyncCollection::Tasks, record_id)
        .unwrap());
}

#[tokio::test]
async fn expired_terminal_token_restarts_once_and_converges_on_new_generation() {
    const DB_KEY: [u8; 32] = [0xd7; 32];
    let tenant_id = Uuid::now_v7();
    let device_id = Uuid::now_v7();
    let starts = Arc::new(AtomicUsize::new(0));
    let completions = Arc::new(AtomicUsize::new(0));
    let start_counter = starts.clone();
    let completion_counter = completions.clone();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(move || async move {
                let mut capabilities = test_capabilities(
                    tenant_id,
                    taskveil_sync::protocol::SYNC_PROTOCOL_VERSION,
                );
                capabilities.gc_horizon_seq = 1;
                capabilities.required_generation = 1;
                capabilities.full_resync_required = true;
                (StatusCode::GONE, axum::Json(capabilities))
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/start",
            axum::routing::post(move || {
                let counter = start_counter.clone();
                async move {
                    let generation = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    axum::Json(taskveil_sync::protocol::ResyncStartResponse {
                        base_seq: 0,
                        generation: generation as i64,
                        page_token: format!("page-{generation}"),
                    })
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/base",
            axum::routing::post(
                |axum::Json(request): axum::Json<
                    taskveil_sync::protocol::BaseScanRequest,
                >| async move {
                    let generation = request.page_token.trim_start_matches("page-");
                    axum::Json(taskveil_sync::protocol::BaseScanResponse {
                        records: Vec::new(),
                        next_cursor: None,
                        has_more: false,
                        next_page_token: None,
                        completion_token: Some(format!("complete-{generation}")),
                    })
                },
            ),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/base/complete",
            axum::routing::post(
                move |axum::Json(_request): axum::Json<
                    taskveil_sync::protocol::CompleteBaseRequest,
                >| {
                    let counter = completion_counter.clone();
                    async move {
                        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                            (
                                StatusCode::CONFLICT,
                                axum::Json(json!({"error": "expired resync token"})),
                            )
                                .into_response()
                        } else {
                            axum::Json(taskveil_sync::protocol::CompleteBaseResponse {
                                base_complete: true,
                            })
                            .into_response()
                        }
                    }
                },
            ),
        )
        .route(
            "/v2/tenants/{tenant_id}/pull",
            axum::routing::get(move || async move {
                axum::Json(taskveil_sync::protocol::PullResponse {
                    records: Vec::new(),
                    next_since: 0,
                    has_more: false,
                    high_water: 0,
                    closure_proof: Some(taskveil_sync::protocol::ClosureProof {
                        proof_id: Uuid::now_v7(),
                        tenant_id,
                        device_id,
                        high_water: 0,
                        generation: 2,
                    }),
                })
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/continuity/ack",
            axum::routing::post(|| async {
                axum::Json(taskveil_sync::protocol::ContinuityAckResponse {
                    continuity_seq: 0,
                    continuity_generation: 2,
                })
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/push",
            axum::routing::post(|| async {
                axum::Json(taskveil_sync::protocol::PushResponse {
                    results: Vec::new(),
                })
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("expired-terminal-restart.sqlite3");
    let mut store = SqliteSyncStore::new(db_path, DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());
    run_sync_now(
        ActiveSyncContext {
            server_url: format!("http://{address}"),
            tenant_id,
            device_id: device_id.to_string(),
            session_token: taskveil_sync::SecretString::new("token"),
            manifest_auth_key: test_manifest_auth_key(),
            keys: LocalSyncKeys {
                tenant_id,
                tenant_root_dek: Some([0xd7; 32].into()),
                tenant_generation: 1,
                historical_tenant_root_deks: Vec::new(),
            },
        },
        &mut store,
        &mut now,
    )
    .await
    .unwrap();

    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(completions.load(Ordering::SeqCst), 2);
    assert!(store.load_full_resync().unwrap().is_none());
    assert_eq!(
        store
            .get_setting(taskveil_sync::SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY)
            .unwrap()
            .as_deref(),
        Some("")
    );
    assert_eq!(
        store
            .get_setting(taskveil_sync::SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY)
            .unwrap()
            .as_deref(),
        Some("")
    );
}

#[tokio::test]
async fn repeated_invalid_page_token_conflict_does_not_restart_forever() {
    const DB_KEY: [u8; 32] = [0xd8; 32];
    let tenant_id = Uuid::now_v7();
    let starts = Arc::new(AtomicUsize::new(0));
    let scans = Arc::new(AtomicUsize::new(0));
    let start_counter = starts.clone();
    let scan_counter = scans.clone();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(move || async move {
                let mut capabilities =
                    test_capabilities(tenant_id, taskveil_sync::protocol::SYNC_PROTOCOL_VERSION);
                capabilities.full_resync_required = true;
                capabilities.required_generation = 1;
                (StatusCode::GONE, axum::Json(capabilities))
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/start",
            axum::routing::post(move || {
                let counter = start_counter.clone();
                async move {
                    let generation = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    axum::Json(taskveil_sync::protocol::ResyncStartResponse {
                        base_seq: 0,
                        generation: generation as i64,
                        page_token: format!("invalid-page-{generation}"),
                    })
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/resync/base",
            axum::routing::post(move || {
                let counter = scan_counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::CONFLICT,
                        axum::Json(json!({"error": "invalid resync page token"})),
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let mut store =
        SqliteSyncStore::new(temp.path().join("bounded-resync-restart.sqlite3"), DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());
    assert_eq!(
        run_sync_now(
            ActiveSyncContext {
                server_url: format!("http://{address}"),
                tenant_id,
                device_id: Uuid::now_v7().to_string(),
                session_token: taskveil_sync::SecretString::new("token"),
                manifest_auth_key: test_manifest_auth_key(),
                keys: LocalSyncKeys {
                    tenant_id,
                    tenant_root_dek: Some([0xd8; 32].into()),
                    tenant_generation: 1,
                    historical_tenant_root_deks: Vec::new(),
                },
            },
            &mut store,
            &mut now,
        )
        .await,
        Err("sync failed".to_string())
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(scans.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn closure_ack_failure_keeps_local_commit_and_retries_before_push() {
    const DB_KEY: [u8; 32] = [0xd6; 32];
    let tenant_id = Uuid::now_v7();
    let proof_device_id = Uuid::now_v7();
    let ack_attempts = Arc::new(AtomicUsize::new(0));
    let pushed = Arc::new(AtomicBool::new(false));
    let ack_counter = ack_attempts.clone();
    let pushed_flag = pushed.clone();
    let app = Router::new()
        .route(
            "/v2/tenants/{tenant_id}/preflight",
            axum::routing::get(move || async move {
                axum::Json(test_capabilities(
                    tenant_id,
                    taskveil_protocol::sync::SYNC_PROTOCOL_VERSION,
                ))
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/pull",
            axum::routing::get(move || async move {
                axum::Json(taskveil_protocol::sync::PullResponse {
                    records: Vec::new(),
                    next_since: 1,
                    has_more: false,
                    high_water: 1,
                    closure_proof: Some(taskveil_protocol::sync::ClosureProof {
                        proof_id: Uuid::now_v7(),
                        tenant_id,
                        device_id: proof_device_id,
                        high_water: 1,
                        generation: 0,
                    }),
                })
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/continuity/ack",
            axum::routing::post(move || {
                let counter = ack_counter.clone();
                async move {
                    if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "retry").into_response();
                    }
                    axum::Json(taskveil_protocol::sync::ContinuityAckResponse {
                        continuity_seq: 1,
                        continuity_generation: 0,
                    })
                    .into_response()
                }
            }),
        )
        .route(
            "/v2/tenants/{tenant_id}/push",
            axum::routing::post(
                move |axum::Json(request): axum::Json<taskveil_protocol::sync::PushRequest>| {
                    let pushed = pushed_flag.clone();
                    async move {
                        pushed.store(true, Ordering::SeqCst);
                        axum::Json(taskveil_protocol::sync::PushResponse {
                            results: request
                                .ops
                                .into_iter()
                                .map(|op| taskveil_protocol::sync::PushResult {
                                    op_id: op.op_id,
                                    record_id: op.record_id,
                                    collection: op.collection,
                                    status: PushStatus::Accepted,
                                    seq: Some(2),
                                    current: None,
                                })
                                .collect(),
                        })
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("ack-crash.sqlite3");
    let record_id = Uuid::now_v7();
    let revision_hlc = hlc(-100, 0, "ack-crash-revision");
    let mutation_hlc = hlc(-200, 0, "ack-crash-mutation");
    let mut repository = SqliteSyncStateRepository::new(open_encrypted(&db_path, &DB_KEY).unwrap());
    repository
        .put_record_state(taskveil_storage::SyncRecordState {
            record_id,
            collection: "tasks".to_string(),
            current_revision_hlc: None,
            state: taskveil_storage::SyncRecordSemanticState::Live {
                mutation_hlc: mutation_hlc.clone(),
                plaintext_json: "{}".to_string(),
            },
            updated_at: 1,
        })
        .unwrap();
    repository
        .put_outbox_head(NewSyncOutboxEntry {
            op_id: Uuid::now_v7(),
            record_id,
            collection: "tasks".to_string(),
            base_revision_hlc: None,
            revision_hlc,
            state: SyncOutboxState::Live {
                mutation_hlc,
                blob: vec![taskveil_sync::ENVELOPE_VERSION, 1],
            },
            created_at: 1,
        })
        .unwrap();
    drop(repository);
    let context = ActiveSyncContext {
        server_url: format!("http://{address}"),
        tenant_id,
        device_id: "ack-crash".to_string(),
        session_token: taskveil_sync::SecretString::new("token"),
        manifest_auth_key: test_manifest_auth_key(),
        keys: LocalSyncKeys {
            tenant_id,
            tenant_root_dek: Some([0xe7; 32].into()),
            ..LocalSyncKeys::default()
        },
    };
    let mut store = SqliteSyncStore::new(db_path, DB_KEY);
    let mut now = || Ok(Utc::now().timestamp_millis());

    assert_eq!(
        run_sync_now(context.clone(), &mut store, &mut now).await,
        Err("sync failed".to_string())
    );
    assert_eq!(store.get_cursor_seq(SYNC_CURSOR_NAME).unwrap(), Some(1));
    assert!(store
        .has_outbox_head(SyncCollection::Tasks, record_id)
        .unwrap());
    assert!(!pushed.load(Ordering::SeqCst));

    run_sync_now(context, &mut store, &mut now).await.unwrap();
    assert_eq!(ack_attempts.load(Ordering::SeqCst), 2);
    assert!(pushed.load(Ordering::SeqCst));
    assert!(!store
        .has_outbox_head(SyncCollection::Tasks, record_id)
        .unwrap());
}

#[tokio::test]
async fn production_two_client_distinct_fields_and_due_mode_conflict_converge() {
    const DB_KEY_A: [u8; 32] = [0xa1; 32];
    const DB_KEY_B: [u8; 32] = [0xb2; 32];
    const MASTER_KEY: [u8; 32] = [0xa3; 32];
    let fixture = Fixture::setup().await;
    let (device_b, token_b) = fixture.register_device().await;
    let server_url = fixture.serve().await;
    let temp = TempDir::new().unwrap();
    let path_a = temp.path().join("client-a.sqlite3");
    let path_b = temp.path().join("client-b.sqlite3");
    let now = Utc::now().timestamp_millis() - 10_000;
    let list = taskveil_domain::new_list(
        "Shared".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    for (path, key) in [(&path_a, &DB_KEY_A), (&path_b, &DB_KEY_B)] {
        SqliteListRepository::new(open_encrypted(path, key).unwrap())
            .insert(list.clone())
            .unwrap();
    }
    let tenant_key = [0xe7; 32];
    let sync_a = LocalMutationContext {
        device_id: fixture.auth.device_id.to_string(),
        keys: LocalSyncKeys {
            tenant_id: fixture.tenant_id,
            tenant_root_dek: Some(tenant_key.into()),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        },
    };
    let sync_b = LocalMutationContext {
        device_id: device_b.to_string(),
        keys: sync_a.keys.clone(),
    };
    persist_local_crypto_context(
        &path_a,
        &DB_KEY_A,
        LocalCryptoIdentity {
            tenant_id: fixture.tenant_id,
            user_id: fixture.auth.user_id,
            device_id: fixture.auth.device_id,
        },
        &MASTER_KEY,
        sync_a.keys.clone(),
        now,
    )
    .unwrap();
    let mut store_a = SqliteSyncStore::new(path_a.clone(), DB_KEY_A);
    let mut seed_now = || Ok(now + 1);
    taskveil_sync::enqueue_list_sync(
        &mut store_a,
        &sync_a.keys,
        &sync_a.device_id,
        &list,
        false,
        &mut seed_now,
    )
    .unwrap();
    let client_a = SqliteMutationService::new(path_a.clone(), DB_KEY_A);
    let client_b = SqliteMutationService::new(path_b.clone(), DB_KEY_B);
    let task = client_a
        .create_task(
            taskveil_client::test_support::CreateTaskInput {
                list_id: list.id,
                title: "Base title".to_string(),
                parent_task_id: None,
                due: None,
                note: Some("Base note".to_string()),
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 1,
            },
            &sync_a,
        )
        .unwrap();
    let context = |device_id: &str, token: &str, keys: LocalSyncKeys| ActiveSyncContext {
        server_url: server_url.clone(),
        tenant_id: fixture.tenant_id,
        device_id: device_id.to_string(),
        session_token: taskveil_sync::SecretString::new(token),
        manifest_auth_key: test_manifest_auth_key(),
        keys,
    };
    let mut clock = now + 100;
    let mut ticking_now = || {
        clock += 1;
        Ok(clock)
    };
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    let mut store_b = SqliteSyncStore::new(path_b.clone(), DB_KEY_B);
    run_sync_now(
        context(&sync_b.device_id, &token_b, sync_b.keys.clone()),
        &mut store_b,
        &mut ticking_now,
    )
    .await
    .unwrap();
    assert_eq!(
        SqliteTaskRepository::new(open_encrypted(&path_b, &DB_KEY_B).unwrap())
            .get(task.id)
            .unwrap()
            .content
            .title,
        "Base title"
    );

    client_a
        .update_task(
            UpdateTaskInput {
                task_id: task.id,
                title: "Title from A".to_string(),
                note: "Base note".to_string(),
                priority: 0,
                due: Some(taskveil_domain::TaskDue::date("2026-07-12").unwrap()),
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 200,
            },
            &sync_a,
        )
        .unwrap();
    client_b
        .update_task(
            UpdateTaskInput {
                task_id: task.id,
                title: "Base title".to_string(),
                note: "Note from B".to_string(),
                priority: 0,
                due: Some(
                    taskveil_domain::TaskDue::date_time(now + 86_400_000, "Asia/Tokyo").unwrap(),
                ),
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 201,
            },
            &sync_b,
        )
        .unwrap();

    let first = run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    assert_eq!(first.push_acked_count, 1);
    drop(store_a);
    let second = run_sync_now(
        context(&sync_b.device_id, &token_b, sync_b.keys.clone()),
        &mut store_b,
        &mut ticking_now,
    )
    .await
    .unwrap();
    assert_eq!(second.push_conflict_count, 0);
    assert!(second.repush_count >= 1);
    assert!(second.push_acked_count >= 1);

    let row = query(
        "SELECT encrypted_blob FROM sync_records
         WHERE tenant_id = $1 AND collection = 'tasks' AND record_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(task.id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    let blob: Vec<u8> = row.get("encrypted_blob");
    let plaintext =
        decrypt_plaintext(&tenant_key, fixture.tenant_id, 1, "tasks", task.id, &blob).unwrap();
    let SyncPlaintext::Task(plaintext) = plaintext else {
        panic!("task plaintext");
    };
    assert_eq!(plaintext.title.value, "Title from A");
    assert_eq!(plaintext.note.value, "Note from B");
    let expected_due =
        Some(taskveil_domain::TaskDue::date_time(now + 86_400_000, "Asia/Tokyo").unwrap());
    assert_eq!(plaintext.due.value, expected_due.clone());

    let mut store_a = SqliteSyncStore::new(path_a.clone(), DB_KEY_A);
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    for (path, key) in [(&path_a, &DB_KEY_A), (&path_b, &DB_KEY_B)] {
        assert_eq!(
            SqliteTaskRepository::new(open_encrypted(path, key).unwrap())
                .get(task.id)
                .unwrap()
                .due,
            expected_due.clone()
        );
    }
}

#[tokio::test]
async fn equal_rank_clients_converge_then_common_reorder_rebalances_and_reconverges() {
    const DB_KEY_A: [u8; 32] = [0xc1; 32];
    const DB_KEY_B: [u8; 32] = [0xd2; 32];
    const MASTER_KEY: [u8; 32] = [0xc3; 32];
    let fixture = Fixture::setup().await;
    let (device_b, token_b) = fixture.register_device().await;
    let server_url = fixture.serve().await;
    let temp = TempDir::new().unwrap();
    let path_a = temp.path().join("rank-client-a.sqlite3");
    let path_b = temp.path().join("rank-client-b.sqlite3");
    let now = Utc::now().timestamp_millis() - 10_000;
    let list = taskveil_domain::new_list(
        "Shared ranks".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    for (path, key) in [(&path_a, &DB_KEY_A), (&path_b, &DB_KEY_B)] {
        SqliteListRepository::new(open_encrypted(path, key).unwrap())
            .insert(list.clone())
            .unwrap();
    }
    let tenant_key = [0xe7; 32];
    let sync_a = LocalMutationContext {
        device_id: fixture.auth.device_id.to_string(),
        keys: LocalSyncKeys {
            tenant_id: fixture.tenant_id,
            tenant_root_dek: Some(tenant_key.into()),
            tenant_generation: 1,
            historical_tenant_root_deks: Vec::new(),
        },
    };
    let sync_b = LocalMutationContext {
        device_id: device_b.to_string(),
        keys: sync_a.keys.clone(),
    };
    persist_local_crypto_context(
        &path_a,
        &DB_KEY_A,
        LocalCryptoIdentity {
            tenant_id: fixture.tenant_id,
            user_id: fixture.auth.user_id,
            device_id: fixture.auth.device_id,
        },
        &MASTER_KEY,
        sync_a.keys.clone(),
        now,
    )
    .unwrap();
    let mut store_a = SqliteSyncStore::new(path_a.clone(), DB_KEY_A);
    let mut seed_now = || Ok(now + 1);
    taskveil_sync::enqueue_list_sync(
        &mut store_a,
        &sync_a.keys,
        &sync_a.device_id,
        &list,
        false,
        &mut seed_now,
    )
    .unwrap();
    let client_a = SqliteMutationService::new(path_a.clone(), DB_KEY_A);
    let client_b = SqliteMutationService::new(path_b.clone(), DB_KEY_B);
    let target = client_a
        .create_task(
            taskveil_client::test_support::CreateTaskInput {
                list_id: list.id,
                title: "reorder target".to_string(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 1,
            },
            &sync_a,
        )
        .unwrap();
    let context = |device_id: &str, token: &str, keys: LocalSyncKeys| ActiveSyncContext {
        server_url: server_url.clone(),
        tenant_id: fixture.tenant_id,
        device_id: device_id.to_string(),
        session_token: taskveil_sync::SecretString::new(token),
        manifest_auth_key: test_manifest_auth_key(),
        keys,
    };
    let mut clock = now + 100;
    let mut ticking_now = || {
        clock += 1;
        Ok(clock)
    };
    let mut store_b = SqliteSyncStore::new(path_b.clone(), DB_KEY_B);
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    run_sync_now(
        context(&sync_b.device_id, &token_b, sync_b.keys.clone()),
        &mut store_b,
        &mut ticking_now,
    )
    .await
    .unwrap();

    let concurrent_a = client_a
        .create_task(
            taskveil_client::test_support::CreateTaskInput {
                list_id: list.id,
                title: "same gap A".to_string(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 200,
            },
            &sync_a,
        )
        .unwrap();
    let concurrent_b = client_b
        .create_task(
            taskveil_client::test_support::CreateTaskInput {
                list_id: list.id,
                title: "same gap B".to_string(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 201,
            },
            &sync_b,
        )
        .unwrap();
    assert_eq!(concurrent_a.sort_order, concurrent_b.sort_order);
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    run_sync_now(
        context(&sync_b.device_id, &token_b, sync_b.keys.clone()),
        &mut store_b,
        &mut ticking_now,
    )
    .await
    .unwrap();
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    let order = |path: &std::path::Path, key: &[u8; 32]| {
        SqliteTaskRepository::new(open_encrypted(path, key).unwrap())
            .list_active_by_list(list.id)
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&path_a, &DB_KEY_A), order(&path_b, &DB_KEY_B));
    let (previous, next) = if concurrent_a.id < concurrent_b.id {
        (concurrent_a.id, concurrent_b.id)
    } else {
        (concurrent_b.id, concurrent_a.id)
    };
    let repository_a = SqliteTaskRepository::new(open_encrypted(&path_a, &DB_KEY_A).unwrap());
    assert!(
        repository_a.get(target.id).is_ok(),
        "target missing: {}",
        target.id
    );
    assert!(
        repository_a.get(concurrent_a.id).is_ok(),
        "concurrent A missing: {}",
        concurrent_a.id
    );
    assert!(
        repository_a.get(concurrent_b.id).is_ok(),
        "concurrent B missing: {}",
        concurrent_b.id
    );
    assert!(
        repository_a.get(previous).is_ok(),
        "previous missing: {previous}"
    );
    assert!(repository_a.get(next).is_ok(), "next missing: {next}");

    client_a
        .reorder_task(
            taskveil_client::test_support::ReorderTaskInput {
                task_id: target.id,
                previous_task_id: Some(previous),
                next_task_id: Some(next),
                now_ms: now + 300,
            },
            &sync_a,
        )
        .unwrap();
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();
    run_sync_now(
        context(&sync_b.device_id, &token_b, sync_b.keys.clone()),
        &mut store_b,
        &mut ticking_now,
    )
    .await
    .unwrap();
    run_sync_now(
        context(&sync_a.device_id, &fixture.token, sync_a.keys.clone()),
        &mut store_a,
        &mut ticking_now,
    )
    .await
    .unwrap();

    let order_a = order(&path_a, &DB_KEY_A);
    let order_b = order(&path_b, &DB_KEY_B);
    assert_eq!(order_a, order_b);
    assert_eq!(order_a, vec![previous, target.id, next]);
    let ranks = SqliteTaskRepository::new(open_encrypted(&path_a, &DB_KEY_A).unwrap())
        .list_active_by_list(list.id)
        .unwrap()
        .into_iter()
        .map(|task| task.sort_order)
        .collect::<Vec<_>>();
    assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn remote_list_deletion_rehomes_offline_descendant_and_republishes_it() {
    const DB_KEY: [u8; 32] = [0xe1; 32];
    let fixture = Fixture::setup().await;
    let server_url = fixture.serve().await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("deletion-cascade.sqlite3");
    let now = Utc::now().timestamp_millis() - 10_000;
    let list = taskveil_domain::new_list(
        "Deleted remotely".to_string(),
        "7fffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    let mut default_list = taskveil_domain::new_list(
        "Inbox".to_string(),
        "bfffffffffffffffffffffffffffffff".to_string(),
        now,
    )
    .unwrap();
    default_list.is_default = true;
    let mut lists = SqliteListRepository::new(open_encrypted(&path, &DB_KEY).unwrap());
    lists.insert(list.clone()).unwrap();
    lists.insert(default_list.clone()).unwrap();
    let keys = LocalSyncKeys {
        tenant_id: fixture.tenant_id,
        tenant_root_dek: Some([0xe7; 32].into()),
        tenant_generation: 1,
        historical_tenant_root_deks: Vec::new(),
    };
    let mutation = hlc(-4_000, 0, "list-live");
    let live_revision = fixture.revision_hlc(-3_900, 0);
    assert_eq!(
        fixture
            .push(PushOp {
                op_id: Uuid::now_v7(),
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: None,
                revision_hlc: live_revision.clone(),
                state: SyncRecordState::Live {
                    mutation_hlc: mutation,
                    blob: STANDARD.encode(structural_envelope(b"opaque-list")),
                },
            })
            .await
            .status,
        PushStatus::Accepted
    );
    let delete_hlc = hlc(-3_000, 0, "list-delete");
    let delete_revision = fixture.revision_hlc(-2_900, 0);
    assert_eq!(
        fixture
            .push(PushOp {
                op_id: Uuid::now_v7(),
                record_id: list.id,
                collection: SyncCollection::Lists,
                base_revision_hlc: Some(live_revision),
                revision_hlc: delete_revision,
                state: SyncRecordState::Tombstone { delete_hlc },
            })
            .await
            .status,
        PushStatus::Accepted
    );

    let mutation_service = SqliteMutationService::new(path.clone(), DB_KEY);
    let late_task = mutation_service
        .create_task(
            taskveil_client::test_support::CreateTaskInput {
                list_id: list.id,
                title: "offline late descendant".to_string(),
                parent_task_id: None,
                due: None,
                note: None,
                priority: 0,
                scheduled_at: None,
                estimated_minutes: None,
                now_ms: now + 1,
            },
            &LocalMutationContext {
                device_id: fixture.auth.device_id.to_string(),
                keys: keys.clone(),
            },
        )
        .unwrap();
    let mut store = SqliteSyncStore::new(path.clone(), DB_KEY);
    let mut clock = now + 100;
    let mut ticking_now = || {
        clock += 1;
        Ok(clock)
    };
    run_sync_now(
        ActiveSyncContext {
            server_url,
            tenant_id: fixture.tenant_id,
            device_id: fixture.auth.device_id.to_string(),
            session_token: taskveil_sync::SecretString::new(fixture.token.clone()),
            manifest_auth_key: test_manifest_auth_key(),
            keys,
        },
        &mut store,
        &mut ticking_now,
    )
    .await
    .unwrap();

    assert_eq!(
        SqliteTaskRepository::new(open_encrypted(&path, &DB_KEY).unwrap())
            .get(late_task.id)
            .unwrap()
            .list_id,
        default_list.id
    );
    let row = query(
        "SELECT delete_hlc, encrypted_blob FROM sync_records
         WHERE tenant_id = $1 AND record_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(late_task.id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<Option<String>, _>("delete_hlc").unwrap(),
        None
    );
    assert!(row
        .try_get::<Option<Vec<u8>>, _>("encrypted_blob")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn rotation_activation_is_atomic_stale_writes_fail_and_retirement_waits() {
    const MASTER_KEY: [u8; 32] = [0x41; 32];
    let fixture = Fixture::setup().await;
    fixture.close_continuity().await;
    let offline_device_id = Uuid::now_v7();
    let expired_device_id = Uuid::now_v7();
    query(
        "INSERT INTO devices
            (id, user_id, device_name, certificate, certified_at, key_expires_at)
         VALUES ($1, $3, 'offline-device', '\\x00'::bytea, now(), NULL),
                ($2, $3, 'expired-device', '\\x00'::bytea, now(), NULL)",
    )
    .bind(offline_device_id)
    .bind(expired_device_id)
    .bind(fixture.auth.user_id)
    .execute(&fixture.admin_pool)
    .await
    .unwrap();
    sync::set_device_key_expiry(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        expired_device_id,
        sync::DeviceKeyExpiryRequest {
            expires_at: Some(Utc::now() - Duration::seconds(1)),
        },
    )
    .await
    .unwrap();
    query(
        "UPDATE tenant_key_generations SET wrapped_tenant_root_dek = $2
         WHERE tenant_id = $1 AND generation = 1",
    )
    .bind(fixture.tenant_id)
    .bind(
        wrap_tenant_root_dek_with_master_key(fixture.tenant_id, 1, &[0x21; 32], &MASTER_KEY)
            .unwrap(),
    )
    .execute(&fixture.admin_pool)
    .await
    .unwrap();

    let record_id = Uuid::now_v7();
    let old_revision = fixture.revision_hlc(-2_000, 0);
    assert_eq!(
        fixture
            .push(live_op(
                record_id,
                None,
                old_revision.clone(),
                hlc(-2_100, 0, "rotation-old-mutation"),
                b"generation-one",
            ))
            .await
            .status,
        PushStatus::Accepted
    );

    let tenant_generation_one = active_manifest(fixture.tenant_id);
    let tenant_prepared_manifest = signed_manifest_after(
        fixture.tenant_id,
        2,
        RotationStatus::Prepared,
        1,
        &tenant_generation_one,
    );
    let tenant_active_manifest = signed_manifest_after(
        fixture.tenant_id,
        2,
        RotationStatus::Active,
        2,
        &tenant_prepared_manifest,
    );

    let prepared = sync::prepare_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::PrepareRotationRequest {
            suite_id: CRYPTO_SUITE_ID,
            generation: 2,
            signed_manifest: STANDARD.encode(&tenant_prepared_manifest),
            wrapped_tenant_root_dek: STANDARD.encode(
                wrap_tenant_root_dek_with_master_key(
                    fixture.tenant_id,
                    2,
                    &[0x22; 32],
                    &MASTER_KEY,
                )
                .unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    assert_eq!(prepared.active_generation, 1);
    let active_before_crash: i64 = query(
        "SELECT generation FROM tenant_key_generations
         WHERE tenant_id = $1 AND status = 'active'",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("generation")
    .unwrap();
    assert_eq!(active_before_crash, 1);

    let activated = sync::activate_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::ActivateRotationRequest {
            generation: 2,
            signed_manifest: STANDARD.encode(&tenant_active_manifest),
        },
    )
    .await
    .unwrap();
    assert_eq!(activated.active_generation, 2);
    assert_eq!(activated.live_heads_remaining, 1);
    fixture.close_continuity().await;
    let overlapping = sync::prepare_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::PrepareRotationRequest {
            suite_id: CRYPTO_SUITE_ID,
            generation: 3,
            signed_manifest: STANDARD.encode(signed_manifest_after(
                fixture.tenant_id,
                3,
                RotationStatus::Prepared,
                2,
                &tenant_active_manifest,
            )),
            wrapped_tenant_root_dek: STANDARD.encode(
                wrap_tenant_root_dek_with_master_key(
                    fixture.tenant_id,
                    3,
                    &[0x32; 32],
                    &MASTER_KEY,
                )
                .unwrap(),
            ),
        },
    )
    .await;
    assert!(overlapping.is_err());

    let stale = live_op(
        record_id,
        Some(old_revision.clone()),
        fixture.revision_hlc(-1_800, 0),
        hlc(-1_900, 0, "rotation-stale-mutation"),
        b"stale-generation",
    );
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest { ops: vec![stale] },
    )
    .await
    .is_err());

    let new_revision = fixture.revision_hlc(-1_600, 0);
    let migrated = PushOp {
        op_id: Uuid::now_v7(),
        record_id,
        collection: SyncCollection::Tasks,
        base_revision_hlc: Some(old_revision),
        revision_hlc: new_revision,
        state: SyncRecordState::Live {
            mutation_hlc: hlc(-1_700, 0, "rotation-new-mutation"),
            blob: STANDARD.encode(structural_envelope_for_generation(2, b"generation-two")),
        },
    };
    assert_eq!(
        sync::push(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            PushRequest {
                ops: vec![migrated]
            },
        )
        .await
        .unwrap()
        .results[0]
            .status,
        PushStatus::Accepted
    );

    let server_url = fixture.serve().await;
    let preflight =
        taskveil_sync::SyncEngine::new(server_url, fixture.tenant_id, fixture.token.clone())
            .unwrap()
            .preflight(0)
            .await
            .unwrap();
    assert_eq!(preflight.active_key_generation, 2);
    assert_eq!(preflight.minimum_write_generation, 2);
    let active_bundle = AccountClient::new(fixture.serve().await)
        .unwrap()
        .active_key_bundle(fixture.tenant_id, &fixture.token)
        .await
        .unwrap();
    let tenant_dek = taskveil_sync::account::unwrap_active_key_bundle(
        fixture.tenant_id,
        &active_bundle,
        &MASTER_KEY,
    )
    .unwrap();
    assert_eq!(*tenant_dek, [0x22; 32]);
    let historical = taskveil_sync::account::unwrap_historical_key_bundles(
        fixture.tenant_id,
        &active_bundle.migrating_generations,
        &MASTER_KEY,
    )
    .unwrap();
    assert_eq!(historical.len(), 1);
    assert_eq!(historical[0].generation, 1);
    assert_eq!(*historical[0].tenant_root_dek, [0x21; 32]);

    sync::acknowledge_key_generation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::RotationGenerationRequest { generation: 2 },
    )
    .await
    .unwrap();
    assert!(sync::retire_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::RotationGenerationRequest { generation: 2 },
    )
    .await
    .is_err());
    query(
        "UPDATE tenant_key_generations
         SET history_retain_until = now() - interval '1 second'
         WHERE tenant_id = $1 AND generation = 1",
    )
    .bind(fixture.tenant_id)
    .execute(&fixture.admin_pool)
    .await
    .unwrap();
    assert!(sync::retire_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::RotationGenerationRequest { generation: 2 },
    )
    .await
    .is_err());
    sync::acknowledge_key_generation(
        &fixture.pool,
        fixture.tenant_id,
        AuthContext {
            user_id: fixture.auth.user_id,
            device_id: offline_device_id,
        },
        sync::RotationGenerationRequest { generation: 2 },
    )
    .await
    .unwrap();
    sync::retire_rotation(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        sync::RotationGenerationRequest { generation: 2 },
    )
    .await
    .unwrap();
    let retired = query(
        "SELECT status, octet_length(wrapped_tenant_root_dek) AS wrapped_len
         FROM tenant_key_generations WHERE tenant_id = $1 AND generation = 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert_eq!(retired.try_get::<String, _>("status").unwrap(), "retired");
    assert_eq!(retired.try_get::<i32, _>("wrapped_len").unwrap(), 0);
}

fn hlc(delta: i64, counter: u32, device: &str) -> String {
    Hlc {
        wall_ms: Utc::now().timestamp_millis() + delta,
        counter,
        device_id: device.to_string(),
    }
    .encode()
    .unwrap()
}

fn live_op(
    record_id: Uuid,
    base_revision_hlc: Option<String>,
    revision_hlc: String,
    mutation_hlc: String,
    blob: &[u8],
) -> PushOp {
    PushOp {
        op_id: Uuid::now_v7(),
        record_id,
        collection: SyncCollection::Tasks,
        base_revision_hlc,
        revision_hlc,
        state: SyncRecordState::Live {
            mutation_hlc,
            blob: STANDARD.encode(structural_envelope(blob)),
        },
    }
}

fn structural_envelope(payload: &[u8]) -> Vec<u8> {
    structural_envelope_for_generation(1, payload)
}

fn structural_envelope_for_generation(generation: u64, payload: &[u8]) -> Vec<u8> {
    let mut envelope = Vec::with_capacity(14 + 24 + payload.len() + 16);
    envelope.extend_from_slice(b"TDE5");
    envelope.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
    envelope.extend_from_slice(&generation.to_be_bytes());
    envelope.extend_from_slice(&[0_u8; 24]);
    envelope.extend_from_slice(payload);
    envelope.extend_from_slice(&[0_u8; 16]);
    envelope
}

fn tombstone_op(
    record_id: Uuid,
    base_revision_hlc: Option<String>,
    revision_hlc: String,
    delete_hlc: String,
) -> PushOp {
    PushOp {
        op_id: Uuid::now_v7(),
        record_id,
        collection: SyncCollection::Tasks,
        base_revision_hlc,
        revision_hlc,
        state: SyncRecordState::Tombstone { delete_hlc },
    }
}

#[tokio::test]
async fn push_rejects_unobservable_or_spoofed_revision_clocks_before_persistence() {
    let fixture = Fixture::setup().await;
    fixture.close_continuity().await;
    let authenticated_node = fixture.auth.device_id.to_string();
    let boundary_id = Uuid::now_v7();
    let no_headroom_id = Uuid::now_v7();
    let maximum_id = Uuid::now_v7();
    let spoofed_id = Uuid::now_v7();

    // Validation covers the whole batch before starting persistence: the valid
    // boundary record must not survive beside a poisoned record.
    let boundary = hlc(-1_000, u32::MAX - 2, &authenticated_node);
    let no_headroom = hlc(-1_000, u32::MAX - 1, &authenticated_node);
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![
                tombstone_op(boundary_id, None, boundary.clone(), boundary.clone()),
                tombstone_op(no_headroom_id, None, no_headroom.clone(), no_headroom,),
            ],
        },
    )
    .await
    .is_err());

    let maximum = hlc(-900, u32::MAX, &authenticated_node);
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![tombstone_op(maximum_id, None, maximum.clone(), maximum,)],
        },
    )
    .await
    .is_err());

    let spoofed = hlc(-800, 0, &Uuid::now_v7().to_string());
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![tombstone_op(spoofed_id, None, spoofed.clone(), spoofed,)],
        },
    )
    .await
    .is_err());

    let rejected_count: i64 = query(
        "SELECT count(*) AS count FROM sync_records
         WHERE tenant_id = $1 AND record_id = ANY($2)",
    )
    .bind(fixture.tenant_id)
    .bind(vec![boundary_id, no_headroom_id, maximum_id, spoofed_id])
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(rejected_count, 0);

    let accepted = sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![tombstone_op(boundary_id, None, boundary.clone(), boundary)],
        },
    )
    .await
    .unwrap();
    assert_eq!(accepted.results[0].status, PushStatus::Accepted);
}

fn timer_live_op(
    record_id: Uuid,
    base_revision_hlc: Option<String>,
    revision_hlc: String,
    mutation_hlc: String,
    blob: &[u8],
) -> PushOp {
    let mut op = live_op(
        record_id,
        base_revision_hlc,
        revision_hlc,
        mutation_hlc,
        blob,
    );
    op.collection = SyncCollection::TimerSessions;
    op
}

fn timer_tombstone_op(
    record_id: Uuid,
    base_revision_hlc: Option<String>,
    revision_hlc: String,
    delete_hlc: String,
) -> PushOp {
    let mut op = tombstone_op(record_id, base_revision_hlc, revision_hlc, delete_hlc);
    op.collection = SyncCollection::TimerSessions;
    op
}

#[tokio::test]
async fn timer_session_live_is_immutable_and_tombstone_is_terminal_and_pullable() {
    let fixture = Fixture::setup().await;
    let record_id = Uuid::now_v7();
    let mutation = hlc(-5_000, 0, "timer-mutation");
    let revision = fixture.revision_hlc(-4_900, 0);
    let create = timer_live_op(
        record_id,
        None,
        revision.clone(),
        mutation.clone(),
        b"opaque-timer-session",
    );

    let accepted = fixture.push(create.clone()).await;
    assert_eq!(accepted.status, PushStatus::Accepted);
    let accepted_seq = accepted.seq.unwrap();

    let mut retry_op = create;
    retry_op.op_id = Uuid::now_v7();
    let retry = fixture.push(retry_op).await;
    assert_eq!(retry.status, PushStatus::NoOp);
    assert_eq!(retry.seq, Some(accepted_seq));

    let conflicting = fixture
        .push(timer_live_op(
            record_id,
            Some(revision.clone()),
            fixture.revision_hlc(-4_000, 0),
            hlc(-4_100, 0, "timer-update-mutation"),
            b"different-opaque-timer-session",
        ))
        .await;
    assert_eq!(conflicting.status, PushStatus::Conflict);
    assert_eq!(conflicting.seq, Some(accepted_seq));
    assert!(matches!(
        conflicting.current.unwrap().state,
        SyncRecordState::Live { .. }
    ));

    let live_pull = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        0,
        Some(100),
        None,
    )
    .await
    .unwrap();
    let live = live_pull
        .records
        .iter()
        .find(|record| record.record_id == record_id)
        .unwrap();
    assert_eq!(live.collection, SyncCollection::TimerSessions);
    assert!(matches!(live.state, SyncRecordState::Live { .. }));

    let resync = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    let base = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &resync.page_token,
        Some(100),
    )
    .await
    .unwrap();
    let stable_timer = base
        .records
        .iter()
        .find(|record| record.record_id == record_id)
        .unwrap();
    assert_eq!(stable_timer.collection, SyncCollection::TimerSessions);
    assert_eq!(
        base.next_cursor.unwrap().collection,
        SyncCollection::TimerSessions
    );

    let delete_revision = fixture.revision_hlc(-3_000, 0);
    let deleted = fixture
        .push(timer_tombstone_op(
            record_id,
            Some(revision),
            delete_revision.clone(),
            hlc(-3_100, 0, "timer-delete"),
        ))
        .await;
    assert_eq!(deleted.status, PushStatus::Accepted);

    let resurrect = fixture
        .push(timer_live_op(
            record_id,
            Some(delete_revision),
            fixture.revision_hlc(-2_000, 0),
            hlc(-2_100, 0, "timer-resurrect-mutation"),
            b"resurrected-timer-session",
        ))
        .await;
    assert_eq!(resurrect.status, PushStatus::Superseded);
    assert!(matches!(
        resurrect.current.unwrap().state,
        SyncRecordState::Tombstone { .. }
    ));

    let tombstone_pull = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        accepted_seq,
        Some(100),
        None,
    )
    .await
    .unwrap();
    let tombstone = tombstone_pull
        .records
        .iter()
        .find(|record| record.record_id == record_id)
        .unwrap();
    assert_eq!(tombstone.collection, SyncCollection::TimerSessions);
    assert!(matches!(tombstone.state, SyncRecordState::Tombstone { .. }));
}

#[tokio::test]
async fn task_series_domain_migration_updates_persisted_record_collection_checks() {
    let fixture = Fixture::setup().await;
    let constraints = query(
        "SELECT conname, pg_get_constraintdef(oid) AS definition
         FROM pg_constraint
         WHERE conname = ANY($1)
         ORDER BY conname",
    )
    .bind(vec![
        "sync_records_collection_check",
        "sync_records_history_collection_check",
    ])
    .fetch_all(&fixture.admin_pool)
    .await
    .unwrap();

    assert_eq!(constraints.len(), 2);
    for row in constraints {
        let definition: String = row.try_get("definition").unwrap();
        assert!(definition.contains("timer_sessions"), "{definition}");
        assert!(definition.contains("templates"), "{definition}");
        assert!(definition.contains("task_series"), "{definition}");
    }
}

#[tokio::test]
async fn cas_retry_semantic_fences_and_pull_preserve_the_current_head() {
    let fixture = Fixture::setup().await;
    let record_id = Uuid::now_v7();
    let mutation_1 = hlc(-4_000, 0, "semantic-a");
    let revision_1 = fixture.revision_hlc(-3_900, 0);
    let create = live_op(
        record_id,
        None,
        revision_1.clone(),
        mutation_1.clone(),
        b"cipher-a",
    );
    let accepted = fixture.push(create.clone()).await;
    assert_eq!(accepted.status, PushStatus::Accepted);
    assert_eq!(accepted.seq, Some(1));

    let retry = fixture.push(create).await;
    assert_eq!(retry.status, PushStatus::NoOp);
    assert_eq!(retry.seq, Some(1));

    let same_revision_different_blob = fixture
        .push(live_op(
            record_id,
            None,
            revision_1.clone(),
            mutation_1.clone(),
            b"same-revision-corruption",
        ))
        .await;
    assert_eq!(same_revision_different_blob.status, PushStatus::Conflict);
    assert_eq!(same_revision_different_blob.seq, Some(1));

    let stale = live_op(
        record_id,
        None,
        fixture.revision_hlc(-3_000, 0),
        hlc(-3_100, 0, "semantic-stale"),
        b"must-not-overwrite",
    );
    let stale_result = fixture.push(stale).await;
    assert_eq!(stale_result.status, PushStatus::Conflict);
    assert_eq!(
        stale_result.current.as_ref().unwrap().revision_hlc,
        revision_1
    );

    let delete_old = tombstone_op(
        record_id,
        Some(revision_1.clone()),
        fixture.revision_hlc(-2_900, 0),
        hlc(-50_000, 0, "delete-old"),
    );
    let superseded = fixture.push(delete_old).await;
    assert_eq!(superseded.status, PushStatus::Superseded);
    assert_eq!(
        superseded.current.as_ref().unwrap().revision_hlc,
        revision_1
    );

    let delete_hlc = hlc(-2_500, 0, "delete-new");
    let delete_revision = fixture.revision_hlc(-2_400, 0);
    let deleted = fixture
        .push(tombstone_op(
            record_id,
            Some(revision_1),
            delete_revision.clone(),
            delete_hlc.clone(),
        ))
        .await;
    assert_eq!(deleted.status, PushStatus::Accepted);
    assert_eq!(deleted.seq, Some(2));

    let live_old = fixture
        .push(live_op(
            record_id,
            Some(delete_revision.clone()),
            fixture.revision_hlc(-2_000, 0),
            mutation_1,
            b"must-stay-deleted",
        ))
        .await;
    assert_eq!(live_old.status, PushStatus::Superseded);
    assert!(matches!(
        live_old.current.unwrap().state,
        SyncRecordState::Tombstone { .. }
    ));

    let resurrected = fixture
        .push(live_op(
            record_id,
            Some(delete_revision.clone()),
            fixture.revision_hlc(-1_500, 0),
            hlc(-1_600, 0, "semantic-live-new"),
            b"cipher-resurrected",
        ))
        .await;
    assert_eq!(resurrected.status, PushStatus::Superseded);
    assert_eq!(resurrected.seq, Some(2));

    let page = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        0,
        Some(100),
        None,
    )
    .await
    .unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].revision_hlc, delete_revision);
    assert!(matches!(
        &page.records[0].state,
        SyncRecordState::Tombstone { .. }
    ));
    assert_eq!(page.next_since, 2);

    let history_count: i64 = query(
        "SELECT count(*) AS count FROM sync_records_history
         WHERE tenant_id = $1 AND record_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(record_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(history_count, 0);
}

#[tokio::test]
async fn server_trusted_continuity_binds_proofs_and_guards_all_writes() {
    let fixture = Fixture::setup().await;
    let record_id = Uuid::now_v7();
    let op = live_op(
        record_id,
        None,
        fixture.revision_hlc(-2_000, 0),
        hlc(-2_100, 0, "continuity-mutation"),
        b"continuity",
    );
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![op.clone()]
        },
    )
    .await
    .is_err());
    let initial = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        0,
        Some(100),
        None,
    )
    .await
    .unwrap();
    let proof = initial.closure_proof.unwrap();
    let initialized_before_ack: bool = query(
        "SELECT initialized FROM tenant_device_continuity
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("initialized")
    .unwrap();
    assert!(!initialized_before_ack);

    let mut wrong_tenant = proof.clone();
    wrong_tenant.tenant_id = Uuid::now_v7();
    assert!(sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: wrong_tenant
        },
    )
    .await
    .is_err());
    let mut wrong_device = proof.clone();
    wrong_device.device_id = Uuid::now_v7();
    assert!(sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: wrong_device
        },
    )
    .await
    .is_err());
    let first_ack = sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: proof.clone(),
        },
    )
    .await
    .unwrap();
    let retried_ack = sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: proof.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(retried_ack, first_ack);

    assert_eq!(
        sync::push(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            PushRequest { ops: vec![op] },
        )
        .await
        .unwrap()
        .results[0]
            .status,
        PushStatus::Accepted
    );
    fixture.close_continuity().await;
    let start = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest { ops: Vec::new() },
    )
    .await
    .is_err());
    let base = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(100),
    )
    .await
    .unwrap();
    assert!(!base.has_more);
    sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        base.completion_token.as_deref().unwrap(),
    )
    .await
    .unwrap();
    let closure = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(100),
        Some(start.generation),
    )
    .await
    .unwrap();
    let full_resync_proof = closure.closure_proof.unwrap();
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest { ops: Vec::new() },
    )
    .await
    .is_err());
    sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: full_resync_proof.clone(),
        },
    )
    .await
    .unwrap();
    let completed_session_count: i64 = query(
        "SELECT count(*) AS count FROM device_resync_sessions
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(
        completed_session_count, 0,
        "a successful continuity ACK removes its completed resync session"
    );
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest { ops: Vec::new() },
    )
    .await
    .is_ok());

    let next = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert!(next.generation > start.generation);
    assert!(sync::ack_continuity(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        taskveil_protocol::sync::ContinuityAckRequest {
            proof: full_resync_proof,
        },
    )
    .await
    .is_err());
}

#[tokio::test]
async fn concurrent_pull_then_ack_obeys_continuity_proof_session_lock_order() {
    let fixture = Fixture::setup().await;
    let initial = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        0,
        Some(100),
        None,
    )
    .await
    .unwrap();
    let proof = initial.closure_proof.unwrap();

    let mut blocker = fixture.admin_pool.begin().await.unwrap();
    query(
        "SELECT 1
         FROM tenant_device_continuity
         WHERE tenant_id = $1 AND device_id = $2
         FOR UPDATE",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();

    let pull_pool = fixture.pool.clone();
    let pull_auth = fixture.auth.clone();
    let tenant_id = fixture.tenant_id;
    let pull_task = tokio::spawn(async move {
        sync::pull(&pull_pool, tenant_id, pull_auth, 0, Some(100), None).await
    });
    wait_for_lock_waiters(&fixture.admin_pool, 1).await;

    let ack_pool = fixture.pool.clone();
    let ack_auth = fixture.auth.clone();
    let ack_proof = proof.clone();
    let ack_task = tokio::spawn(async move {
        sync::ack_continuity(
            &ack_pool,
            tenant_id,
            ack_auth,
            taskveil_sync::protocol::ContinuityAckRequest { proof: ack_proof },
        )
        .await
    });
    wait_for_lock_waiters(&fixture.admin_pool, 2).await;
    blocker.commit().await.unwrap();

    let (pulled, acknowledged) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        (pull_task.await.unwrap(), ack_task.await.unwrap())
    })
    .await
    .expect("pull and ACK must serialize without a deadlock");
    assert_eq!(
        pulled.unwrap().closure_proof.unwrap().proof_id,
        proof.proof_id
    );
    acknowledged.unwrap();
}

#[tokio::test]
async fn concurrent_ack_then_completion_replay_obeys_lock_order_without_deadlock() {
    let fixture = Fixture::setup().await;
    fixture.close_continuity().await;
    let start = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    let base = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(100),
    )
    .await
    .unwrap();
    let completion_token = base.completion_token.unwrap();
    sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &completion_token,
    )
    .await
    .unwrap();
    let proof = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(100),
        Some(start.generation),
    )
    .await
    .unwrap()
    .closure_proof
    .unwrap();

    let mut blocker = fixture.admin_pool.begin().await.unwrap();
    query(
        "SELECT 1
         FROM tenant_device_continuity
         WHERE tenant_id = $1 AND device_id = $2
         FOR UPDATE",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();

    let ack_pool = fixture.pool.clone();
    let ack_auth = fixture.auth.clone();
    let tenant_id = fixture.tenant_id;
    let ack_task = tokio::spawn(async move {
        sync::ack_continuity(
            &ack_pool,
            tenant_id,
            ack_auth,
            taskveil_sync::protocol::ContinuityAckRequest { proof },
        )
        .await
    });
    wait_for_lock_waiters(&fixture.admin_pool, 1).await;

    let complete_pool = fixture.pool.clone();
    let complete_auth = fixture.auth.clone();
    let token_keys = fixture.resync_tokens.clone();
    let complete_task = tokio::spawn(async move {
        sync::complete_base(
            &complete_pool,
            &token_keys,
            tenant_id,
            complete_auth,
            &completion_token,
        )
        .await
    });
    wait_for_lock_waiters(&fixture.admin_pool, 2).await;
    blocker.commit().await.unwrap();

    let (acknowledged, completion_replay) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            (ack_task.await.unwrap(), complete_task.await.unwrap())
        })
        .await
        .expect("ACK and completion replay must serialize without a deadlock");
    acknowledged.unwrap();
    assert!(
        completion_replay.is_err(),
        "the serialized stale completion replay must observe the ACK-deleted session"
    );
}

#[tokio::test]
async fn long_running_sync_and_resync_restarts_keep_continuity_rows_bounded() {
    let fixture = Fixture::setup().await;
    fixture.close_continuity().await;
    let mut since: i64 = query(
        "SELECT continuity_seq FROM tenant_device_continuity
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("continuity_seq")
    .unwrap();
    let mut previous_proof_id = None;

    for counter in 0_u32..64 {
        let result = sync::push(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            PushRequest {
                ops: vec![live_op(
                    Uuid::now_v7(),
                    None,
                    fixture.revision_hlc(-2_000, counter),
                    hlc(-2_100, counter, "bounded-retention"),
                    b"bounded-retention",
                )],
            },
        )
        .await
        .unwrap()
        .results
        .pop()
        .unwrap();
        assert_eq!(result.status, PushStatus::Accepted);

        let page = sync::pull(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            since,
            Some(100),
            None,
        )
        .await
        .unwrap();
        assert!(!page.has_more);
        assert!(page.next_since > since);
        let proof = page.closure_proof.clone().unwrap();
        if let Some(previous) = previous_proof_id {
            assert_ne!(
                proof.proof_id, previous,
                "a new high-water replaces the current proof"
            );
        }
        let replayed_page = sync::pull(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            since,
            Some(100),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            replayed_page.closure_proof.unwrap().proof_id,
            proof.proof_id,
            "the same closure reuses its current proof"
        );
        let first_ack = sync::ack_continuity(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            taskveil_sync::protocol::ContinuityAckRequest {
                proof: proof.clone(),
            },
        )
        .await
        .unwrap();
        let retried_ack = sync::ack_continuity(
            &fixture.pool,
            fixture.tenant_id,
            fixture.auth.clone(),
            taskveil_sync::protocol::ContinuityAckRequest {
                proof: proof.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(retried_ack, first_ack);
        since = page.next_since;
        previous_proof_id = Some(proof.proof_id);
    }

    let proof_count: i64 = query(
        "SELECT count(*) AS count FROM continuity_closure_proofs
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(
        proof_count, 1,
        "normal sync frequency must not increase proof retention"
    );

    let mut latest_generation = 0;
    for _ in 0..64 {
        latest_generation = sync::begin_full_resync(
            &fixture.pool,
            &fixture.resync_tokens,
            fixture.tenant_id,
            fixture.auth.clone(),
        )
        .await
        .unwrap()
        .generation;
    }
    let session = query(
        "SELECT count(*) AS count, max(generation) AS generation
         FROM device_resync_sessions
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert_eq!(session.try_get::<i64, _>("count").unwrap(), 1);
    assert_eq!(
        session.try_get::<Option<i64>, _>("generation").unwrap(),
        Some(latest_generation)
    );
}

#[tokio::test]
async fn v2_route_rejects_v1_unknown_collection_invalid_blob_and_collection_changes() {
    let fixture = Fixture::setup().await;
    fixture.close_continuity().await;
    let record_id = Uuid::now_v7();
    let revision = fixture.revision_hlc(-1_000, 0);
    let mutation = hlc(-1_100, 0, "route-mutation");
    let valid_body = json!({
        "ops": [{
            "op_id": Uuid::now_v7(),
            "record_id": record_id,
            "collection": "tasks",
            "base_revision_hlc": null,
            "revision_hlc": revision,
            "state": {
                "kind": "live",
                "mutation_hlc": mutation,
                "blob": STANDARD.encode(structural_envelope(b"cipher"))
            }
        }]
    });
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v1/tenants/{}/push", fixture.tenant_id),
            Some(&fixture.token),
            Some(valid_body.clone()),
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/push", fixture.tenant_id),
            None,
            Some(valid_body.clone()),
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    let old_protocol_response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v2/tenants/{}/push", fixture.tenant_id))
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&valid_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old_protocol_response.status(), StatusCode::CONFLICT);
    let removed_list_key_response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v2/tenants/{}/list-keys", fixture.tenant_id))
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed_list_key_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/push", fixture.tenant_id),
            Some(&fixture.token),
            Some(valid_body),
        )
        .await,
        StatusCode::OK
    );
    fixture.close_continuity().await;

    let unknown_collection = json!({
        "ops": [{
            "op_id": Uuid::now_v7(),
            "record_id": Uuid::now_v7(),
            "collection": "reminders",
            "base_revision_hlc": null,
            "revision_hlc": hlc(-900, 0, "unknown"),
            "state": {
                "kind": "tombstone",
                "delete_hlc": hlc(-950, 0, "unknown-delete")
            }
        }]
    });
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/push", fixture.tenant_id),
            Some(&fixture.token),
            Some(unknown_collection),
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_blob = json!({
        "ops": [{
            "op_id": Uuid::now_v7(),
            "record_id": Uuid::now_v7(),
            "collection": "tasks",
            "base_revision_hlc": null,
            "revision_hlc": hlc(-800, 0, "invalid-blob"),
            "state": {
                "kind": "live",
                "mutation_hlc": hlc(-850, 0, "invalid-blob-mutation"),
                "blob": "***"
            }
        }]
    });
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/push", fixture.tenant_id),
            Some(&fixture.token),
            Some(invalid_blob),
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let invalid_clock = json!({
        "ops": [{
            "op_id": Uuid::now_v7(),
            "record_id": Uuid::now_v7(),
            "collection": "tasks",
            "base_revision_hlc": null,
            "revision_hlc": "not-an-hlc",
            "state": {
                "kind": "tombstone",
                "delete_hlc": hlc(-700, 0, "invalid-clock-delete")
            }
        }]
    });
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/push", fixture.tenant_id),
            Some(&fixture.token),
            Some(invalid_clock),
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let changed_collection = PushOp {
        op_id: Uuid::now_v7(),
        record_id,
        collection: SyncCollection::Lists,
        base_revision_hlc: Some(revision),
        revision_hlc: fixture.revision_hlc(-700, 0),
        state: SyncRecordState::Tombstone {
            delete_hlc: hlc(-750, 0, "changed-delete"),
        },
    };
    assert!(sync::push(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        PushRequest {
            ops: vec![changed_collection]
        },
    )
    .await
    .is_err());
}

#[tokio::test]
async fn v2_schema_enforces_tagged_state_and_gc_only_removes_tombstones() {
    let fixture = Fixture::setup().await;
    let invalid = query(
        "INSERT INTO sync_records
         (tenant_id, record_id, collection, seq, revision_hlc, mutation_hlc,
          delete_hlc, encrypted_blob)
         VALUES ($1, $2, 'tasks', 1, 'revision', NULL, 'delete', $3)",
    )
    .bind(fixture.tenant_id)
    .bind(Uuid::now_v7())
    .bind(vec![1_u8])
    .execute(&fixture.admin_pool)
    .await;
    assert!(invalid.is_err());

    let live_id = Uuid::now_v7();
    let tombstone_id = Uuid::now_v7();
    fixture
        .push(live_op(
            live_id,
            None,
            fixture.revision_hlc(-1_000, 0),
            hlc(-1_100, 0, "gc-live-mutation"),
            b"live",
        ))
        .await;
    fixture
        .push(tombstone_op(
            tombstone_id,
            None,
            fixture.revision_hlc(-900, 0),
            hlc(-950, 0, "gc-delete"),
        ))
        .await;
    db::run_migrations(&fixture.admin_pool).await.unwrap();
    let preserved_after_rerun: i64 =
        query("SELECT count(*) AS count FROM sync_records WHERE tenant_id = $1")
            .bind(fixture.tenant_id)
            .fetch_one(&fixture.admin_pool)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(preserved_after_rerun, 2);
    query("UPDATE sync_records SET updated_at = $1 WHERE tenant_id = $2")
        .bind(Utc::now() - Duration::days(181))
        .bind(fixture.tenant_id)
        .execute(&fixture.admin_pool)
        .await
        .unwrap();

    assert_eq!(
        gc_tombstones(&fixture.admin_pool, Utc::now())
            .await
            .unwrap(),
        1
    );
    let remaining: Vec<Uuid> =
        query("SELECT record_id FROM sync_records WHERE tenant_id = $1 ORDER BY record_id")
            .bind(fixture.tenant_id)
            .fetch_all(&fixture.admin_pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get("record_id").unwrap())
            .collect();
    assert_eq!(remaining, vec![live_id]);

    let preflight = sync::preflight(&fixture.pool, fixture.tenant_id, fixture.auth.clone(), 0)
        .await
        .unwrap();
    assert_eq!(preflight.gc_horizon_seq, 2);
    assert!(preflight.full_resync_required);
    assert_eq!(
        request_status(
            &fixture.app,
            Method::GET,
            format!("/v2/tenants/{}/preflight?since=0", fixture.tenant_id),
            Some(&fixture.token),
            None,
        )
        .await,
        StatusCode::GONE
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::GET,
            format!("/v2/tenants/{}/preflight?since=1", fixture.tenant_id),
            Some(&fixture.token),
            None,
        )
        .await,
        StatusCode::GONE
    );
}

#[tokio::test]
async fn fuzzy_base_uses_stable_keys_and_delta_recovers_behind_cursor_changes() {
    let fixture = Fixture::setup().await;
    let behind_cursor = Uuid::from_u128(5);
    let first = Uuid::from_u128(10);
    let last = Uuid::from_u128(30);
    let first_revision = fixture.revision_hlc(-5_000, 0);
    fixture
        .push(live_op(
            first,
            None,
            first_revision.clone(),
            hlc(-5_100, 0, "stable-first-mutation"),
            b"first-v1",
        ))
        .await;
    fixture
        .push(live_op(
            last,
            None,
            fixture.revision_hlc(-4_900, 0),
            hlc(-5_000, 0, "stable-last-mutation"),
            b"last",
        ))
        .await;

    let start = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert_eq!(start.base_seq, 2);
    let first_page = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(1),
    )
    .await
    .unwrap();
    assert_eq!(first_page.records[0].record_id, first);
    assert!(first_page.has_more);

    fixture
        .push(live_op(
            first,
            Some(first_revision),
            fixture.revision_hlc(-4_000, 0),
            hlc(-4_100, 0, "stable-first-update-mutation"),
            b"first-v2",
        ))
        .await;
    fixture
        .push(live_op(
            behind_cursor,
            None,
            fixture.revision_hlc(-3_900, 0),
            hlc(-4_000, 0, "stable-behind-mutation"),
            b"behind",
        ))
        .await;

    let second_page = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        first_page.next_page_token.as_deref().unwrap(),
        Some(1),
    )
    .await
    .unwrap();
    assert_eq!(second_page.records[0].record_id, last);
    assert!(!second_page.has_more);
    sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        second_page.completion_token.as_deref().unwrap(),
    )
    .await
    .unwrap();

    let delta = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(1),
        Some(start.generation),
    )
    .await
    .unwrap();
    assert_eq!(delta.records.len(), 1);
    assert_eq!(delta.records[0].record_id, first);
    assert!(delta.has_more);
    assert_eq!(delta.high_water, 4);
    assert_eq!(delta.next_since, 3);

    let closure = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        delta.next_since,
        Some(1),
        Some(start.generation),
    )
    .await
    .unwrap();
    assert_eq!(closure.records[0].record_id, behind_cursor);
    assert!(!closure.has_more);
    assert_eq!(closure.next_since, closure.high_water);
    assert!(taskveil_sync::delta_reached_closure(
        closure.next_since,
        closure.has_more,
        closure.high_water
    ));
}

#[tokio::test]
async fn empty_resync_closes_and_base_scan_is_not_limited_to_start_seq() {
    let fixture = Fixture::setup().await;
    let start = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert_eq!(start.base_seq, 0);
    let empty_base = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(100),
    )
    .await
    .unwrap();
    assert!(empty_base.records.is_empty());
    assert!(!empty_base.has_more);
    sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        empty_base.completion_token.as_deref().unwrap(),
    )
    .await
    .unwrap();
    let empty_delta = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(100),
        Some(start.generation),
    )
    .await
    .unwrap();
    assert!(empty_delta.records.is_empty());
    assert!(taskveil_sync::delta_reached_closure(
        empty_delta.next_since,
        empty_delta.has_more,
        empty_delta.high_water
    ));

    let created_after_start = Uuid::from_u128(42);
    fixture
        .push(live_op(
            created_after_start,
            None,
            fixture.revision_hlc(-2_000, 0),
            hlc(-2_100, 0, "after-start-mutation"),
            b"created-after-start",
        ))
        .await;
    let restarted = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    let fuzzy_base = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &restarted.page_token,
        Some(100),
    )
    .await
    .unwrap();
    assert_eq!(fuzzy_base.records.len(), 1);
    assert_eq!(fuzzy_base.records[0].record_id, created_after_start);
    assert!(fuzzy_base.records[0].seq > start.base_seq);
}

#[tokio::test]
async fn resync_page_tokens_are_replayable_bound_and_require_idempotent_completion_ack() {
    use taskveil_server::resync_token::ResyncTokenClaims;

    let fixture = Fixture::setup().await;
    let start = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    let base_path = format!("/v2/tenants/{}/resync/base", fixture.tenant_id);
    assert_eq!(
        request_status(
            &fixture.app,
            Method::GET,
            format!("{base_path}?page_token={}", start.page_token),
            Some(&fixture.token),
            None,
        )
        .await,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            base_path.clone(),
            Some(&fixture.token),
            Some(json!({
                "page_token": start.page_token,
                "limit": 100,
                "cursor": {"collection": "tasks", "record_id": Uuid::now_v7()}
            })),
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            base_path.clone(),
            Some(&fixture.token),
            Some(json!({"page_token": "x".repeat(9 * 1024), "limit": 100})),
        )
        .await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            format!("/v2/tenants/{}/resync/base/complete", fixture.tenant_id),
            Some(&fixture.token),
            Some(json!({"completion_token": "x".repeat(9 * 1024)})),
        )
        .await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request_status(
            &fixture.app,
            Method::POST,
            base_path,
            Some(&fixture.token),
            Some(json!({"page_token": start.page_token, "limit": 100})),
        )
        .await,
        StatusCode::OK
    );
    let first = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(100),
    )
    .await
    .unwrap();
    let replay = sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &start.page_token,
        Some(100),
    )
    .await
    .unwrap();
    assert_eq!(replay, first);
    assert!(sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(100),
        Some(start.generation),
    )
    .await
    .is_err());

    let completion = first.completion_token.as_deref().unwrap();
    let first_ack = sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        completion,
    )
    .await
    .unwrap();
    let replayed_ack = sync::complete_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        completion,
    )
    .await
    .unwrap();
    assert_eq!(replayed_ack, first_ack);
    let continuity = query(
        "SELECT continuity_generation, required_generation
         FROM tenant_device_continuity
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert_ne!(
        continuity
            .try_get::<i64, _>("continuity_generation")
            .unwrap(),
        continuity.try_get::<i64, _>("required_generation").unwrap(),
        "base completion ACK must not close the independent continuity proof"
    );
    assert!(sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        start.base_seq,
        Some(100),
        Some(start.generation),
    )
    .await
    .is_ok());

    let stale = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    let current = sync::begin_full_resync(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert!(sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &stale.page_token,
        Some(100),
    )
    .await
    .is_err());
    assert!(sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &format!("{}x", current.page_token),
        Some(100),
    )
    .await
    .is_err());

    let token_now = Utc::now().timestamp();
    let cross_device_claims = ResyncTokenClaims::page(
        fixture.tenant_id,
        Uuid::now_v7(),
        current.generation,
        current.base_seq,
        None,
        token_now,
    )
    .unwrap();
    let cross_device = fixture
        .resync_tokens
        .sign(&cross_device_claims, token_now)
        .unwrap();
    assert!(sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &cross_device,
        Some(100),
    )
    .await
    .is_err());

    let cross_tenant_claims = ResyncTokenClaims::page(
        Uuid::now_v7(),
        fixture.auth.device_id,
        current.generation,
        current.base_seq,
        None,
        token_now,
    )
    .unwrap();
    let cross_tenant = fixture
        .resync_tokens
        .sign(&cross_tenant_claims, token_now)
        .unwrap();
    assert!(sync::scan_base(
        &fixture.pool,
        &fixture.resync_tokens,
        fixture.tenant_id,
        fixture.auth.clone(),
        &cross_tenant,
        Some(100),
    )
    .await
    .is_err());

    let restarted_keyring = taskveil_server::resync_token::ResyncTokenKeyring::for_tests();
    let after_restart = sync::scan_base(
        &fixture.pool,
        &restarted_keyring,
        fixture.tenant_id,
        fixture.auth.clone(),
        &current.page_token,
        Some(100),
    )
    .await
    .unwrap();
    let replayed_after_response_loss = sync::scan_base(
        &fixture.pool,
        &restarted_keyring,
        fixture.tenant_id,
        fixture.auth.clone(),
        &current.page_token,
        Some(100),
    )
    .await
    .unwrap();
    assert_eq!(after_restart, replayed_after_response_loss);

    let expired_issued_at = token_now - taskveil_server::resync_token::MAX_TOKEN_LIFETIME_SECS - 1;
    let expired_claims = ResyncTokenClaims::page(
        fixture.tenant_id,
        fixture.auth.device_id,
        current.generation,
        current.base_seq,
        None,
        expired_issued_at,
    )
    .unwrap();
    let expired = fixture
        .resync_tokens
        .sign(&expired_claims, expired_issued_at)
        .unwrap();
    assert!(sync::scan_base(
        &fixture.pool,
        &restarted_keyring,
        fixture.tenant_id,
        fixture.auth.clone(),
        &expired,
        Some(100),
    )
    .await
    .is_err());
    let replacement = sync::begin_full_resync(
        &fixture.pool,
        &restarted_keyring,
        fixture.tenant_id,
        fixture.auth.clone(),
    )
    .await
    .unwrap();
    assert!(replacement.generation > current.generation);
    let session_count: i64 = query(
        "SELECT count(*) AS count FROM device_resync_sessions
         WHERE tenant_id = $1 AND device_id = $2",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.auth.device_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(
        session_count, 1,
        "an expired chain restarts by replacing the device's only session row"
    );
}

#[tokio::test]
async fn gc_horizon_can_exceed_max_active_seq_and_empty_delta_reaches_high_water() {
    let fixture = Fixture::setup().await;
    let live_id = Uuid::from_u128(100);
    let tombstone_id = Uuid::from_u128(200);
    fixture
        .push(live_op(
            live_id,
            None,
            fixture.revision_hlc(-5_000, 0),
            hlc(-5_100, 0, "horizon-live-mutation"),
            b"live",
        ))
        .await;
    fixture
        .push(tombstone_op(
            tombstone_id,
            None,
            fixture.revision_hlc(-4_000, 0),
            hlc(-4_100, 0, "horizon-delete-semantic"),
        ))
        .await;
    query(
        "UPDATE sync_records SET updated_at = $1
         WHERE tenant_id = $2 AND record_id = $3",
    )
    .bind(Utc::now() - Duration::days(181))
    .bind(fixture.tenant_id)
    .bind(tombstone_id)
    .execute(&fixture.admin_pool)
    .await
    .unwrap();
    assert_eq!(
        gc_tombstones(&fixture.admin_pool, Utc::now())
            .await
            .unwrap(),
        1
    );

    let max_active: i64 = query(
        "SELECT coalesce(max(seq), 0)::BIGINT AS max_seq
         FROM sync_records WHERE tenant_id = $1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap()
    .try_get("max_seq")
    .unwrap();
    let preflight = sync::preflight(&fixture.pool, fixture.tenant_id, fixture.auth.clone(), 0)
        .await
        .unwrap();
    assert_eq!(max_active, 1);
    assert_eq!(preflight.gc_horizon_seq, 2);

    let page = sync::pull(
        &fixture.pool,
        fixture.tenant_id,
        fixture.auth.clone(),
        1,
        Some(100),
        None,
    )
    .await
    .unwrap();
    assert!(page.records.is_empty());
    assert_eq!(page.next_since, 2);
    assert_eq!(page.high_water, 2);
    assert!(!page.has_more);
    assert!(taskveil_sync::delta_reached_closure(
        page.next_since,
        page.has_more,
        page.high_water
    ));
}

async fn request_status(
    app: &Router,
    method: Method,
    uri: String,
    token: Option<&str>,
    body: Option<Value>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header(
        taskveil_protocol::sync::SYNC_PROTOCOL_VERSION_HEADER,
        taskveil_protocol::sync::SYNC_PROTOCOL_VERSION.to_string(),
    );
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let body = body
        .map(|value| Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap_or_else(Body::empty);
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    status
}
