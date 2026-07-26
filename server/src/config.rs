use std::{collections::HashMap, env, future::Future};

use reqwest::{header::HeaderValue, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    billing::{BillingConfigurationError, BillingEnvironment, BillingService},
    realtime::{RealtimeConfigError, RealtimeGateway},
    resync_token::{ResyncTokenConfigError, ResyncTokenKeyring},
};

const DATABASE_URL: &str = "DATABASE_URL";
const BILLING_ENVIRONMENT: &str = "TASKVEIL_BILLING_ENVIRONMENT";
const AUTH_ISSUER: &str = "TASKVEIL_AUTH_ISSUER";
const RUNTIME_SECRET_ID: &str = "TASKVEIL_RUNTIME_SECRET_ID";
const EXTENSION_PORT: &str = "PARAMETERS_SECRETS_EXTENSION_HTTP_PORT";
const AWS_SESSION_TOKEN: &str = "AWS_SESSION_TOKEN";

pub struct RuntimeConfig {
    pub database_url: String,
    pub billing: BillingService,
    pub realtime: RealtimeGateway,
    pub auth_issuer: String,
    pub resync_tokens: ResyncTokenKeyring,
}

#[derive(Debug, Error)]
pub enum RuntimeConfigError {
    #[error("missing runtime configuration variable {0}")]
    Missing(&'static str),
    #[error("invalid billing configuration: {0}")]
    Billing(#[from] BillingConfigurationError),
    #[error("invalid realtime configuration: {0}")]
    Realtime(#[from] RealtimeConfigError),
    #[error("invalid resync token configuration: {0}")]
    ResyncToken(#[from] ResyncTokenConfigError),
    #[error("authorization server issuer is invalid")]
    InvalidAuthIssuer,
    #[error("runtime secret extension request failed")]
    ExtensionRequest,
    #[error("runtime secret payload is invalid")]
    SecretPayload,
}

#[derive(Deserialize)]
struct ExtensionSecretResponse {
    #[serde(rename = "SecretString")]
    secret_string: String,
}

impl RuntimeConfig {
    pub async fn load() -> Result<Self, RuntimeConfigError> {
        let environment = billing_environment()?;
        Self::load_from_sources(
            environment,
            env::var(RUNTIME_SECRET_ID).ok(),
            |secret_id| async move { fetch_secret_values(&secret_id).await },
            |name| env::var(name).ok(),
        )
        .await
    }

    async fn load_from_sources<F, Fut>(
        environment: BillingEnvironment,
        secret_id: Option<String>,
        fetch: F,
        local_lookup: impl Fn(&'static str) -> Option<String> + Copy,
    ) -> Result<Self, RuntimeConfigError>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<HashMap<String, String>, RuntimeConfigError>>,
    {
        if let Some(secret_id) = secret_id {
            let values = fetch(secret_id).await?;
            Self::from_values(environment, |name| {
                values.get(name).cloned().or_else(|| {
                    if name == AUTH_ISSUER {
                        local_lookup(name)
                    } else {
                        None
                    }
                })
            })
        } else {
            Self::from_values(environment, local_lookup)
        }
    }

    pub fn from_secret_json(
        environment: BillingEnvironment,
        secret_json: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let values: HashMap<String, String> =
            serde_json::from_str(secret_json).map_err(|_| RuntimeConfigError::SecretPayload)?;
        Self::from_values(environment, |name| values.get(name).cloned())
    }

    fn from_values(
        environment: BillingEnvironment,
        lookup: impl Fn(&'static str) -> Option<String> + Copy,
    ) -> Result<Self, RuntimeConfigError> {
        let database_url = lookup(DATABASE_URL).ok_or(RuntimeConfigError::Missing(DATABASE_URL))?;
        let auth_issuer = lookup(AUTH_ISSUER).ok_or(RuntimeConfigError::Missing(AUTH_ISSUER))?;
        validate_auth_issuer(&auth_issuer)?;
        let billing = BillingService::from_values(environment, lookup)?;
        let realtime = RealtimeGateway::from_string_values(lookup)?;
        let resync_tokens = ResyncTokenKeyring::from_string_values(lookup)?;
        Ok(Self {
            database_url,
            billing,
            realtime,
            auth_issuer,
            resync_tokens,
        })
    }
}

fn validate_auth_issuer(issuer: &str) -> Result<(), RuntimeConfigError> {
    let url = Url::parse(issuer).map_err(|_| RuntimeConfigError::InvalidAuthIssuer)?;
    let local_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if (url.scheme() != "https" && !local_http)
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(RuntimeConfigError::InvalidAuthIssuer);
    }
    Ok(())
}

fn billing_environment() -> Result<BillingEnvironment, RuntimeConfigError> {
    env::var(BILLING_ENVIRONMENT)
        .map_err(|_| RuntimeConfigError::Missing(BILLING_ENVIRONMENT))?
        .parse()
        .map_err(RuntimeConfigError::Billing)
}

async fn fetch_secret_values(
    secret_id: &str,
) -> Result<HashMap<String, String>, RuntimeConfigError> {
    let port = env::var(EXTENSION_PORT).unwrap_or_else(|_| "2773".to_string());
    let session_token =
        env::var(AWS_SESSION_TOKEN).map_err(|_| RuntimeConfigError::Missing(AWS_SESSION_TOKEN))?;
    let session_token =
        HeaderValue::from_str(&session_token).map_err(|_| RuntimeConfigError::ExtensionRequest)?;
    let mut url = Url::parse(&format!("http://127.0.0.1:{port}/secretsmanager/get"))
        .map_err(|_| RuntimeConfigError::ExtensionRequest)?;
    url.query_pairs_mut().append_pair("secretId", secret_id);
    let response = reqwest::Client::new()
        .get(url)
        .header("X-Aws-Parameters-Secrets-Token", session_token)
        .send()
        .await
        .map_err(|_| RuntimeConfigError::ExtensionRequest)?
        .error_for_status()
        .map_err(|_| RuntimeConfigError::ExtensionRequest)?
        .json::<ExtensionSecretResponse>()
        .await
        .map_err(|_| RuntimeConfigError::SecretPayload)?;
    serde_json::from_str(&response.secret_string).map_err(|_| RuntimeConfigError::SecretPayload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox_secret() -> &'static str {
        r#"{
            "DATABASE_URL":"postgres://runtime:redacted@example.invalid/taskveil",
            "TASKVEIL_AUTH_ISSUER":"https://api.staging.taskveil.example",
            "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT_ID":"resync-2026-07",
            "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "REVENUECAT_SANDBOX_PROJECT_ID":"sandbox-project",
            "REVENUECAT_SANDBOX_APP_ID":"sandbox-app",
            "REVENUECAT_SANDBOX_SECRET_KEY":"sandbox-secret",
            "REVENUECAT_SANDBOX_WEBHOOK_AUTHORIZATION":"sandbox-authorization",
            "REVENUECAT_SANDBOX_WEBHOOK_HMAC_SECRET":"sandbox-hmac"
        }"#
    }

    fn production_secret() -> &'static str {
        r#"{
            "DATABASE_URL":"postgres://runtime:redacted@example.invalid/taskveil",
            "TASKVEIL_AUTH_ISSUER":"https://api.taskveil.example",
            "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT_ID":"resync-2026-07",
            "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "REVENUECAT_PRODUCTION_PROJECT_ID":"production-project",
            "REVENUECAT_PRODUCTION_APP_ID":"production-app",
            "REVENUECAT_PRODUCTION_SECRET_KEY":"production-secret",
            "REVENUECAT_PRODUCTION_WEBHOOK_AUTHORIZATION":"production-authorization",
            "REVENUECAT_PRODUCTION_WEBHOOK_HMAC_SECRET":"production-hmac"
        }"#
    }

    #[test]
    fn sandbox_runtime_does_not_require_production_secrets() {
        let config = RuntimeConfig::from_secret_json(BillingEnvironment::Sandbox, sandbox_secret())
            .expect("sandbox config");
        assert_eq!(config.billing.environment(), BillingEnvironment::Sandbox);
        assert!(config.database_url.contains("runtime"));
    }

    #[test]
    fn production_runtime_does_not_require_sandbox_secrets() {
        let config =
            RuntimeConfig::from_secret_json(BillingEnvironment::Production, production_secret())
                .expect("production config");
        assert_eq!(config.billing.environment(), BillingEnvironment::Production);
        assert!(config.database_url.contains("runtime"));
    }

    #[tokio::test]
    async fn runtime_secret_id_selects_the_extension_source() {
        let values: HashMap<String, String> = serde_json::from_str(sandbox_secret()).unwrap();
        let config = RuntimeConfig::load_from_sources(
            BillingEnvironment::Sandbox,
            Some("taskveil-staging/runtime".to_string()),
            move |secret_id| async move {
                assert_eq!(secret_id, "taskveil-staging/runtime");
                Ok(values)
            },
            |_| panic!("local environment fallback must not run in Lambda mode"),
        )
        .await
        .expect("extension-backed config");
        assert_eq!(config.billing.environment(), BillingEnvironment::Sandbox);
    }

    #[tokio::test]
    async fn missing_runtime_secret_id_selects_local_environment_values() {
        let values: HashMap<String, String> = serde_json::from_str(production_secret()).unwrap();
        let config = RuntimeConfig::load_from_sources(
            BillingEnvironment::Production,
            None,
            |_| async { panic!("extension fetch must not run for local configuration") },
            |name| values.get(name).cloned(),
        )
        .await
        .expect("local config");
        assert_eq!(config.billing.environment(), BillingEnvironment::Production);
    }

    #[test]
    fn selected_environment_secret_is_required() {
        let error =
            RuntimeConfig::from_secret_json(BillingEnvironment::Production, sandbox_secret())
                .err()
                .expect("production config must reject sandbox-only values");
        assert!(error
            .to_string()
            .contains("REVENUECAT_PRODUCTION_PROJECT_ID"));
    }

    #[test]
    fn malformed_secret_does_not_echo_its_contents() {
        let secret = r#"{"DATABASE_URL":"do-not-log-me""#;
        let error = RuntimeConfig::from_secret_json(BillingEnvironment::Sandbox, secret)
            .err()
            .expect("malformed secret");
        assert_eq!(error.to_string(), "runtime secret payload is invalid");
        assert!(!error.to_string().contains("do-not-log-me"));
    }

    #[test]
    fn authorization_server_issuer_rejects_insecure_remote_and_non_root_urls() {
        assert!(matches!(
            validate_auth_issuer("http://api.taskveil.example"),
            Err(RuntimeConfigError::InvalidAuthIssuer)
        ));
        assert!(matches!(
            validate_auth_issuer("https://api.taskveil.example/auth"),
            Err(RuntimeConfigError::InvalidAuthIssuer)
        ));
        validate_auth_issuer("https://api.taskveil.example").expect("production issuer");
        validate_auth_issuer("http://127.0.0.1:3000").expect("local development issuer");
    }
}
