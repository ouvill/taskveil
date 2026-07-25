pub mod auth;
pub mod billing;
pub mod config;
pub mod db;
pub mod organization;
pub mod realtime;
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
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl AppError {
    pub fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            bearer_challenge: false,
        }
    }

    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized",
            bearer_challenge: false,
        }
    }

    pub fn invalid_bearer_token() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized",
            bearer_challenge: true,
        }
    }

    pub fn invalid_grant() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: "invalid_grant",
            bearer_challenge: false,
        }
    }

    pub fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: "forbidden",
            bearer_challenge: false,
        }
    }

    pub fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
            bearer_challenge: false,
        }
    }

    pub fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message,
            bearer_challenge: false,
        }
    }

    pub fn gone(message: &'static str) -> Self {
        Self {
            status: StatusCode::GONE,
            message,
            bearer_challenge: false,
        }
    }

    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error",
            bearer_challenge: false,
        }
    }

    pub fn service_unavailable(message: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message,
            bearer_challenge: false,
        }
    }

    pub fn payment_required(message: &'static str) -> Self {
        Self {
            status: StatusCode::PAYMENT_REQUIRED,
            message,
            bearer_challenge: false,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let mut response = (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response();
        if self.bearer_challenge {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer error=\"invalid_token\""),
            );
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
}
