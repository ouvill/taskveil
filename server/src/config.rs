use std::{collections::HashMap, env, fmt, future::Future, num::NonZeroU64};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::{header::HeaderValue, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    billing::{BillingConfigurationError, BillingEnvironment, BillingService},
    email_verification::{EmailVerificationConfig, EmailVerificationService},
    realtime::{RealtimeConfigError, RealtimeGateway},
    resync_token::{ResyncTokenConfigError, ResyncTokenKeyring},
};

const DATABASE_URL: &str = "DATABASE_URL";
const BILLING_ENVIRONMENT: &str = "TASKVEIL_BILLING_ENVIRONMENT";
const AUTH_ISSUER: &str = "TASKVEIL_AUTH_ISSUER";
const AUTH_LIMIT_HMAC_KEY: &str = "TASKVEIL_AUTH_LIMIT_HMAC_KEY";
const AUTH_LIMIT_HMAC_KEY_GENERATION: &str = "TASKVEIL_AUTH_LIMIT_HMAC_KEY_GENERATION";
const TRUST_SOURCE_IP_HEADER: &str = "TASKVEIL_TRUST_SOURCE_IP_HEADER";
const RUNTIME_SECRET_ID: &str = "TASKVEIL_RUNTIME_SECRET_ID";
const EXTENSION_PORT: &str = "PARAMETERS_SECRETS_EXTENSION_HTTP_PORT";
const AWS_SESSION_TOKEN: &str = "AWS_SESSION_TOKEN";
const EMAIL_TOKEN_KEY_CURRENT_VERSION: &str = "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT_VERSION";
const EMAIL_TOKEN_KEY_CURRENT: &str = "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT";
const EMAIL_TOKEN_KEY_PREVIOUS_VERSION: &str = "TASKVEIL_EMAIL_TOKEN_KEY_PREVIOUS_VERSION";
const EMAIL_TOKEN_KEY_PREVIOUS: &str = "TASKVEIL_EMAIL_TOKEN_KEY_PREVIOUS";
const EMAIL_STATE_KEY_CURRENT_VERSION: &str = "TASKVEIL_EMAIL_STATE_KEY_CURRENT_VERSION";
const EMAIL_STATE_KEY_CURRENT: &str = "TASKVEIL_EMAIL_STATE_KEY_CURRENT";
const EMAIL_STATE_KEY_PREVIOUS_VERSION: &str = "TASKVEIL_EMAIL_STATE_KEY_PREVIOUS_VERSION";
const EMAIL_STATE_KEY_PREVIOUS: &str = "TASKVEIL_EMAIL_STATE_KEY_PREVIOUS";
const EMAIL_DATA_KEY_CURRENT_VERSION: &str = "TASKVEIL_EMAIL_DATA_KEY_CURRENT_VERSION";
const EMAIL_DATA_KEY_CURRENT: &str = "TASKVEIL_EMAIL_DATA_KEY_CURRENT";
const EMAIL_DATA_KEY_PREVIOUS_VERSION: &str = "TASKVEIL_EMAIL_DATA_KEY_PREVIOUS_VERSION";
const EMAIL_DATA_KEY_PREVIOUS: &str = "TASKVEIL_EMAIL_DATA_KEY_PREVIOUS";
const EMAIL_DELIVERY_ENDPOINT: &str = "TASKVEIL_EMAIL_DELIVERY_ENDPOINT";
const EMAIL_DELIVERY_SIGNING_KEY_ID: &str = "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY_ID";
const EMAIL_DELIVERY_SIGNING_KEY: &str = "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY";
const EMAIL_DISPATCH_TRIGGER_KEY: &str = "TASKVEIL_EMAIL_DISPATCH_TRIGGER_KEY";

pub struct RuntimeConfig {
    pub database_url: String,
    pub billing: BillingService,
    pub realtime: RealtimeGateway,
    pub auth_issuer: String,
    pub resync_tokens: ResyncTokenKeyring,
    pub auth_limit_hmac_key: [u8; 32],
    pub auth_limit_hmac_key_generation: AuthLimitKeyGeneration,
    pub trust_source_ip_header: bool,
    pub email_verification: EmailVerificationService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthLimitKeyGeneration(NonZeroU64);

impl AuthLimitKeyGeneration {
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for AuthLimitKeyGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
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
    #[error("authentication limit HMAC key is invalid")]
    InvalidAuthLimitHmacKey,
    #[error("authentication limit HMAC key generation is invalid")]
    InvalidAuthLimitHmacKeyGeneration,
    #[error("trusted source IP header setting is invalid")]
    InvalidTrustedSourceIpHeader,
    #[error("email verification configuration is invalid")]
    InvalidEmailVerification,
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
                if matches!(
                    name,
                    TRUST_SOURCE_IP_HEADER | AUTH_LIMIT_HMAC_KEY_GENERATION
                ) {
                    return local_lookup(name);
                }
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
        let auth_limit_hmac_key = decode_auth_limit_hmac_key(
            &lookup(AUTH_LIMIT_HMAC_KEY).ok_or(RuntimeConfigError::Missing(AUTH_LIMIT_HMAC_KEY))?,
        )?;
        let auth_limit_hmac_key_generation = lookup(AUTH_LIMIT_HMAC_KEY_GENERATION)
            .ok_or(RuntimeConfigError::Missing(AUTH_LIMIT_HMAC_KEY_GENERATION))?
            .parse::<NonZeroU64>()
            .map(AuthLimitKeyGeneration)
            .map_err(|_| RuntimeConfigError::InvalidAuthLimitHmacKeyGeneration)?;
        let trust_source_ip_header = lookup(TRUST_SOURCE_IP_HEADER)
            .map(|value| match value.as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(RuntimeConfigError::InvalidTrustedSourceIpHeader),
            })
            .transpose()?
            .unwrap_or(false);
        let billing = BillingService::from_values(environment, lookup)?;
        let realtime = RealtimeGateway::from_string_values(lookup)?;
        let resync_tokens = ResyncTokenKeyring::from_string_values(lookup)?;
        let email_verification = EmailVerificationService::new(EmailVerificationConfig {
            token_key_current_version: key_version(lookup, EMAIL_TOKEN_KEY_CURRENT_VERSION)?,
            token_key_current: secret_key(lookup, EMAIL_TOKEN_KEY_CURRENT)?,
            token_key_previous: optional_versioned_key(
                lookup,
                EMAIL_TOKEN_KEY_PREVIOUS_VERSION,
                EMAIL_TOKEN_KEY_PREVIOUS,
            )?,
            state_key_current_version: key_version(lookup, EMAIL_STATE_KEY_CURRENT_VERSION)?,
            state_key_current: secret_key(lookup, EMAIL_STATE_KEY_CURRENT)?,
            state_key_previous: optional_versioned_key(
                lookup,
                EMAIL_STATE_KEY_PREVIOUS_VERSION,
                EMAIL_STATE_KEY_PREVIOUS,
            )?,
            delivery_key_current_version: key_version(lookup, EMAIL_DATA_KEY_CURRENT_VERSION)?,
            delivery_key_current: secret_key(lookup, EMAIL_DATA_KEY_CURRENT)?,
            delivery_key_previous: optional_versioned_key(
                lookup,
                EMAIL_DATA_KEY_PREVIOUS_VERSION,
                EMAIL_DATA_KEY_PREVIOUS,
            )?,
            delivery_endpoint: required(lookup, EMAIL_DELIVERY_ENDPOINT)?,
            delivery_signing_key_id: required(lookup, EMAIL_DELIVERY_SIGNING_KEY_ID)?,
            delivery_signing_key: secret_key(lookup, EMAIL_DELIVERY_SIGNING_KEY)?,
            dispatch_trigger_key: secret_key(lookup, EMAIL_DISPATCH_TRIGGER_KEY)?,
        })
        .map_err(|_| RuntimeConfigError::InvalidEmailVerification)?;
        Ok(Self {
            database_url,
            billing,
            realtime,
            auth_issuer,
            resync_tokens,
            auth_limit_hmac_key,
            auth_limit_hmac_key_generation,
            trust_source_ip_header,
            email_verification,
        })
    }
}

fn required(
    lookup: impl Fn(&'static str) -> Option<String>,
    name: &'static str,
) -> Result<String, RuntimeConfigError> {
    lookup(name).ok_or(RuntimeConfigError::Missing(name))
}

fn key_version(
    lookup: impl Fn(&'static str) -> Option<String>,
    name: &'static str,
) -> Result<u32, RuntimeConfigError> {
    required(lookup, name)?
        .parse::<u32>()
        .ok()
        .filter(|version| *version > 0)
        .ok_or(RuntimeConfigError::InvalidEmailVerification)
}

fn secret_key(
    lookup: impl Fn(&'static str) -> Option<String>,
    name: &'static str,
) -> Result<[u8; 32], RuntimeConfigError> {
    STANDARD
        .decode(required(lookup, name)?)
        .ok()
        .and_then(|key| key.try_into().ok())
        .ok_or(RuntimeConfigError::InvalidEmailVerification)
}

fn optional_versioned_key(
    lookup: impl Fn(&'static str) -> Option<String> + Copy,
    version_name: &'static str,
    key_name: &'static str,
) -> Result<Option<(u32, [u8; 32])>, RuntimeConfigError> {
    match (lookup(version_name), lookup(key_name)) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Ok(Some((
            key_version(lookup, version_name)?,
            secret_key(lookup, key_name)?,
        ))),
        _ => Err(RuntimeConfigError::InvalidEmailVerification),
    }
}

fn decode_auth_limit_hmac_key(value: &str) -> Result<[u8; 32], RuntimeConfigError> {
    STANDARD
        .decode(value)
        .map_err(|_| RuntimeConfigError::InvalidAuthLimitHmacKey)?
        .try_into()
        .map_err(|_| RuntimeConfigError::InvalidAuthLimitHmacKey)
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
            "TASKVEIL_AUTH_LIMIT_HMAC_KEY":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "TASKVEIL_AUTH_LIMIT_HMAC_KEY_GENERATION":"1",
            "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "TASKVEIL_EMAIL_STATE_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_STATE_KEY_CURRENT":"BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ=",
            "TASKVEIL_EMAIL_DATA_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_DATA_KEY_CURRENT":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
            "TASKVEIL_EMAIL_DELIVERY_ENDPOINT":"https://email.staging.taskveil.example/v1/enqueue",
            "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY_ID":"email-sign-v1",
            "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY":"AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=",
            "TASKVEIL_EMAIL_DISPATCH_TRIGGER_KEY":"AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=",
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
            "TASKVEIL_AUTH_LIMIT_HMAC_KEY":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "TASKVEIL_AUTH_LIMIT_HMAC_KEY_GENERATION":"2",
            "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_TOKEN_KEY_CURRENT":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "TASKVEIL_EMAIL_STATE_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_STATE_KEY_CURRENT":"BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ=",
            "TASKVEIL_EMAIL_DATA_KEY_CURRENT_VERSION":"1",
            "TASKVEIL_EMAIL_DATA_KEY_CURRENT":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
            "TASKVEIL_EMAIL_DELIVERY_ENDPOINT":"https://email.taskveil.example/v1/enqueue",
            "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY_ID":"email-sign-v1",
            "TASKVEIL_EMAIL_DELIVERY_SIGNING_KEY":"AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=",
            "TASKVEIL_EMAIL_DISPATCH_TRIGGER_KEY":"AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=",
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
            |name| match name {
                TRUST_SOURCE_IP_HEADER => None,
                AUTH_LIMIT_HMAC_KEY_GENERATION => Some("7".to_string()),
                _ => panic!("unexpected local config lookup: {name}"),
            },
        )
        .await
        .expect("extension-backed config");
        assert_eq!(config.billing.environment(), BillingEnvironment::Sandbox);
        assert_eq!(config.auth_limit_hmac_key_generation.get(), 7);
        assert!(!config.trust_source_ip_header);
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
    fn authentication_limit_key_requires_exactly_32_base64_bytes() {
        assert_eq!(
            decode_auth_limit_hmac_key("not-base64")
                .unwrap_err()
                .to_string(),
            "authentication limit HMAC key is invalid"
        );
        assert_eq!(
            decode_auth_limit_hmac_key("c2hvcnQ=")
                .unwrap_err()
                .to_string(),
            "authentication limit HMAC key is invalid"
        );
        decode_auth_limit_hmac_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .expect("32-byte key");
    }

    #[tokio::test]
    async fn trusted_source_header_requires_an_explicit_boolean_setting() {
        let values: HashMap<String, String> = serde_json::from_str(sandbox_secret()).unwrap();
        let config = RuntimeConfig::load_from_sources(
            BillingEnvironment::Sandbox,
            Some("taskveil-staging/runtime".to_string()),
            move |_| async move { Ok(values) },
            |name| match name {
                TRUST_SOURCE_IP_HEADER => Some("true".to_string()),
                AUTH_LIMIT_HMAC_KEY_GENERATION => Some("1".to_string()),
                _ => None,
            },
        )
        .await
        .expect("trusted ingress config");
        assert!(config.trust_source_ip_header);

        let values: HashMap<String, String> = serde_json::from_str(sandbox_secret()).unwrap();
        let error = RuntimeConfig::load_from_sources(
            BillingEnvironment::Sandbox,
            Some("taskveil-staging/runtime".to_string()),
            move |_| async move { Ok(values) },
            |name| match name {
                TRUST_SOURCE_IP_HEADER => Some("yes".to_string()),
                AUTH_LIMIT_HMAC_KEY_GENERATION => Some("1".to_string()),
                _ => None,
            },
        )
        .await
        .err()
        .expect("non-boolean trust setting must fail");
        assert!(matches!(
            error,
            RuntimeConfigError::InvalidTrustedSourceIpHeader
        ));
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

    #[test]
    fn authentication_limit_key_generation_is_positive_and_nonsecret() {
        let mut values: HashMap<String, String> = serde_json::from_str(sandbox_secret()).unwrap();
        values.insert(AUTH_LIMIT_HMAC_KEY_GENERATION.to_string(), "0".to_string());
        let error = RuntimeConfig::from_values(BillingEnvironment::Sandbox, |name| {
            values.get(name).cloned()
        })
        .err()
        .expect("zero generation must fail");
        assert!(matches!(
            error,
            RuntimeConfigError::InvalidAuthLimitHmacKeyGeneration
        ));
    }
}
