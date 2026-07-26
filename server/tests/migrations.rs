use std::{
    fs,
    path::{Path, PathBuf},
};

use sqlx::{migrate::MigrateError, AssertSqlSafe};
use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::PgPool;
use taskveil_server::db;
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};

struct Fixture {
    pool: PgPool,
    host: String,
    port: u16,
    _postgres: ContainerAsync<postgres::Postgres>,
}

impl Fixture {
    async fn start() -> Self {
        let postgres = postgres::Postgres::default().start().await.unwrap();
        let host = postgres.get_host().await.unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = db::connect(&database_url).await.unwrap();
        Self {
            pool,
            host: host.to_string(),
            port,
            _postgres: postgres,
        }
    }

    fn database_url(&self, user: &str, password: &str) -> String {
        format!(
            "postgres://{user}:{password}@{}:{}/postgres",
            self.host, self.port
        )
    }
}

fn migration_paths() -> Vec<PathBuf> {
    let mut paths = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn migration_version(path: &Path) -> i64 {
    path.file_stem()
        .unwrap()
        .to_string_lossy()
        .split_once('_')
        .unwrap()
        .0
        .parse()
        .unwrap()
}

fn legacy_migration_paths() -> Vec<PathBuf> {
    migration_paths()
        .into_iter()
        .filter(|path| migration_version(path) < 202607260001)
        .collect()
}

async fn constraint_oid(pool: &PgPool, constraint_name: &str) -> u32 {
    query("SELECT oid::bigint AS oid FROM pg_constraint WHERE conname = $1")
        .bind(constraint_name)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get::<i64, _>("oid")
        .unwrap()
        .try_into()
        .unwrap()
}

async fn assert_runtime_cannot_modify_migration_ledgers(fixture: &Fixture) {
    raw_sql(
        "CREATE ROLE taskveil_runtime_migration_test
             LOGIN PASSWORD 'taskveil-runtime-migration-test'
             NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS;
         GRANT taskveil_app TO taskveil_runtime_migration_test;",
    )
    .execute(&fixture.pool)
    .await
    .unwrap();
    let runtime_pool = db::connect_application(&fixture.database_url(
        "taskveil_runtime_migration_test",
        "taskveil-runtime-migration-test",
    ))
    .await
    .unwrap();

    for table in ["_sqlx_migrations", "taskveil_schema_migrations"] {
        let row = query(
            "SELECT has_table_privilege(current_user, $1, 'SELECT') AS can_select,
                    has_table_privilege(current_user, $1, 'INSERT') AS can_insert,
                    has_table_privilege(current_user, $1, 'UPDATE') AS can_update,
                    has_table_privilege(current_user, $1, 'DELETE') AS can_delete",
        )
        .bind(table)
        .fetch_one(&runtime_pool)
        .await
        .unwrap();
        assert!(!row.try_get::<bool, _>("can_select").unwrap());
        assert!(!row.try_get::<bool, _>("can_insert").unwrap());
        assert!(!row.try_get::<bool, _>("can_update").unwrap());
        assert!(!row.try_get::<bool, _>("can_delete").unwrap());
    }

    runtime_pool.close().await;
}

async fn seed_representative_legacy_data(pool: &PgPool) {
    raw_sql(
        "INSERT INTO users
             (id, email, opaque_suite_id, opaque_record, account_root_public)
         VALUES
             ('00000000-0000-0000-0000-000000000001',
              'migration-seed@example.invalid',
              2,
              decode('01', 'hex'),
              decode('02', 'hex'));

         INSERT INTO devices (id, user_id, device_name)
         VALUES
             ('00000000-0000-0000-0000-000000000002',
              '00000000-0000-0000-0000-000000000001',
              'Migration seed device');

         INSERT INTO tenants (id, kind, owner_user_id)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              'personal',
              '00000000-0000-0000-0000-000000000001');

         INSERT INTO tenant_members (tenant_id, user_id, role)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000001',
              'owner');

         INSERT INTO tenant_seq (tenant_id, last_seq)
         VALUES ('00000000-0000-0000-0000-000000000003', 1);

         INSERT INTO user_key_generations
             (user_id, generation, suite_id, status, wrapper_revision,
              wrapped_mk_by_password, wrapped_mk_by_recovery,
              account_root_public, wrapped_account_root_private)
         VALUES
             ('00000000-0000-0000-0000-000000000001',
              1, 2, 'active', 1,
              decode('03', 'hex'), decode('04', 'hex'),
              decode('05', 'hex'), decode('06', 'hex'));

         INSERT INTO tenant_key_generations
             (tenant_id, generation, suite_id, status, minimum_write_generation,
              signed_manifest, wrapped_tenant_root_dek)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              1, 2, 'active', 1,
              decode(repeat('07', 107), 'hex'), decode('08', 'hex'));

         INSERT INTO key_recipients
             (tenant_id, generation, device_id, recipient_key_fingerprint, wrapped_dek)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              1,
              '00000000-0000-0000-0000-000000000002',
              decode(repeat('09', 32), 'hex'),
              decode('0a', 'hex'));

         INSERT INTO sync_records
             (tenant_id, record_id, collection, seq, revision_hlc, mutation_hlc,
              encrypted_blob, suite_id, key_generation)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000004',
              'tasks', 1, 'seed', 'seed', decode('0b', 'hex'), 2, 1);

         INSERT INTO billing_customers
             (user_id, provider_app_user_id)
         VALUES
             ('00000000-0000-0000-0000-000000000001',
              '00000000-0000-0000-0000-000000000005');

         INSERT INTO session_families
             (id, user_id, device_id, client_id, absolute_expires_at)
         VALUES
             ('00000000-0000-0000-0000-000000000006',
              '00000000-0000-0000-0000-000000000001',
              '00000000-0000-0000-0000-000000000002',
              'taskveil-native',
              now() + interval '1 day');

         INSERT INTO access_tokens
             (id, family_id, token_hash, expires_at)
         VALUES
             ('00000000-0000-0000-0000-000000000007',
              '00000000-0000-0000-0000-000000000006',
              decode(repeat('0c', 32), 'hex'),
              now() + interval '15 minutes');

         INSERT INTO refresh_tokens
             (id, family_id, generation, token_hash, expires_at)
         VALUES
             ('00000000-0000-0000-0000-000000000008',
              '00000000-0000-0000-0000-000000000006',
              1,
              decode(repeat('0d', 32), 'hex'),
              now() + interval '1 day');",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn representative_legacy_data_is_present(pool: &PgPool) -> bool {
    query(
        "SELECT
             EXISTS (
                 SELECT 1 FROM users
                 WHERE id = '00000000-0000-0000-0000-000000000001'
             )
             AND EXISTS (
                 SELECT 1 FROM user_key_generations
                 WHERE user_id = '00000000-0000-0000-0000-000000000001'
             )
             AND EXISTS (
                 SELECT 1 FROM tenant_key_generations
                 WHERE tenant_id = '00000000-0000-0000-0000-000000000003'
             )
             AND EXISTS (
                 SELECT 1 FROM key_recipients
                 WHERE tenant_id = '00000000-0000-0000-0000-000000000003'
             )
             AND EXISTS (
                 SELECT 1 FROM sync_records
                 WHERE record_id = '00000000-0000-0000-0000-000000000004'
                   AND collection = 'tasks'
             )
             AND EXISTS (
                 SELECT 1 FROM billing_customers
                 WHERE user_id = '00000000-0000-0000-0000-000000000001'
             )
             AND EXISTS (
                 SELECT 1 FROM session_families
                 WHERE id = '00000000-0000-0000-0000-000000000006'
             )
             AND EXISTS (
                 SELECT 1 FROM access_tokens
                 WHERE id = '00000000-0000-0000-0000-000000000007'
             )
             AND EXISTS (
                 SELECT 1 FROM refresh_tokens
                 WHERE id = '00000000-0000-0000-0000-000000000008'
             )
             AND EXISTS (
                 SELECT 1 FROM taskveil_schema_migrations
                 WHERE version = '202607240002_task_series_domain'
             ) AS present",
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .try_get("present")
    .unwrap()
}

#[tokio::test]
async fn migrator_records_versions_skips_applied_sql_and_rejects_checksum_changes() {
    let fixture = Fixture::start().await;
    let paths = migration_paths();
    let expected_versions = paths
        .iter()
        .map(|path| migration_version(path))
        .collect::<Vec<_>>();
    assert!(expected_versions.contains(&202607260005));

    db::run_migrations(&fixture.pool).await.unwrap();

    let applied_versions = query(
        "SELECT version
         FROM _sqlx_migrations
         WHERE success
         ORDER BY version",
    )
    .fetch_all(&fixture.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.try_get::<i64, _>("version").unwrap())
    .collect::<Vec<_>>();
    assert_eq!(applied_versions, expected_versions);

    let constraint_before = constraint_oid(&fixture.pool, "sync_records_collection_check").await;
    db::run_migrations(&fixture.pool).await.unwrap();
    let constraint_after = constraint_oid(&fixture.pool, "sync_records_collection_check").await;
    assert_eq!(constraint_after, constraint_before);

    assert_runtime_cannot_modify_migration_ledgers(&fixture).await;

    let corrupted_version = expected_versions[0];
    query(
        "UPDATE _sqlx_migrations
         SET checksum = decode(repeat('00', 48), 'hex')
         WHERE version = $1",
    )
    .bind(corrupted_version)
    .execute(&fixture.pool)
    .await
    .unwrap();

    let error = db::run_migrations(&fixture.pool).await.unwrap_err();
    assert!(matches!(
        error,
        MigrateError::VersionMismatch(version) if version == corrupted_version
    ));
}

#[tokio::test]
async fn migrator_bootstraps_ledger_for_database_created_by_legacy_runner() {
    let fixture = Fixture::start().await;
    let legacy_paths = legacy_migration_paths();

    for path in &legacy_paths {
        let sql = fs::read_to_string(path).unwrap();
        raw_sql(AssertSqlSafe(sql))
            .execute(&fixture.pool)
            .await
            .unwrap();
    }

    let ledger_before: Option<String> =
        query("SELECT to_regclass('_sqlx_migrations')::text AS name")
            .fetch_one(&fixture.pool)
            .await
            .unwrap()
            .try_get("name")
            .unwrap();
    assert_eq!(ledger_before, None);

    seed_representative_legacy_data(&fixture.pool).await;
    assert!(representative_legacy_data_is_present(&fixture.pool).await);

    db::run_migrations(&fixture.pool).await.unwrap();

    let applied_count: i64 = query("SELECT count(*) AS count FROM _sqlx_migrations WHERE success")
        .fetch_one(&fixture.pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(
        applied_count,
        i64::try_from(migration_paths().len()).unwrap()
    );
    assert!(representative_legacy_data_is_present(&fixture.pool).await);
    assert_runtime_cannot_modify_migration_ledgers(&fixture).await;
}

#[tokio::test]
async fn continuity_retention_migration_compacts_existing_proofs_before_unique_index() {
    let fixture = Fixture::start().await;
    for path in legacy_migration_paths() {
        raw_sql(AssertSqlSafe(fs::read_to_string(path).unwrap()))
            .execute(&fixture.pool)
            .await
            .unwrap();
    }
    seed_representative_legacy_data(&fixture.pool).await;
    raw_sql(
        "INSERT INTO tenant_device_continuity
             (tenant_id, device_id, continuity_seq, continuity_generation,
              required_generation, initialized)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              10, 1, 1, true);

         INSERT INTO continuity_closure_proofs
             (proof_id, tenant_id, device_id, high_water, generation,
              acknowledged_at, created_at)
         VALUES
             ('00000000-0000-0000-0000-000000000011',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              8, 1, now() - interval '3 days', now() - interval '3 days'),
             ('00000000-0000-0000-0000-000000000012',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              10, 1, now() - interval '1 day', now() - interval '1 day'),
             ('00000000-0000-0000-0000-000000000013',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              11, 1, NULL, now() - interval '2 days');

         INSERT INTO devices (id, user_id, device_name)
         VALUES
             ('00000000-0000-0000-0000-000000000020',
              '00000000-0000-0000-0000-000000000001',
              'Migration seed acknowledged device');

         INSERT INTO tenant_device_continuity
             (tenant_id, device_id, continuity_seq, continuity_generation,
              required_generation, initialized)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000020',
              10, 1, 3, true);

         INSERT INTO continuity_closure_proofs
             (proof_id, tenant_id, device_id, high_water, generation,
              acknowledged_at, created_at)
         VALUES
             ('00000000-0000-0000-0000-000000000021',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000020',
              8, 1, now() - interval '3 days', now() - interval '3 days'),
             ('00000000-0000-0000-0000-000000000022',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000020',
              10, 1, now() - interval '1 day', now() - interval '1 day');

         INSERT INTO device_resync_sessions
             (tenant_id, device_id, generation, base_seq, base_complete,
              created_at, updated_at)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              1, 8, true, now() - interval '3 days', now() - interval '3 days'),
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              2, 10, false, now() - interval '1 day', now() - interval '1 day'),
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000020',
              1, 8, true, now() - interval '3 days', now() - interval '3 days'),
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000020',
              2, 10, false, now() - interval '1 day', now() - interval '1 day');",
    )
    .execute(&fixture.pool)
    .await
    .unwrap();

    let migration = migration_paths()
        .into_iter()
        .find(|path| migration_version(path) == 202607260004)
        .unwrap();
    raw_sql(AssertSqlSafe(fs::read_to_string(migration).unwrap()))
        .execute(&fixture.pool)
        .await
        .unwrap();

    let retained = query(
        "SELECT proof_id::text AS proof_id,
                acknowledged_at IS NULL AS unacknowledged
         FROM continuity_closure_proofs
         WHERE tenant_id = '00000000-0000-0000-0000-000000000003'
           AND device_id = '00000000-0000-0000-0000-000000000002'",
    )
    .fetch_all(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(
        retained[0].try_get::<String, _>("proof_id").unwrap(),
        "00000000-0000-0000-0000-000000000013"
    );
    assert!(retained[0].try_get::<bool, _>("unacknowledged").unwrap());

    let acknowledged = query(
        "SELECT proof_id::text AS proof_id
         FROM continuity_closure_proofs
         WHERE tenant_id = '00000000-0000-0000-0000-000000000003'
           AND device_id = '00000000-0000-0000-0000-000000000020'",
    )
    .fetch_all(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(acknowledged.len(), 1);
    assert_eq!(
        acknowledged[0].try_get::<String, _>("proof_id").unwrap(),
        "00000000-0000-0000-0000-000000000022"
    );

    let sessions = query(
        "SELECT device_id::text AS device_id, generation
         FROM device_resync_sessions
         WHERE tenant_id = '00000000-0000-0000-0000-000000000003'
         ORDER BY device_id",
    )
    .fetch_all(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(
        sessions[0].try_get::<String, _>("device_id").unwrap(),
        "00000000-0000-0000-0000-000000000002"
    );
    assert_eq!(
        sessions[0].try_get::<i64, _>("generation").unwrap(),
        1,
        "the required generation is retained even when an older schema has a newer row"
    );
    assert_eq!(
        sessions[1].try_get::<String, _>("device_id").unwrap(),
        "00000000-0000-0000-0000-000000000020"
    );
    assert_eq!(
        sessions[1].try_get::<i64, _>("generation").unwrap(),
        2,
        "the greatest generation is retained when required_generation has no session"
    );

    let duplicate = query(
        "INSERT INTO continuity_closure_proofs
             (proof_id, tenant_id, device_id, high_water, generation)
         VALUES
             ('00000000-0000-0000-0000-000000000014',
              '00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              12, 1)",
    )
    .execute(&fixture.pool)
    .await;
    assert!(
        duplicate.is_err(),
        "the migration must enforce one current proof per tenant/device"
    );

    let duplicate_session = query(
        "INSERT INTO device_resync_sessions
             (tenant_id, device_id, generation, base_seq)
         VALUES
             ('00000000-0000-0000-0000-000000000003',
              '00000000-0000-0000-0000-000000000002',
              3, 12)",
    )
    .execute(&fixture.pool)
    .await;
    assert!(
        duplicate_session.is_err(),
        "the migration must enforce one current resync session per tenant/device"
    );
}
