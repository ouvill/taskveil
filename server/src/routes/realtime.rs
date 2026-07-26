use axum::{routing::post, Extension, Json, Router};

use super::authorized_sync::AuthorizedSyncRequest;
use crate::{
    realtime::{observe_realtime, RealtimeEvent, RealtimeGateway, RealtimeTicketResponse},
    AppError, SharedState,
};

pub fn router() -> Router<SharedState> {
    Router::new().route("/{tenant_id}/realtime/ticket", post(ticket))
}

async fn ticket(
    Extension(realtime): Extension<RealtimeGateway>,
    authorized: AuthorizedSyncRequest,
) -> Result<Json<RealtimeTicketResponse>, AppError> {
    let Some(response) =
        realtime.issue_ticket(authorized.tenant_id, authorized.auth_context.device_id)
    else {
        observe_realtime(RealtimeEvent::TicketUnavailable);
        return Err(AppError::service_unavailable("realtime unavailable"));
    };
    observe_realtime(RealtimeEvent::TicketIssued);
    Ok(Json(response))
}
