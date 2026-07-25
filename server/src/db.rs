use sqlx_core::raw_sql::raw_sql;
use sqlx_postgres::{PgPool, PgPoolOptions, PgTransaction};
use uuid::Uuid;

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx_core::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
}

pub async fn connect_application(database_url: &str) -> Result<PgPool, sqlx_core::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    let protected_tables = [
        "tenants",
        "tenant_members",
        "tenant_seq",
        "tenant_key_generations",
        "key_recipients",
        "key_generation_acks",
        "sync_records",
        "sync_records_history",
        "tenant_device_continuity",
        "device_resync_sessions",
        "continuity_closure_proofs",
    ];
    let role = sqlx::query!(
        "SELECT r.rolcanlogin AS \"rolcanlogin!\",
                r.rolsuper AS \"rolsuper!\",
                r.rolinherit AS \"rolinherit!\",
                r.rolbypassrls AS \"rolbypassrls!\",
                pg_has_role(current_user, 'taskveil_app', 'USAGE')
                    AS \"has_app_privileges!\",
                EXISTS (
                    SELECT 1
                    FROM pg_class c
                    WHERE c.relnamespace = 'public'::regnamespace
                      AND c.relname = ANY($1)
                      AND c.relowner = r.oid
                ) AS \"owns_protected_table!\"
         FROM pg_roles r
         WHERE r.rolname = current_user",
        &protected_tables as &[&str],
    )
    .fetch_one(&pool)
    .await?;
    let is_safe = role.rolcanlogin
        && !role.rolsuper
        && role.rolinherit
        && !role.rolbypassrls
        && role.has_app_privileges
        && !role.owns_protected_table;
    if !is_safe {
        pool.close().await;
        return Err(sqlx_core::Error::InvalidArgument(
            "DATABASE_URL must use a non-owner LOGIN role that inherits taskveil_app and cannot bypass RLS"
                .to_string(),
        ));
    }
    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx_core::Error> {
    raw_sql(include_str!("../migrations/202607080001_sync_server.sql"))
        .execute(pool)
        .await?;
    raw_sql(include_str!(
        "../migrations/202607080002_account_key_bundles.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607100001_sync_protocol_v2.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!("../migrations/202607100002_fuzzy_resync.sql"))
        .execute(pool)
        .await?;
    raw_sql(include_str!("../migrations/202607110001_rls_hardening.sql"))
        .execute(pool)
        .await?;
    raw_sql(include_str!(
        "../migrations/202607110002_archive_first_deletion.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607130001_timer_sessions.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607160001_billing_foundation.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607170001_template_recurrence.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607240001_tenant_record_keys.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607240002_task_series_domain.sql"
    ))
    .execute(pool)
    .await?;
    raw_sql(include_str!(
        "../migrations/202607250001_standard_session_management.sql"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn begin_tenant_transaction(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<PgTransaction<'_>, sqlx_core::Error> {
    let mut tx = pool.begin().await?;
    set_tenant_context(&mut tx, tenant_id).await?;
    Ok(tx)
}

pub async fn set_tenant_context(
    tx: &mut PgTransaction<'_>,
    tenant_id: Uuid,
) -> Result<(), sqlx_core::Error> {
    let _ = sqlx::query_scalar!(
        "SELECT set_config('taskveil.tenant_id', $1, true)",
        tenant_id.to_string()
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}

pub async fn set_user_context(
    tx: &mut PgTransaction<'_>,
    user_id: Uuid,
) -> Result<(), sqlx_core::Error> {
    let _ = sqlx::query_scalar!(
        "SELECT set_config('taskveil.user_id', $1, true)",
        user_id.to_string()
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}
