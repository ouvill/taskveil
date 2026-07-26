pub mod auth;
pub mod billing;
pub mod config;
pub mod db;
pub mod organization;
pub mod realtime;
pub mod resync_token;
pub mod routes;
pub mod sync;

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    Extension, Json, Router,
};
use realtime::RealtimeGateway;
use serde::Serialize;
use sqlx_postgres::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub billing: billing::BillingService,
    pub auth_issuer: String,
    pub resync_tokens: resync_token::ResyncTokenKeyring,
}

pub type SharedState = Arc<AppState>;

pub fn build_router(state: AppState) -> Router {
    build_router_with_realtime(state, RealtimeGateway::disabled())
}

pub fn build_router_with_realtime(state: AppState, realtime: RealtimeGateway) -> Router {
    routes::router()
        .layer(Extension(realtime))
        .with_state(Arc::new(state))
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: &'static str,
    bearer_challenge: bool,
    problem: Option<ProblemMetadata>,
}

#[derive(Debug)]
struct ProblemMetadata {
    problem_type: &'static str,
    code: &'static str,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    problem_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}

impl AppError {
    pub fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized",
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn invalid_bearer_token() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized",
            bearer_challenge: true,
            problem: None,
        }
    }

    pub fn invalid_grant() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: "invalid_grant",
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: "forbidden",
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn retryable_clock_skew() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: "hlc retry after server clock advances",
            bearer_challenge: false,
            problem: Some(ProblemMetadata {
                problem_type: taskveil_protocol::sync::SYNC_CLOCK_SKEW_RETRYABLE_TYPE,
                code: taskveil_protocol::sync::SYNC_CLOCK_SKEW_RETRYABLE_CODE,
            }),
        }
    }

    pub fn gone(message: &'static str) -> Self {
        Self {
            status: StatusCode::GONE,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error",
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn service_unavailable(message: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }

    pub fn payment_required(message: &'static str) -> Self {
        Self {
            status: StatusCode::PAYMENT_REQUIRED,
            message,
            bearer_challenge: false,
            problem: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let mut response = (
            self.status,
            Json(ErrorBody {
                error: self.message,
                problem_type: self.problem.as_ref().map(|problem| problem.problem_type),
                code: self.problem.as_ref().map(|problem| problem.code),
                title: self.problem.as_ref().map(|_| self.message),
                status: self.problem.as_ref().map(|_| self.status.as_u16()),
            }),
        )
            .into_response();
        if self.bearer_challenge {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer error=\"invalid_token\""),
            );
        }
        if self.problem.is_some() {
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

impl From<sqlx_core::Error> for AppError {
    fn from(error: sqlx_core::Error) -> Self {
        tracing::error!(kind = "sqlx", error = %error, "server database error");
        Self::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_challenge_is_limited_to_protected_resource_authentication() {
        let opaque_response = AppError::unauthorized().into_response();
        assert_eq!(opaque_response.status(), StatusCode::UNAUTHORIZED);
        assert!(opaque_response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .is_none());

        let bearer_response = AppError::invalid_bearer_token().into_response();
        assert_eq!(bearer_response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            bearer_response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer error=\"invalid_token\"")
        );
    }

    #[tokio::test]
    async fn retryable_clock_skew_uses_stable_problem_code_and_not_http_425() {
        let response = AppError::retryable_clock_skew().into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "1");
        let body = axum::body::to_bytes(response.into_body(), 4_096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "sync_clock_skew_retryable");
        assert_eq!(
            body["type"],
            "https://taskveil.com/problems/sync-clock-skew-retryable"
        );
        assert_eq!(body["status"], StatusCode::CONFLICT.as_u16());

        let generic = AppError::conflict("different conflict").into_response();
        let body = axum::body::to_bytes(generic.into_body(), 4_096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body.get("code").is_none());
        assert!(body.get("type").is_none());
    }
}
