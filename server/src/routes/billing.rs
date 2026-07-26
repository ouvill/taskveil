use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use crate::{
    auth,
    billing::{self, BillingResponse},
    AppError, SharedState,
};

pub fn tenant_router() -> Router<SharedState> {
    Router::new()
        .route("/{tenant_id}/billing", get(get_billing))
        .route("/{tenant_id}/billing/refresh", post(refresh_billing))
}

pub fn webhook_router() -> Router<SharedState> {
    Router::new().route("/billing/webhooks/revenuecat", post(revenuecat_webhook))
}

async fn get_billing(
    State(state): State<SharedState>,
    Path(tenant_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<BillingResponse>, AppError> {
    let token = super::authorized_sync::bearer_token(&headers)?;
    let context = auth::authenticate(&state.pool, token, tenant_id).await?;
    billing::get_billing(
        &state.pool,
        state.billing.environment(),
        tenant_id,
        context.user_id,
    )
    .await
    .map(Json)
}

async fn refresh_billing(
    State(state): State<SharedState>,
    Path(tenant_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<BillingResponse>, AppError> {
    let token = super::authorized_sync::bearer_token(&headers)?;
    let context = auth::authenticate(&state.pool, token, tenant_id).await?;
    billing::refresh_billing(&state.pool, &state.billing, tenant_id, context.user_id)
        .await
        .map(Json)
}

async fn revenuecat_webhook(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let signature = headers
        .get("x-revenuecat-webhook-signature")
        .and_then(|value| value.to_str().ok());
    billing::process_revenuecat_webhook(
        &state.pool,
        &state.billing,
        authorization,
        signature,
        &body,
    )
    .await?;
    Ok(StatusCode::OK)
}
