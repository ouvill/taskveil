use sqlx_core::{query::query, raw_sql::raw_sql, row::Row};
use sqlx_postgres::{PgPool, Postgres};
use taskveil_server::db;
use testcontainers_modules::{
    postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use uuid::Uuid;

struct Fixture {
    admin_pool: PgPool,
    application_pool: PgPool,
    tenant_a: Uuid,
    tenant_b: Uuid,
    user_a: Uuid,
    second_user_a_tenant: Uuid,
    owner_database_url: String,
    bypass_database_url: String,
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
            "CREATE ROLE taskveil_runtime_test LOGIN PASSWORD 'taskveil-runtime-test'
             NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS",
        )
        .execute(&admin_pool)
        .await
        .unwrap();
        raw_sql("GRANT taskveil_app TO taskveil_runtime_test")
            .execute(&admin_pool)
            .await
            .unwrap();
        raw_sql(
            "CREATE ROLE taskveil_bypass_test LOGIN PASSWORD 'taskveil-bypass-test'
             NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT BYPASSRLS",
        )
        .execute(&admin_pool)
        .await
        .unwrap();
        raw_sql("GRANT taskveil_app TO taskveil_bypass_test")
            .execute(&admin_pool)
            .await
            .unwrap();

        let user_a = Uuid::now_v7();
        let user_b = Uuid::now_v7();
        let tenant_a = Uuid::now_v7();
        let tenant_b = Uuid::now_v7();
        for (user_id, email) in [
            (user_a, "rls-a@example.test"),
            (user_b, "rls-b@example.test"),
        ] {
            query::<Postgres>(
                "INSERT INTO users
                    (id, email, canonical_email, opaque_credential_id,
                     opaque_suite_id, opaque_record, account_root_public)
                 VALUES ($1, $2, $2, $1, 2, $3, '\\x00'::bytea)",
            )
            .bind(user_id)
            .bind(email)
            .bind(vec![1_u8])
            .execute(&admin_pool)
            .await
            .unwrap();
        }
        for (tenant_id, user_id, seq) in [(tenant_a, user_a, 1_i64), (tenant_b, user_b, 2)] {
            query::<Postgres>(
                "INSERT INTO tenants (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
            )
            .bind(tenant_id)
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
            query::<Postgres>(
                "INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(tenant_id)
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
            query::<Postgres>("INSERT INTO tenant_seq (tenant_id, last_seq) VALUES ($1, $2)")
                .bind(tenant_id)
                .bind(seq)
                .execute(&admin_pool)
                .await
                .unwrap();
            query::<Postgres>(
                "INSERT INTO tenant_key_generations
                    (tenant_id, suite_id, generation, status, minimum_write_generation,
                     signed_manifest, wrapped_tenant_root_dek)
                 VALUES ($1, 2, 1, 'active', 1, $2, $3)",
            )
            .bind(tenant_id)
            .bind(vec![0_u8; 124])
            .bind(vec![seq as u8])
            .execute(&admin_pool)
            .await
            .unwrap();
            let record_id = Uuid::now_v7();
            query::<Postgres>(
                "INSERT INTO sync_records
                 (tenant_id, record_id, collection, seq, revision_hlc, mutation_hlc,
                  encrypted_blob, suite_id, key_generation)
                 VALUES ($1, $2, 'lists', $3, 'z', 'a', $4, 2, 1)",
            )
            .bind(tenant_id)
            .bind(record_id)
            .bind(seq)
            .bind(vec![seq as u8])
            .execute(&admin_pool)
            .await
            .unwrap();
            query::<Postgres>(
                "INSERT INTO sync_records_history
                 (tenant_id, record_id, collection, seq, revision_hlc, mutation_hlc,
                  encrypted_blob, suite_id, key_generation, author_user_id)
                 VALUES ($1, $2, 'lists', $3, 'z', 'a', $4, 2, 1, $5)",
            )
            .bind(tenant_id)
            .bind(record_id)
            .bind(seq)
            .bind(vec![seq as u8])
            .bind(user_id)
            .execute(&admin_pool)
            .await
            .unwrap();
        }
        let second_user_a_tenant = Uuid::now_v7();
        query::<Postgres>(
            "INSERT INTO tenants (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
        )
        .bind(second_user_a_tenant)
        .bind(user_a)
        .execute(&admin_pool)
        .await
        .unwrap();
        query::<Postgres>(
            "INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(second_user_a_tenant)
        .bind(user_a)
        .execute(&admin_pool)
        .await
        .unwrap();

        let application_url = format!(
            "postgres://taskveil_runtime_test:taskveil-runtime-test@{host}:{port}/postgres"
        );
        let bypass_database_url =
            format!("postgres://taskveil_bypass_test:taskveil-bypass-test@{host}:{port}/postgres");
        let application_pool = db::connect_application(&application_url).await.unwrap();
        Self {
            admin_pool,
            application_pool,
            tenant_a,
            tenant_b,
            user_a,
            second_user_a_tenant,
            owner_database_url: database_url,
            bypass_database_url,
            _postgres: postgres,
        }
    }
}

#[tokio::test]
async fn application_role_and_rls_policies_fail_closed_and_isolate_tenants() {
    let fixture = Fixture::setup().await;

    let current_user: String = query::<Postgres>("SELECT current_user AS current_user")
        .fetch_one(&fixture.application_pool)
        .await
        .unwrap()
        .try_get("current_user")
        .unwrap();
    assert_eq!(current_user, "taskveil_runtime_test");

    assert!(db::connect_application(&fixture.owner_database_url)
        .await
        .is_err());
    assert!(db::connect_application(&fixture.bypass_database_url)
        .await
        .is_err());

    let group_role = query::<Postgres>(
        "SELECT rolcanlogin, rolsuper, rolbypassrls
         FROM pg_roles WHERE rolname = 'taskveil_app'",
    )
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert!(!group_role.try_get::<bool, _>("rolcanlogin").unwrap());
    assert!(!group_role.try_get::<bool, _>("rolsuper").unwrap());
    assert!(!group_role.try_get::<bool, _>("rolbypassrls").unwrap());

    let role = query::<Postgres>(
        "SELECT rolcanlogin, rolsuper, rolinherit, rolbypassrls,
                pg_has_role('taskveil_runtime_test', 'taskveil_app', 'USAGE') AS has_app_privileges
         FROM pg_roles WHERE rolname = 'taskveil_runtime_test'",
    )
    .fetch_one(&fixture.admin_pool)
    .await
    .unwrap();
    assert!(role.try_get::<bool, _>("rolcanlogin").unwrap());
    assert!(!role.try_get::<bool, _>("rolsuper").unwrap());
    assert!(role.try_get::<bool, _>("rolinherit").unwrap());
    assert!(!role.try_get::<bool, _>("rolbypassrls").unwrap());
    assert!(role.try_get::<bool, _>("has_app_privileges").unwrap());

    let protected_tables = query::<Postgres>(
        "SELECT relname, relrowsecurity, relforcerowsecurity
         FROM pg_class
         WHERE relnamespace = 'public'::regnamespace
           AND relname = ANY($1)
         ORDER BY relname",
    )
    .bind(vec![
        "tenants",
        "tenant_members",
        "tenant_seq",
        "user_key_generations",
        "tenant_key_generations",
        "key_recipients",
        "key_generation_acks",
        "sync_records",
        "sync_records_history",
        "tenant_device_continuity",
        "device_resync_sessions",
        "continuity_closure_proofs",
    ])
    .fetch_all(&fixture.admin_pool)
    .await
    .unwrap();
    assert_eq!(protected_tables.len(), 12);
    assert!(protected_tables.iter().all(|row| {
        row.try_get::<bool, _>("relrowsecurity").unwrap()
            && row.try_get::<bool, _>("relforcerowsecurity").unwrap()
    }));

    let no_context_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM sync_records")
        .fetch_one(&fixture.application_pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(no_context_count, 0);

    let mut user_only_tx = fixture.application_pool.begin().await.unwrap();
    db::set_user_context(&mut user_only_tx, fixture.user_a)
        .await
        .unwrap();
    let user_memberships: i64 = query::<Postgres>("SELECT count(*) AS count FROM tenant_members")
        .fetch_one(&mut *user_only_tx)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(user_memberships, 2);
    let user_only_role_update = query::<Postgres>(
        "UPDATE tenant_members SET role = 'admin' WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(fixture.second_user_a_tenant)
    .bind(fixture.user_a)
    .execute(&mut *user_only_tx)
    .await
    .unwrap();
    assert_eq!(user_only_role_update.rows_affected(), 0);
    let user_only_membership_insert = query::<Postgres>(
        "INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(fixture.tenant_b)
    .bind(fixture.user_a)
    .execute(&mut *user_only_tx)
    .await;
    assert!(user_only_membership_insert.is_err());
    user_only_tx.rollback().await.unwrap();

    let mut tx = db::begin_tenant_transaction(&fixture.application_pool, fixture.tenant_a)
        .await
        .unwrap();
    db::set_user_context(&mut tx, fixture.user_a).await.unwrap();
    for (table, sql) in [
        ("tenants", "SELECT count(*) AS count FROM tenants"),
        (
            "tenant_members",
            "SELECT count(*) AS count FROM tenant_members",
        ),
        ("tenant_seq", "SELECT count(*) AS count FROM tenant_seq"),
        (
            "tenant_key_generations",
            "SELECT count(*) AS count FROM tenant_key_generations",
        ),
        ("sync_records", "SELECT count(*) AS count FROM sync_records"),
        (
            "sync_records_history",
            "SELECT count(*) AS count FROM sync_records_history",
        ),
    ] {
        let count: i64 = query::<Postgres>(sql)
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
        assert_eq!(count, 1, "unexpected visible row count for {table}");
    }
    let wrong_update =
        query::<Postgres>("UPDATE tenant_seq SET last_seq = last_seq + 100 WHERE tenant_id = $1")
            .bind(fixture.tenant_b)
            .execute(&mut *tx)
            .await
            .unwrap();
    assert_eq!(wrong_update.rows_affected(), 0);
    let wrong_delete = query::<Postgres>("DELETE FROM tenant_key_generations WHERE tenant_id = $1")
        .bind(fixture.tenant_b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(wrong_delete.rows_affected(), 0);
    tx.commit().await.unwrap();

    let after_commit_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM sync_records")
        .fetch_one(&fixture.application_pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(after_commit_count, 0);

    let mut tx = db::begin_tenant_transaction(&fixture.application_pool, fixture.tenant_a)
        .await
        .unwrap();
    let cross_tenant_insert =
        query::<Postgres>("INSERT INTO tenant_seq (tenant_id, last_seq) VALUES ($1, 0)")
            .bind(Uuid::now_v7())
            .execute(&mut *tx)
            .await;
    assert!(cross_tenant_insert.is_err());
    tx.rollback().await.unwrap();

    let after_rollback_count: i64 = query::<Postgres>("SELECT count(*) AS count FROM sync_records")
        .fetch_one(&fixture.application_pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(after_rollback_count, 0);

    let tenant_b_seq: i64 =
        query::<Postgres>("SELECT last_seq FROM tenant_seq WHERE tenant_id = $1")
            .bind(fixture.tenant_b)
            .fetch_one(&fixture.admin_pool)
            .await
            .unwrap()
            .try_get("last_seq")
            .unwrap();
    assert_eq!(tenant_b_seq, 2);
}
