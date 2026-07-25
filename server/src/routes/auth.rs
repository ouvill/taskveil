use axum::{
    extract::{Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    routing::post,
    Json, Router,
};

use crate::{
    auth::{
        self, AuthorizationServerMetadata, LoginFinishRequest, LoginSessionResponse,
        LogoutResponse, OpaqueStartRequest, OpaqueStartResponse, RegisterFinishRequest,
        RevocationRequest, SessionResponse, TokenRequest, TokenResponse,
    },
    AppError, SharedState,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/register/start", post(register_start))
        .route("/register/finish", post(register_finish))
        .route("/login/start", post(login_start))
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
    Json(request): Json<taskveil_sync::account::DeviceEnrollmentDto>,
) -> Result<Json<LogoutResponse>, AppError> {
    let token = bearer_token(&headers)?;
    auth::certify_device(&state.pool, token, request)
        .await
        .map(Json)
}

async fn update_key_wrappers(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<taskveil_sync::account::UpdateKeyWrappersRequest>,
) -> Result<Json<LogoutResponse>, AppError> {
    let token = bearer_token(&headers)?;
    auth::update_key_wrappers(&state.pool, token, request)
        .await
        .map(Json)
}

async fn register_start(
    State(state): State<SharedState>,
    Json(request): Json<OpaqueStartRequest>,
) -> Result<Json<OpaqueStartResponse>, AppError> {
    auth::register_start(&state.pool, request).await.map(Json)
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
    Json(request): Json<OpaqueStartRequest>,
) -> Result<Json<OpaqueStartResponse>, AppError> {
    auth::login_start(&state.pool, request).await.map(Json)
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
