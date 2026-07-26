use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Form, FromRequest, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use std::{net::SocketAddr, str::FromStr};

use crate::{
    auth::{
        self, AuthorizationServerMetadata, LoginFinishRequest, LoginSessionResponse,
        LoginStartResponse, LogoutResponse, OpaqueStartRequest, RegisterFinishRequest,
        RegistrationStartResponse, RevocationRequest, SessionResponse, TokenRequest, TokenResponse,
    },
    auth_protection::{
        AuthAdmission, ClientSource, AUTH_START_BODY_LIMIT, TRUSTED_SOURCE_IP_HEADER,
    },
    AppError, SharedState,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route(
            "/register/start",
            post(register_start).layer(DefaultBodyLimit::max(AUTH_START_BODY_LIMIT)),
        )
        .route("/register/finish", post(register_finish))
        .route(
            "/login/start",
            post(login_start).layer(DefaultBodyLimit::max(AUTH_START_BODY_LIMIT)),
        )
        .route("/login/finish", post(login_finish))
        .route("/device/certify", post(certify_device))
        .route("/token", post(refresh_session))
        .route("/revoke", post(revoke_token))
        .route("/key-wrappers", post(update_key_wrappers))
}

pub async fn authorization_server_metadata(
    State(state): State<SharedState>,
) -> Json<AuthorizationServerMetadata> {
    Json(auth::authorization_server_metadata(&state.auth_issuer))
}

async fn certify_device(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<taskveil_protocol::account::DeviceEnrollmentDto>,
) -> Result<Json<LogoutResponse>, AppError> {
    let token = bearer_token(&headers)?;
    auth::certify_device(&state.pool, token, request)
        .await
        .map(Json)
}

async fn update_key_wrappers(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<taskveil_protocol::account::UpdateKeyWrappersRequest>,
) -> Result<Json<LogoutResponse>, AppError> {
    let token = bearer_token(&headers)?;
    auth::update_key_wrappers(&state.pool, token, request)
        .await
        .map(Json)
}

async fn register_start(
    State(state): State<SharedState>,
    LimitedOpaqueStart { request, admission }: LimitedOpaqueStart,
) -> Result<Json<RegistrationStartResponse>, AppError> {
    auth::register_start(&state.pool, request, admission.identifier_key())
        .await
        .map(Json)
}

async fn register_finish(
    State(state): State<SharedState>,
    Json(request): Json<RegisterFinishRequest>,
) -> Result<(HeaderMap, Json<SessionResponse>), AppError> {
    auth::register_finish(&state.pool, request)
        .await
        .map(|response| (token_response_headers(), Json(response)))
}

async fn login_start(
    State(state): State<SharedState>,
    LimitedOpaqueStart { request, admission }: LimitedOpaqueStart,
) -> Result<Json<LoginStartResponse>, AppError> {
    auth::login_start(&state.pool, request, admission.identifier_key())
        .await
        .map(Json)
}

async fn login_finish(
    State(state): State<SharedState>,
    Json(request): Json<LoginFinishRequest>,
) -> Result<(HeaderMap, Json<LoginSessionResponse>), AppError> {
    auth::login_finish(&state.pool, request)
        .await
        .map(|response| (token_response_headers(), Json(response)))
}

async fn refresh_session(
    State(state): State<SharedState>,
    Form(request): Form<TokenRequest>,
) -> Result<(HeaderMap, Json<TokenResponse>), AppError> {
    auth::refresh_session(&state.pool, request)
        .await
        .map(|response| (token_response_headers(), Json(response)))
}

async fn revoke_token(
    State(state): State<SharedState>,
    Form(request): Form<RevocationRequest>,
) -> Result<(HeaderMap, StatusCode), AppError> {
    auth::revoke_token(&state.pool, request)
        .await
        .map(|_| (token_response_headers(), StatusCode::OK))
}

fn token_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
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

struct LimitedOpaqueStart {
    request: OpaqueStartRequest,
    admission: AuthAdmission,
}

impl FromRequest<SharedState> for LimitedOpaqueStart {
    type Rejection = Response;

    async fn from_request(request: Request, state: &SharedState) -> Result<Self, Self::Rejection> {
        let peer_address = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| *address);
        let source = client_source(
            request.headers(),
            peer_address,
            state.trust_source_ip_header,
        );
        state
            .auth_protection
            .admit_source(source)
            .map_err(|error| AppError::rate_limited(error.retry_after_seconds).into_response())?;

        let Json(request) = Json::<OpaqueStartRequest>::from_request(request, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let canonical_identifier =
            auth::normalize_email(&request.email).map_err(IntoResponse::into_response)?;
        let admission = state
            .auth_protection
            .admit_identifier(&canonical_identifier)
            .map_err(|error| AppError::rate_limited(error.retry_after_seconds).into_response())?;
        Ok(Self { request, admission })
    }
}

fn client_source(
    headers: &HeaderMap,
    peer_address: Option<SocketAddr>,
    trust_source_ip_header: bool,
) -> ClientSource {
    trust_source_ip_header
        .then(|| canonical_source_ip_header(headers))
        .flatten()
        .or_else(|| peer_address.map(|address| ClientSource::Ip(address.ip())))
        .unwrap_or(ClientSource::Unattributed)
}

fn canonical_source_ip_header(headers: &HeaderMap) -> Option<ClientSource> {
    let mut values = headers.get_all(TRUSTED_SOURCE_IP_HEADER).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let address = std::net::IpAddr::from_str(value).ok()?;
    (address.to_string() == value).then_some(ClientSource::Ip(address))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use sqlx_postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        auth_protection::AuthProtection,
        billing::{BillingEnvironment, BillingService},
        build_router, AppState,
    };

    #[test]
    fn trusted_source_header_overrides_the_transport_peer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TRUSTED_SOURCE_IP_HEADER,
            HeaderValue::from_static("203.0.113.42"),
        );
        let transport = SocketAddr::from(([127, 0, 0, 1], 4000));
        assert_eq!(
            client_source(&headers, Some(transport), true),
            ClientSource::Ip("203.0.113.42".parse().unwrap())
        );
    }

    #[test]
    fn malformed_trusted_source_header_falls_back_to_transport_peer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TRUSTED_SOURCE_IP_HEADER,
            HeaderValue::from_static("not-an-address"),
        );
        let transport = SocketAddr::from(([192, 0, 2, 7], 4000));
        assert_eq!(
            client_source(&headers, Some(transport), true),
            ClientSource::Ip("192.0.2.7".parse().unwrap())
        );
    }

    #[test]
    fn source_header_requires_explicit_trust_and_one_canonical_value() {
        let transport = SocketAddr::from(([192, 0, 2, 9], 4000));
        let mut headers = HeaderMap::new();
        headers.append(
            TRUSTED_SOURCE_IP_HEADER,
            HeaderValue::from_static("203.0.113.1"),
        );
        assert_eq!(
            client_source(&headers, Some(transport), false),
            ClientSource::Ip("192.0.2.9".parse().unwrap())
        );

        headers.append(
            TRUSTED_SOURCE_IP_HEADER,
            HeaderValue::from_static("203.0.113.2"),
        );
        assert_eq!(
            client_source(&headers, Some(transport), true),
            ClientSource::Ip("192.0.2.9".parse().unwrap())
        );

        let mut noncanonical = HeaderMap::new();
        noncanonical.insert(
            TRUSTED_SOURCE_IP_HEADER,
            HeaderValue::from_static("2001:0DB8::1"),
        );
        assert_eq!(
            client_source(&noncanonical, Some(transport), true),
            ClientSource::Ip("192.0.2.9".parse().unwrap())
        );
    }

    #[tokio::test]
    async fn oversized_streaming_start_body_is_rejected_before_json_and_database_work() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://runtime:secret@127.0.0.1:1/taskveil")
            .unwrap();
        pool.close().await;
        let app = build_router(AppState {
            pool,
            billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: crate::resync_token::ResyncTokenKeyring::for_tests(),
            auth_protection: AuthProtection::new([0xA7; 32]),
            trust_source_ip_header: false,
        });
        let request = Request::post("/v1/auth/login/start")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(vec![b' '; AUTH_START_BODY_LIMIT + 1]))
            .unwrap();
        assert!(request.headers().get(header::CONTENT_LENGTH).is_none());

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn malformed_json_is_source_limited_before_repeated_parsing() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://runtime:secret@127.0.0.1:1/taskveil")
            .unwrap();
        pool.close().await;
        let app = build_router(AppState {
            pool,
            billing: BillingService::unavailable_for_tests(BillingEnvironment::Sandbox),
            auth_issuer: "http://localhost".to_string(),
            resync_tokens: crate::resync_token::ResyncTokenKeyring::for_tests(),
            auth_protection: AuthProtection::new([0xA7; 32]),
            trust_source_ip_header: false,
        });

        for _ in 0..20 {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/v1/auth/login/start")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let limited = app
            .oneshot(
                Request::post("/v1/auth/login/start")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );
    }
}
