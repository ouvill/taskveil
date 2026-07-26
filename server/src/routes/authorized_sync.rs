use axum::{
    extract::{FromRequestParts, Path},
    http::{header, request::Parts, HeaderMap},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{auth::AuthContext, billing, AppError, SharedState};
use taskveil_protocol::sync::{SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION_HEADER};

/// The tenant and actor produced by the shared sync/realtime authorization policy.
///
/// Keeping the tenant beside the authenticated actor prevents handlers from
/// authorizing one tenant and then operating on another request value.
pub(super) struct AuthorizedSyncRequest {
    pub(super) tenant_id: Uuid,
    pub(super) auth_context: AuthContext,
}

#[derive(Deserialize)]
struct TenantPath {
    tenant_id: Uuid,
}

impl FromRequestParts<SharedState> for AuthorizedSyncRequest {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let Path(TenantPath { tenant_id }) = Path::<TenantPath>::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let token = bearer_token(&parts.headers).map_err(IntoResponse::into_response)?;

        // This order is security-sensitive. Authentication performs the
        // session/device checks and the tenant membership check in one
        // transaction, then billing checks the request-time entitlement.
        // Protocol compatibility is intentionally last so unauthenticated or
        // unentitled callers cannot probe the deployed protocol version.
        let auth_context =
            billing::authenticate_sync_request(&state.pool, &state.billing, token, tenant_id)
                .await
                .map_err(IntoResponse::into_response)?;
        require_current_protocol(&parts.headers).map_err(IntoResponse::into_response)?;

        Ok(Self {
            tenant_id,
            auth_context,
        })
    }
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(AppError::invalid_bearer_token)?
        .to_str()
        .map_err(|_| AppError::invalid_bearer_token())?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(AppError::invalid_bearer_token)?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || token.contains(' ') {
        return Err(AppError::invalid_bearer_token());
    }
    Ok(token)
}

fn require_current_protocol(headers: &HeaderMap) -> Result<(), AppError> {
    let version = headers
        .get(SYNC_PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok());
    if version != Some(SYNC_PROTOCOL_VERSION) {
        return Err(AppError::conflict("sync protocol upgrade required"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_scheme_is_case_insensitive_and_rejects_ambiguous_values() {
        for scheme in ["Bearer", "bearer", "BEARER"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("{scheme} test-token")).unwrap(),
            );
            assert_eq!(bearer_token(&headers).ok(), Some("test-token"));
        }

        for value in ["Basic test-token", "Bearer", "Bearer ", "Bearer one two"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            assert!(bearer_token(&headers).is_err());
        }
    }
}
