use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::SharedState;

pub mod auth;
mod authorized_sync;
pub mod billing;
pub mod realtime;
pub mod sync;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route(
            "/.well-known/oauth-authorization-server",
            get(auth::authorization_server_metadata),
        )
        .nest("/v1/auth", auth::router())
        .nest("/v1", billing::webhook_router())
        .nest(
            "/v2/tenants",
            sync::router()
                .merge(realtime::router())
                .merge(billing::tenant_router()),
        )
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn ready(State(state): State<SharedState>) -> (StatusCode, Json<Value>) {
    match sqlx::query_scalar!("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "unavailable" })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sqlx_postgres::PgPoolOptions;

    use super::*;
    use crate::{
        billing::{BillingEnvironment, BillingService},
        AppState,
    };

    #[tokio::test]
    async fn readiness_fails_closed_without_database_details() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://runtime:secret@127.0.0.1:1/taskveil")
            .unwrap();
        pool.close().await;
        let state = Arc::new(AppState {
            pool,
            billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: crate::resync_token::ResyncTokenKeyring::for_tests(),
        });
        let (status, Json(body)) = ready(State(state)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, json!({ "status": "unavailable" }));
        assert!(!body.to_string().contains("secret"));
    }
}
