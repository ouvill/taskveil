use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use super::authorized_sync::AuthorizedSyncRequest;
use crate::{
    sync::{
        self, ActivateRotationRequest, DeviceKeyExpiryRequest, DeviceKeyExpiryResponse,
        PrepareRotationRequest, RotationGenerationRequest, RotationStateResponse,
    },
    AppError, SharedState,
};
use taskveil_protocol::sync::{
    BaseScanResponse, ContinuityAckRequest, ContinuityAckResponse, PullResponse, PushRequest,
    PushResponse, PushStatus, ResyncStartResponse, StableRecordCursor, SyncCollection,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{tenant_id}/preflight", get(preflight))
        .route("/{tenant_id}/push", post(push))
        .route("/{tenant_id}/pull", get(pull))
        .route("/{tenant_id}/resync/start", post(begin_full_resync))
        .route("/{tenant_id}/resync/base", get(scan_base))
        .route("/{tenant_id}/continuity/ack", post(ack_continuity))
        .route("/{tenant_id}/key-rotation", get(rotation_state))
        .route("/{tenant_id}/key-rotation/bundle", get(active_key_bundle))
        .route("/{tenant_id}/key-rotation/prepare", post(prepare_rotation))
        .route(
            "/{tenant_id}/key-rotation/activate",
            post(activate_rotation),
        )
        .route("/{tenant_id}/key-rotation/ack", post(ack_key_generation))
        .route("/{tenant_id}/key-rotation/retire", post(retire_rotation))
        .route(
            "/{tenant_id}/devices/{device_id}/key-expiry",
            post(set_device_key_expiry),
        )
}

async fn set_device_key_expiry(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Path((_tenant_id, device_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<DeviceKeyExpiryRequest>,
) -> Result<Json<DeviceKeyExpiryResponse>, AppError> {
    sync::set_device_key_expiry(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        device_id,
        request,
    )
    .await
    .map(Json)
}

async fn prepare_rotation(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Json(request): Json<PrepareRotationRequest>,
) -> Result<Json<RotationStateResponse>, AppError> {
    sync::prepare_rotation(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        request,
    )
    .await
    .map(Json)
}

async fn activate_rotation(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Json(request): Json<ActivateRotationRequest>,
) -> Result<Json<RotationStateResponse>, AppError> {
    sync::activate_rotation(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        request,
    )
    .await
    .map(Json)
}

async fn ack_key_generation(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Json(request): Json<RotationGenerationRequest>,
) -> Result<Json<RotationStateResponse>, AppError> {
    sync::acknowledge_key_generation(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        request,
    )
    .await
    .map(Json)
}

async fn retire_rotation(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Json(request): Json<RotationGenerationRequest>,
) -> Result<Json<RotationStateResponse>, AppError> {
    sync::retire_rotation(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        request,
    )
    .await
    .map(Json)
}

async fn rotation_state(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
) -> Result<Json<RotationStateResponse>, AppError> {
    sync::rotation_state_for_tenant(&state.pool, authorized.tenant_id, authorized.auth_context)
        .await
        .map(Json)
}

async fn active_key_bundle(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
) -> Result<Json<taskveil_protocol::account::ActiveKeyBundleDto>, AppError> {
    sync::active_key_bundle(&state.pool, authorized.tenant_id, authorized.auth_context)
        .await
        .map(Json)
}

async fn preflight(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Query(query): Query<PreflightQuery>,
) -> Result<Response, AppError> {
    let capabilities = sync::preflight(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        query.since,
    )
    .await?;
    let status = if capabilities.full_resync_required {
        StatusCode::GONE
    } else {
        StatusCode::OK
    };
    Ok((status, Json(capabilities)).into_response())
}

#[derive(Debug, Deserialize)]
struct PreflightQuery {
    since: i64,
}

#[derive(Debug, Deserialize)]
struct PullQuery {
    since: i64,
    limit: Option<i64>,
    generation: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BaseScanQuery {
    generation: i64,
    after_collection: Option<SyncCollection>,
    after_record_id: Option<Uuid>,
    limit: Option<i64>,
}

async fn begin_full_resync(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
) -> Result<Json<ResyncStartResponse>, AppError> {
    sync::begin_full_resync(&state.pool, authorized.tenant_id, authorized.auth_context)
        .await
        .map(Json)
}

async fn scan_base(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Query(query): Query<BaseScanQuery>,
) -> Result<Json<BaseScanResponse>, AppError> {
    let cursor = match (query.after_collection, query.after_record_id) {
        (None, None) => None,
        (Some(collection), Some(record_id)) => Some(StableRecordCursor {
            collection,
            record_id,
        }),
        _ => return Err(AppError::bad_request("incomplete base cursor")),
    };
    sync::scan_base(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        query.generation,
        cursor,
        query.limit,
    )
    .await
    .map(Json)
}

async fn push(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Extension(realtime): Extension<crate::realtime::RealtimeGateway>,
    Json(request): Json<PushRequest>,
) -> Result<Json<PushResponse>, AppError> {
    let tenant_id = authorized.tenant_id;
    let device_id = authorized.auth_context.device_id;
    let response = sync::push(&state.pool, tenant_id, authorized.auth_context, request).await?;
    if should_publish(&response) {
        realtime.publish_change(tenant_id, device_id).await;
    }
    Ok(Json(response))
}

fn should_publish(response: &PushResponse) -> bool {
    response
        .results
        .iter()
        .any(|result| result.status == PushStatus::Accepted)
}

async fn pull(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResponse>, AppError> {
    sync::pull(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        query.since,
        query.limit,
        query.generation,
    )
    .await
    .map(Json)
}

async fn ack_continuity(
    State(state): State<SharedState>,
    authorized: AuthorizedSyncRequest,
    Json(request): Json<ContinuityAckRequest>,
) -> Result<Json<ContinuityAckResponse>, AppError> {
    sync::ack_continuity(
        &state.pool,
        authorized.tenant_id,
        authorized.auth_context,
        request,
    )
    .await
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskveil_protocol::sync::{PushResult, SyncCollection};

    #[test]
    fn publish_is_attempted_only_when_at_least_one_result_is_accepted() {
        for status in [
            PushStatus::NoOp,
            PushStatus::Conflict,
            PushStatus::Superseded,
        ] {
            assert!(!should_publish(&response_with(status)));
        }
        assert!(should_publish(&response_with(PushStatus::Accepted)));
        assert!(should_publish(&PushResponse {
            results: vec![result(PushStatus::NoOp), result(PushStatus::Accepted),],
        }));
    }

    fn response_with(status: PushStatus) -> PushResponse {
        PushResponse {
            results: vec![result(status)],
        }
    }

    fn result(status: PushStatus) -> PushResult {
        PushResult {
            op_id: Uuid::nil(),
            record_id: Uuid::nil(),
            collection: SyncCollection::Tasks,
            status,
            seq: None,
            current: None,
        }
    }
}
