use std::{
    collections::{HashMap, HashSet},
    env,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx_postgres::{PgPool, PgTransaction};
use thiserror::Error;
use uuid::Uuid;

use crate::{auth, db, AppError};

pub const PRO_LOOKUP_KEY: &str = "pro";
pub const MONTHLY_PRODUCT_ID: &str = "com.taskveil.app.pro.monthly";
pub const YEARLY_PRODUCT_ID: &str = "com.taskveil.app.pro.yearly";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BillingEnvironment {
    Sandbox,
    Production,
}

impl BillingEnvironment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }

    fn env_prefix(self) -> &'static str {
        match self {
            Self::Sandbox => "REVENUECAT_SANDBOX",
            Self::Production => "REVENUECAT_PRODUCTION",
        }
    }
}

impl FromStr for BillingEnvironment {
    type Err = BillingConfigurationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "sandbox" => Ok(Self::Sandbox),
            "production" => Ok(Self::Production),
            _ => Err(BillingConfigurationError::InvalidEnvironment),
        }
    }
}

#[derive(Debug, Error)]
pub enum BillingConfigurationError {
    #[error("missing billing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid billing environment")]
    InvalidEnvironment,
}

#[derive(Debug, Clone)]
struct RevenueCatEnvironmentConfig {
    project_id: String,
    app_id: String,
    secret_key: String,
    webhook_authorization: String,
    webhook_hmac_secret: String,
}

#[derive(Debug, Clone)]
pub struct BillingSettings {
    pub environment: BillingEnvironment,
    pub project_id: String,
    pub app_id: String,
    pub secret_key: String,
    pub webhook_authorization: String,
    pub webhook_hmac_secret: String,
}

impl RevenueCatEnvironmentConfig {
    fn from_values(
        environment: BillingEnvironment,
        lookup: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, BillingConfigurationError> {
        let prefix = environment.env_prefix();
        let required = |suffix: &'static str| {
            let name = match (environment, suffix) {
                (BillingEnvironment::Sandbox, "PROJECT_ID") => "REVENUECAT_SANDBOX_PROJECT_ID",
                (BillingEnvironment::Sandbox, "APP_ID") => "REVENUECAT_SANDBOX_APP_ID",
                (BillingEnvironment::Sandbox, "SECRET_KEY") => "REVENUECAT_SANDBOX_SECRET_KEY",
                (BillingEnvironment::Sandbox, "WEBHOOK_AUTHORIZATION") => {
                    "REVENUECAT_SANDBOX_WEBHOOK_AUTHORIZATION"
                }
                (BillingEnvironment::Sandbox, "WEBHOOK_HMAC_SECRET") => {
                    "REVENUECAT_SANDBOX_WEBHOOK_HMAC_SECRET"
                }
                (BillingEnvironment::Production, "PROJECT_ID") => {
                    "REVENUECAT_PRODUCTION_PROJECT_ID"
                }
                (BillingEnvironment::Production, "APP_ID") => "REVENUECAT_PRODUCTION_APP_ID",
                (BillingEnvironment::Production, "SECRET_KEY") => {
                    "REVENUECAT_PRODUCTION_SECRET_KEY"
                }
                (BillingEnvironment::Production, "WEBHOOK_AUTHORIZATION") => {
                    "REVENUECAT_PRODUCTION_WEBHOOK_AUTHORIZATION"
                }
                (BillingEnvironment::Production, "WEBHOOK_HMAC_SECRET") => {
                    "REVENUECAT_PRODUCTION_WEBHOOK_HMAC_SECRET"
                }
                _ => "REVENUECAT_CONFIGURATION",
            };
            debug_assert_eq!(name, format!("{prefix}_{suffix}"));
            lookup(name).ok_or(match (environment, suffix) {
                (BillingEnvironment::Sandbox, "PROJECT_ID") => {
                    BillingConfigurationError::Missing("REVENUECAT_SANDBOX_PROJECT_ID")
                }
                (BillingEnvironment::Sandbox, "APP_ID") => {
                    BillingConfigurationError::Missing("REVENUECAT_SANDBOX_APP_ID")
                }
                (BillingEnvironment::Sandbox, "SECRET_KEY") => {
                    BillingConfigurationError::Missing("REVENUECAT_SANDBOX_SECRET_KEY")
                }
                (BillingEnvironment::Sandbox, "WEBHOOK_AUTHORIZATION") => {
                    BillingConfigurationError::Missing("REVENUECAT_SANDBOX_WEBHOOK_AUTHORIZATION")
                }
                (BillingEnvironment::Sandbox, "WEBHOOK_HMAC_SECRET") => {
                    BillingConfigurationError::Missing("REVENUECAT_SANDBOX_WEBHOOK_HMAC_SECRET")
                }
                (BillingEnvironment::Production, "PROJECT_ID") => {
                    BillingConfigurationError::Missing("REVENUECAT_PRODUCTION_PROJECT_ID")
                }
                (BillingEnvironment::Production, "APP_ID") => {
                    BillingConfigurationError::Missing("REVENUECAT_PRODUCTION_APP_ID")
                }
                (BillingEnvironment::Production, "SECRET_KEY") => {
                    BillingConfigurationError::Missing("REVENUECAT_PRODUCTION_SECRET_KEY")
                }
                (BillingEnvironment::Production, "WEBHOOK_AUTHORIZATION") => {
                    BillingConfigurationError::Missing(
                        "REVENUECAT_PRODUCTION_WEBHOOK_AUTHORIZATION",
                    )
                }
                (BillingEnvironment::Production, "WEBHOOK_HMAC_SECRET") => {
                    BillingConfigurationError::Missing("REVENUECAT_PRODUCTION_WEBHOOK_HMAC_SECRET")
                }
                _ => BillingConfigurationError::Missing("REVENUECAT_CONFIGURATION"),
            })
        };
        Ok(Self {
            project_id: required("PROJECT_ID")?,
            app_id: required("APP_ID")?,
            secret_key: required("SECRET_KEY")?,
            webhook_authorization: required("WEBHOOK_AUTHORIZATION")?,
            webhook_hmac_secret: required("WEBHOOK_HMAC_SECRET")?,
        })
    }
}

#[derive(Debug, Error, Clone, Copy)]
pub enum ProviderError {
    #[error("provider is unavailable")]
    Unavailable,
    #[error("provider response is invalid")]
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    Trial,
    Active,
    Grace,
    Expired,
    Revoked,
}

impl SubscriptionStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Trial => "trial",
            Self::Active => "active",
            Self::Grace => "grace",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }

    fn from_provider(value: &str) -> Self {
        match value {
            "trialing" => Self::Trial,
            "active" => Self::Active,
            "in_grace_period" => Self::Grace,
            // RevenueCat explicitly exposes `unknown` and may add future
            // states. They must revoke access instead of preserving an older
            // active aggregate through a provider parse failure.
            _ => Self::Expired,
        }
    }

    const fn allows_access(self) -> bool {
        matches!(self, Self::Trial | Self::Active | Self::Grace)
    }
}

#[derive(Debug, Clone)]
pub struct ProviderSubscriptionSnapshot {
    pub provider_subscription_id: String,
    pub provider_product_id: String,
    pub store_transaction_identifier: Option<String>,
    pub store_original_transaction_identifier: Option<String>,
    pub store_product_identifier: String,
    pub status: SubscriptionStatus,
    pub gives_access: bool,
    pub current_period_ends_at: Option<DateTime<Utc>>,
    pub access_expires_at: Option<DateTime<Utc>>,
    pub will_renew: Option<bool>,
    pub revocation_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderSnapshot {
    pub refresh_token: Option<Uuid>,
    pub observed_at: DateTime<Utc>,
    pub subscriptions: Vec<ProviderSubscriptionSnapshot>,
}

pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderSnapshot, ProviderError>> + Send + 'a>>;

pub trait BillingProvider: Send + Sync {
    fn snapshot<'a>(
        &'a self,
        app_user_id: Uuid,
        environment: BillingEnvironment,
    ) -> ProviderFuture<'a>;
}

#[derive(Clone)]
pub struct BillingService {
    environment: BillingEnvironment,
    configuration: Arc<RevenueCatEnvironmentConfig>,
    provider: Arc<dyn BillingProvider>,
}

impl BillingService {
    pub fn from_env() -> Result<Self, BillingConfigurationError> {
        let environment = env::var("TASKVEIL_BILLING_ENVIRONMENT")
            .map_err(|_| BillingConfigurationError::Missing("TASKVEIL_BILLING_ENVIRONMENT"))?
            .parse()?;
        let configuration =
            RevenueCatEnvironmentConfig::from_values(environment, |name| env::var(name).ok())?;
        Ok(Self::from_configuration(environment, configuration))
    }

    pub fn from_values(
        environment: BillingEnvironment,
        lookup: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, BillingConfigurationError> {
        let configuration = RevenueCatEnvironmentConfig::from_values(environment, lookup)?;
        Ok(Self::from_configuration(environment, configuration))
    }

    pub fn from_settings(settings: BillingSettings) -> Self {
        Self::from_configuration(
            settings.environment,
            RevenueCatEnvironmentConfig {
                project_id: settings.project_id,
                app_id: settings.app_id,
                secret_key: settings.secret_key,
                webhook_authorization: settings.webhook_authorization,
                webhook_hmac_secret: settings.webhook_hmac_secret,
            },
        )
    }

    fn from_configuration(
        environment: BillingEnvironment,
        configuration: RevenueCatEnvironmentConfig,
    ) -> Self {
        let configuration = Arc::new(configuration);
        let provider = Arc::new(RevenueCatProvider::new(environment, configuration.clone()));
        Self {
            environment,
            configuration,
            provider,
        }
    }

    #[doc(hidden)]
    pub fn for_tests(environment: BillingEnvironment, provider: Arc<dyn BillingProvider>) -> Self {
        let prefix = environment.as_str();
        let configuration = Arc::new(RevenueCatEnvironmentConfig {
            project_id: format!("test-{prefix}-project"),
            app_id: format!("test-{prefix}-app"),
            secret_key: format!("test-{prefix}-secret"),
            webhook_authorization: format!("test-{prefix}-authorization"),
            webhook_hmac_secret: format!("test-{prefix}-hmac"),
        });
        Self {
            environment,
            configuration,
            provider,
        }
    }

    #[doc(hidden)]
    pub fn unavailable_for_tests(environment: BillingEnvironment) -> Self {
        Self::for_tests(environment, Arc::new(UnavailableProvider))
    }

    pub const fn environment(&self) -> BillingEnvironment {
        self.environment
    }

    fn configuration(&self) -> &RevenueCatEnvironmentConfig {
        &self.configuration
    }

    pub async fn snapshot(
        &self,
        app_user_id: Uuid,
        environment: BillingEnvironment,
    ) -> Result<ProviderSnapshot, ProviderError> {
        self.provider.snapshot(app_user_id, environment).await
    }
}

async fn claim_provider_snapshot(
    pool: &PgPool,
    service: &BillingService,
    user_id: Uuid,
    provider_app_user_id: Uuid,
    environment: BillingEnvironment,
) -> Result<ProviderSnapshot, AppError> {
    let refresh_token = Uuid::now_v7();
    let claimed = match environment {
        BillingEnvironment::Sandbox => {
            sqlx::query!(
                "UPDATE billing_customers SET sandbox_refresh_token = $3
                 WHERE user_id = $1 AND provider_app_user_id = $2",
                user_id,
                provider_app_user_id,
                refresh_token
            )
            .execute(pool)
            .await?
        }
        BillingEnvironment::Production => {
            sqlx::query!(
                "UPDATE billing_customers SET production_refresh_token = $3
                 WHERE user_id = $1 AND provider_app_user_id = $2",
                user_id,
                provider_app_user_id,
                refresh_token
            )
            .execute(pool)
            .await?
        }
    };
    if claimed.rows_affected() != 1 {
        return Err(AppError::not_found("billing_customer_not_found"));
    }
    let mut snapshot = service
        .snapshot(provider_app_user_id, environment)
        .await
        .map_err(|_| AppError::service_unavailable("billing_provider_unavailable"))?;
    snapshot.refresh_token = Some(refresh_token);
    Ok(snapshot)
}

struct UnavailableProvider;

impl BillingProvider for UnavailableProvider {
    fn snapshot<'a>(
        &'a self,
        _app_user_id: Uuid,
        _environment: BillingEnvironment,
    ) -> ProviderFuture<'a> {
        Box::pin(async { Err(ProviderError::Unavailable) })
    }
}

struct RevenueCatProvider {
    http: reqwest::Client,
    environment: BillingEnvironment,
    configuration: Arc<RevenueCatEnvironmentConfig>,
}

impl RevenueCatProvider {
    fn new(
        environment: BillingEnvironment,
        configuration: Arc<RevenueCatEnvironmentConfig>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            environment,
            configuration,
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        config: &RevenueCatEnvironmentConfig,
        url: &str,
    ) -> Result<T, ProviderError> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&config.secret_key)
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        if !response.status().is_success() {
            return if response.status().is_server_error()
                || response.status() == StatusCode::TOO_MANY_REQUESTS
            {
                Err(ProviderError::Unavailable)
            } else {
                Err(ProviderError::InvalidResponse)
            };
        }
        response
            .json()
            .await
            .map_err(|_| ProviderError::InvalidResponse)
    }

    async fn paginated<T: for<'de> Deserialize<'de>>(
        &self,
        config: &RevenueCatEnvironmentConfig,
        first_url: String,
    ) -> Result<Vec<T>, ProviderError> {
        let mut result = Vec::new();
        let mut next = Some(first_url);
        let mut pages = 0usize;
        while let Some(url) = next {
            pages += 1;
            if pages > 100 {
                return Err(ProviderError::InvalidResponse);
            }
            let page: RevenueCatList<T> = self.get_json(config, &url).await?;
            result.extend(page.items);
            next = page
                .next_page
                .map(|value| revenuecat_page_url(&value))
                .transpose()?;
        }
        Ok(result)
    }
}

fn revenuecat_page_url(value: &str) -> Result<String, ProviderError> {
    if value.starts_with("/v2/") {
        Ok(format!("https://api.revenuecat.com{value}"))
    } else if value.starts_with("https://api.revenuecat.com/v2/") {
        Ok(value.to_string())
    } else {
        // Never forward the RevenueCat bearer credential to an origin or
        // plaintext URL supplied through pagination metadata.
        Err(ProviderError::InvalidResponse)
    }
}

impl BillingProvider for RevenueCatProvider {
    fn snapshot<'a>(
        &'a self,
        app_user_id: Uuid,
        environment: BillingEnvironment,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            if environment != self.environment {
                return Err(ProviderError::InvalidResponse);
            }
            let config = &self.configuration;
            let customer = app_user_id.to_string();
            let subscriptions_url = format!(
                "https://api.revenuecat.com/v2/projects/{}/customers/{customer}/subscriptions?environment={}&limit=100",
                config.project_id,
                environment.as_str()
            );
            let active_url = format!(
                "https://api.revenuecat.com/v2/projects/{}/customers/{customer}/active_entitlements?limit=100",
                config.project_id
            );
            let subscriptions: Vec<RevenueCatSubscription> =
                self.paginated(config, subscriptions_url).await?;
            let active: Vec<RevenueCatActiveEntitlement> =
                self.paginated(config, active_url).await?;
            let active_deadlines: HashMap<_, _> = active
                .into_iter()
                .map(|item| (item.entitlement_id, item.expires_at))
                .collect();
            let observed_at = Utc::now();
            let mut product_cache: HashMap<String, RevenueCatProduct> = HashMap::new();
            let mut normalized = Vec::new();

            for subscription in subscriptions {
                let Some(product_id) = subscription.product_id.clone() else {
                    continue;
                };
                let entitlement_id = subscription
                    .entitlements
                    .items
                    .iter()
                    .find(|item| item.lookup_key == PRO_LOOKUP_KEY)
                    .map(|item| item.id.clone());
                let access_expires_at = entitlement_id
                    .as_ref()
                    .and_then(|id| active_deadlines.get(id))
                    .and_then(|value| DateTime::from_timestamp_millis(*value));
                let product = if let Some(product) = product_cache.get(&product_id) {
                    product.clone()
                } else {
                    let url = format!(
                        "https://api.revenuecat.com/v2/projects/{}/products/{product_id}",
                        config.project_id
                    );
                    let product: RevenueCatProduct = self.get_json(config, &url).await?;
                    if product.app_id != config.app_id {
                        return Err(ProviderError::InvalidResponse);
                    }
                    product_cache.insert(product_id.clone(), product.clone());
                    product
                };
                let status = SubscriptionStatus::from_provider(&subscription.status);
                let gives_access = subscription.gives_access
                    && status.allows_access()
                    && access_expires_at.is_some_and(|deadline| deadline > observed_at);
                normalized.push(ProviderSubscriptionSnapshot {
                    provider_subscription_id: subscription.id,
                    provider_product_id: product_id,
                    store_transaction_identifier: Some(subscription.store_subscription_identifier),
                    store_original_transaction_identifier: None,
                    store_product_identifier: product.store_identifier,
                    status,
                    gives_access,
                    current_period_ends_at: subscription
                        .current_period_ends_at
                        .and_then(DateTime::from_timestamp_millis),
                    access_expires_at,
                    will_renew: Some(matches!(
                        subscription.auto_renewal_status.as_str(),
                        "will_renew" | "has_already_renewed" | "will_change_product"
                    )),
                    revocation_reason: None,
                });
            }
            Ok(ProviderSnapshot {
                refresh_token: None,
                observed_at,
                subscriptions: normalized,
            })
        })
    }
}

#[derive(Debug, Deserialize)]
struct RevenueCatList<T> {
    items: Vec<T>,
    next_page: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevenueCatSubscription {
    id: String,
    product_id: Option<String>,
    store_subscription_identifier: String,
    current_period_ends_at: Option<i64>,
    gives_access: bool,
    auto_renewal_status: String,
    status: String,
    entitlements: RevenueCatList<RevenueCatEntitlement>,
}

#[derive(Debug, Deserialize)]
struct RevenueCatEntitlement {
    id: String,
    lookup_key: String,
}

#[derive(Debug, Deserialize)]
struct RevenueCatActiveEntitlement {
    entitlement_id: String,
    expires_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct RevenueCatProduct {
    store_identifier: String,
    app_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct BillingEntitlement {
    pub lookup_key: String,
    pub status: String,
    pub sync_allowed: bool,
    pub store_product_identifier: Option<String>,
    pub expires_at: Option<i64>,
    pub grace_expires_at: Option<i64>,
    pub will_renew: Option<bool>,
    pub environment: String,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct BillingResponse {
    pub provider: String,
    pub provider_app_user_id: Uuid,
    pub entitlement: BillingEntitlement,
}

pub async fn authenticate_sync_request(
    pool: &PgPool,
    billing: &BillingService,
    bearer_token: &str,
    tenant_id: Uuid,
) -> Result<auth::AuthContext, AppError> {
    let context = auth::authenticate(pool, bearer_token, tenant_id).await?;
    require_sync_entitlement(pool, billing.environment(), tenant_id, context.user_id).await?;
    Ok(context)
}

pub async fn require_sync_entitlement(
    pool: &PgPool,
    environment: BillingEnvironment,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    db::set_user_context(&mut tx, user_id).await?;
    db::set_tenant_context(&mut tx, tenant_id).await?;
    let tenant = sqlx::query!(
        "SELECT kind, owner_user_id FROM tenants WHERE id = $1",
        tenant_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(AppError::invalid_bearer_token)?;
    if tenant.kind != "personal" {
        return Err(AppError::forbidden());
    }
    if tenant.owner_user_id != user_id {
        return Err(AppError::forbidden());
    }
    let row = sqlx::query!(
        "SELECT ae.status, ae.gives_access, ae.expires_at, ae.grace_expires_at,
                bs.user_id AS source_user_id, bs.revocation_reason
         FROM account_entitlements ae
         LEFT JOIN billing_subscriptions bs ON bs.id = ae.source_subscription_id
         WHERE ae.user_id = $1 AND ae.environment = $2 AND ae.lookup_key = 'pro'",
        user_id,
        environment.as_str()
    )
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some(row) = row else {
        return Err(AppError::payment_required("entitlement_required"));
    };
    let deadline = if row.status == "grace" {
        row.grace_expires_at
    } else {
        row.expires_at
    };
    let allowed = row.gives_access
        && matches!(row.status.as_str(), "trial" | "active" | "grace")
        && deadline.is_some_and(|value| value > Utc::now())
        && row.source_user_id == Some(user_id)
        && row.revocation_reason.is_none();
    if allowed {
        Ok(())
    } else {
        Err(AppError::payment_required("entitlement_required"))
    }
}

pub async fn get_billing(
    pool: &PgPool,
    environment: BillingEnvironment,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<BillingResponse, AppError> {
    require_personal_owner(pool, tenant_id, user_id).await?;
    let provider_app_user_id = sqlx::query_scalar!(
        "SELECT provider_app_user_id FROM billing_customers WHERE user_id = $1",
        user_id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(AppError::internal)?;
    let entitlement = load_entitlement(pool, environment, user_id).await?;
    Ok(BillingResponse {
        provider: "revenuecat".into(),
        provider_app_user_id,
        entitlement,
    })
}

pub async fn refresh_billing(
    pool: &PgPool,
    service: &BillingService,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<BillingResponse, AppError> {
    require_personal_owner(pool, tenant_id, user_id).await?;
    let provider_app_user_id = sqlx::query_scalar!(
        "SELECT provider_app_user_id FROM billing_customers WHERE user_id = $1",
        user_id
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(AppError::internal)?;
    let snapshot = claim_provider_snapshot(
        pool,
        service,
        user_id,
        provider_app_user_id,
        service.environment(),
    )
    .await?;
    apply_snapshot(
        pool,
        user_id,
        provider_app_user_id,
        service.environment(),
        snapshot,
        None,
    )
    .await?;
    get_billing(pool, service.environment(), tenant_id, user_id).await
}

async fn require_personal_owner(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    db::set_user_context(&mut tx, user_id).await?;
    db::set_tenant_context(&mut tx, tenant_id).await?;
    let row = sqlx::query!(
        "SELECT kind, owner_user_id FROM tenants WHERE id = $1",
        tenant_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(AppError::forbidden)?;
    tx.commit().await?;
    if row.kind == "personal" && row.owner_user_id == user_id {
        Ok(())
    } else {
        Err(AppError::forbidden())
    }
}

async fn load_entitlement(
    pool: &PgPool,
    environment: BillingEnvironment,
    user_id: Uuid,
) -> Result<BillingEntitlement, AppError> {
    let row = sqlx::query!(
        "SELECT status, gives_access, store_product_identifier, expires_at,
                grace_expires_at, will_renew, updated_at
         FROM account_entitlements
         WHERE user_id = $1 AND environment = $2 AND lookup_key = 'pro'",
        user_id,
        environment.as_str()
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(BillingEntitlement {
            lookup_key: PRO_LOOKUP_KEY.into(),
            status: "free".into(),
            sync_allowed: false,
            store_product_identifier: None,
            expires_at: None,
            grace_expires_at: None,
            will_renew: None,
            environment: environment.as_str().into(),
            updated_at: None,
        });
    };
    let deadline = if row.status == "grace" {
        row.grace_expires_at
    } else {
        row.expires_at
    };
    Ok(BillingEntitlement {
        lookup_key: PRO_LOOKUP_KEY.into(),
        sync_allowed: row.gives_access
            && matches!(row.status.as_str(), "trial" | "active" | "grace")
            && deadline.is_some_and(|value| value > Utc::now()),
        status: row.status,
        store_product_identifier: row.store_product_identifier,
        expires_at: row.expires_at.map(|value| value.timestamp_millis()),
        grace_expires_at: row.grace_expires_at.map(|value| value.timestamp_millis()),
        will_renew: row.will_renew,
        environment: environment.as_str().into(),
        updated_at: Some(row.updated_at.timestamp_millis()),
    })
}

pub async fn apply_snapshot(
    pool: &PgPool,
    user_id: Uuid,
    provider_app_user_id: Uuid,
    environment: BillingEnvironment,
    snapshot: ProviderSnapshot,
    processed_event: Option<(&str, &str)>,
) -> Result<(), AppError> {
    apply_snapshot_with_revocation(
        pool,
        user_id,
        provider_app_user_id,
        environment,
        snapshot,
        processed_event,
        None,
    )
    .await
}

#[derive(Debug)]
enum SnapshotRevocation {
    Refund {
        store_transaction_identifier: String,
    },
    TransferAway,
}

async fn apply_snapshot_with_revocation(
    pool: &PgPool,
    user_id: Uuid,
    provider_app_user_id: Uuid,
    environment: BillingEnvironment,
    snapshot: ProviderSnapshot,
    processed_event: Option<(&str, &str)>,
    revocation: Option<SnapshotRevocation>,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let customer = sqlx::query!(
        "SELECT user_id, sandbox_refresh_token, production_refresh_token
         FROM billing_customers
         WHERE provider_app_user_id = $1 FOR UPDATE",
        provider_app_user_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("billing_customer_not_found"))?;
    if customer.user_id != user_id {
        return Err(AppError::conflict("billing_subscription_owner_conflict"));
    }
    let current_refresh_token = match environment {
        BillingEnvironment::Sandbox => customer.sandbox_refresh_token,
        BillingEnvironment::Production => customer.production_refresh_token,
    };
    if snapshot
        .refresh_token
        .is_some_and(|token| current_refresh_token != Some(token))
    {
        return Err(AppError::service_unavailable("billing_refresh_superseded"));
    }

    let observed_at = snapshot.observed_at;
    let mut seen = HashSet::new();
    for subscription in snapshot.subscriptions {
        if !matches!(
            subscription.store_product_identifier.as_str(),
            MONTHLY_PRODUCT_ID | YEARLY_PRODUCT_ID
        ) {
            continue;
        }
        if !seen.insert(subscription.provider_subscription_id.clone()) {
            return Err(AppError::service_unavailable(
                "billing_provider_invalid_response",
            ));
        }
        let existing = sqlx::query_scalar!(
            "SELECT user_id FROM billing_subscriptions
             WHERE provider = 'revenuecat' AND environment = $1
               AND provider_subscription_id = $2",
            environment.as_str(),
            subscription.provider_subscription_id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if existing.is_some_and(|existing_user| existing_user != user_id) {
            return Err(AppError::conflict("billing_subscription_owner_conflict"));
        }
        sqlx::query!(
            "INSERT INTO billing_subscriptions
                (user_id, provider, environment, provider_subscription_id,
                 store_transaction_identifier, store_original_transaction_identifier,
                 store_product_identifier, provider_product_id, status, gives_access,
                 current_period_ends_at, access_expires_at, will_renew, revocation_reason,
                 provider_observed_at, last_seen_at)
             VALUES ($1, 'revenuecat', $2, $3, $4, $5, $6, $7, $8, $9,
                     $10, $11, $12, $13, $14, $14)
             ON CONFLICT (provider, environment, provider_subscription_id) DO UPDATE SET
                 store_transaction_identifier = EXCLUDED.store_transaction_identifier,
                 store_original_transaction_identifier = COALESCE(
                     EXCLUDED.store_original_transaction_identifier,
                     billing_subscriptions.store_original_transaction_identifier
                 ),
                 store_product_identifier = EXCLUDED.store_product_identifier,
                 provider_product_id = EXCLUDED.provider_product_id,
                 status = CASE
                     WHEN billing_subscriptions.revocation_reason = 'ownership_transfer'
                         THEN 'revoked'
                     WHEN billing_subscriptions.revocation_reason = 'refund'
                          AND (NOT EXCLUDED.gives_access
                               OR EXCLUDED.store_transaction_identifier
                                  IS NOT DISTINCT FROM billing_subscriptions.store_transaction_identifier)
                         THEN 'revoked'
                     ELSE EXCLUDED.status
                 END,
                 gives_access = CASE
                     WHEN billing_subscriptions.revocation_reason = 'ownership_transfer'
                         THEN FALSE
                     WHEN billing_subscriptions.revocation_reason = 'refund'
                          AND (NOT EXCLUDED.gives_access
                               OR EXCLUDED.store_transaction_identifier
                                  IS NOT DISTINCT FROM billing_subscriptions.store_transaction_identifier)
                         THEN FALSE
                     ELSE EXCLUDED.gives_access
                 END,
                 current_period_ends_at = EXCLUDED.current_period_ends_at,
                 access_expires_at = EXCLUDED.access_expires_at,
                 will_renew = EXCLUDED.will_renew,
                 revocation_reason = CASE
                     WHEN billing_subscriptions.revocation_reason = 'ownership_transfer'
                         THEN 'ownership_transfer'
                     WHEN billing_subscriptions.revocation_reason = 'refund'
                          AND (NOT EXCLUDED.gives_access
                               OR EXCLUDED.store_transaction_identifier
                                  IS NOT DISTINCT FROM billing_subscriptions.store_transaction_identifier)
                         THEN 'refund'
                     ELSE EXCLUDED.revocation_reason
                 END,
                 provider_observed_at = EXCLUDED.provider_observed_at,
                 last_seen_at = EXCLUDED.last_seen_at,
                 updated_at = now()",
            user_id,
            environment.as_str(),
            subscription.provider_subscription_id,
            subscription.store_transaction_identifier,
            subscription.store_original_transaction_identifier,
            subscription.store_product_identifier,
            subscription.provider_product_id,
            subscription.status.as_str(),
            subscription.gives_access,
            subscription.current_period_ends_at,
            subscription.access_expires_at,
            subscription.will_renew,
            subscription.revocation_reason,
            observed_at
        )
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query!(
        "UPDATE billing_subscriptions
         SET status = CASE WHEN revocation_reason IS NULL THEN 'expired' ELSE 'revoked' END,
             gives_access = FALSE, access_expires_at = NULL,
             will_renew = FALSE, provider_observed_at = $3, updated_at = now()
         WHERE user_id = $1 AND environment = $2 AND provider = 'revenuecat'
           AND last_seen_at < $3",
        user_id,
        environment.as_str(),
        observed_at
    )
    .execute(&mut *tx)
    .await?;

    match revocation {
        Some(SnapshotRevocation::Refund {
            store_transaction_identifier,
        }) => {
            sqlx::query!(
                "UPDATE billing_subscriptions
                 SET status = 'revoked', gives_access = FALSE,
                     access_expires_at = NULL, will_renew = FALSE,
                     revocation_reason = 'refund', updated_at = now()
                 WHERE user_id = $1 AND environment = $2 AND provider = 'revenuecat'
                   AND store_transaction_identifier = $3",
                user_id,
                environment.as_str(),
                store_transaction_identifier
            )
            .execute(&mut *tx)
            .await?;
        }
        Some(SnapshotRevocation::TransferAway) => {
            sqlx::query!(
                "UPDATE billing_subscriptions
                 SET status = 'revoked', gives_access = FALSE,
                     access_expires_at = NULL, will_renew = FALSE,
                     revocation_reason = 'ownership_transfer', updated_at = now()
                 WHERE user_id = $1 AND environment = $2 AND provider = 'revenuecat'
                   AND provider_observed_at = $3 AND gives_access = FALSE",
                user_id,
                environment.as_str(),
                observed_at
            )
            .execute(&mut *tx)
            .await?;
        }
        None => {}
    }

    aggregate_entitlement(&mut tx, user_id, environment, observed_at).await?;
    if let Some((project_id, provider_event_id)) = processed_event {
        sqlx::query!(
            "UPDATE billing_events SET processing_status = 'processed',
                    processing_error_code = NULL, processed_at = now()
             WHERE provider = 'revenuecat' AND project_id = $1 AND provider_event_id = $2",
            project_id,
            provider_event_id
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RevenueCatWebhookEnvelope {
    event: RevenueCatWebhookEvent,
}

#[derive(Debug, Deserialize)]
struct RevenueCatWebhookEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    app_id: String,
    environment: String,
    app_user_id: Option<String>,
    original_app_user_id: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    transferred_from: Vec<String>,
    #[serde(default)]
    transferred_to: Vec<String>,
    cancel_reason: Option<String>,
    store: Option<String>,
    product_id: Option<String>,
    transaction_id: Option<String>,
    original_transaction_id: Option<String>,
    price_in_purchased_currency: Option<f64>,
    currency: Option<String>,
    country_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookOutcome {
    Processed,
    Duplicate,
}

pub async fn process_revenuecat_webhook(
    pool: &PgPool,
    service: &BillingService,
    authorization: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
) -> Result<WebhookOutcome, AppError> {
    let authorization = authorization.ok_or_else(AppError::unauthorized)?;
    let environment = service.environment();
    let config = service.configuration();
    if !constant_time_equal(
        authorization.as_bytes(),
        config.webhook_authorization.as_bytes(),
    ) {
        return Err(AppError::unauthorized());
    }
    verify_webhook_signature(
        config.webhook_hmac_secret.as_bytes(),
        signature.ok_or_else(AppError::unauthorized)?,
        body,
        Utc::now().timestamp(),
    )?;

    let envelope: RevenueCatWebhookEnvelope =
        serde_json::from_slice(body).map_err(|_| AppError::bad_request("invalid_webhook"))?;
    let webhook_environment = match envelope.event.environment.as_str() {
        "SANDBOX" => BillingEnvironment::Sandbox,
        "PRODUCTION" => BillingEnvironment::Production,
        _ => return Err(AppError::bad_request("invalid_webhook")),
    };
    if webhook_environment != environment || envelope.event.app_id != config.app_id {
        return Err(AppError::forbidden());
    }
    validate_event_field(&envelope.event.id, 255)?;
    validate_event_field(&envelope.event.event_type, 255)?;
    validate_optional_code(envelope.event.currency.as_deref(), 3)?;
    validate_optional_code(envelope.event.country_code.as_deref(), 2)?;

    let transfer = envelope.event.event_type == "TRANSFER";
    let identity_values: Vec<&str> = if transfer {
        if envelope.event.transferred_from.is_empty() || envelope.event.transferred_to.is_empty() {
            return Err(AppError::bad_request("invalid_webhook"));
        }
        envelope
            .event
            .transferred_from
            .iter()
            .map(String::as_str)
            .collect()
    } else {
        envelope
            .event
            .app_user_id
            .as_deref()
            .into_iter()
            .chain(envelope.event.original_app_user_id.as_deref())
            .chain(envelope.event.aliases.iter().map(String::as_str))
            .collect()
    };
    let mut customer_ids = Vec::new();
    for value in identity_values {
        if let Ok(id) = Uuid::parse_str(value) {
            if !customer_ids.contains(&id) {
                customer_ids.push(id);
            }
        }
    }
    if customer_ids.is_empty() {
        return Err(AppError::not_found("billing_customer_not_found"));
    }
    let mut resolved = Vec::new();
    for provider_app_user_id in customer_ids {
        if let Some(user_id) = sqlx::query_scalar!(
            "SELECT user_id FROM billing_customers WHERE provider_app_user_id = $1",
            provider_app_user_id
        )
        .fetch_optional(pool)
        .await?
        {
            if !resolved
                .iter()
                .any(|(known_user, _)| *known_user == user_id)
            {
                resolved.push((user_id, provider_app_user_id));
            }
        }
    }
    if resolved.len() != 1 {
        return if resolved.is_empty() {
            Err(AppError::not_found("billing_customer_not_found"))
        } else {
            Err(AppError::conflict("billing_customer_alias_conflict"))
        };
    }
    let (user_id, provider_app_user_id) = resolved[0];
    let payload_sha256 = sha2::Sha256::digest(body).to_vec();
    let price = envelope
        .event
        .price_in_purchased_currency
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.4}"));

    let mut tx = pool.begin().await?;
    let inserted = sqlx::query!(
        "INSERT INTO billing_events
            (provider, project_id, provider_event_id, app_id, environment,
             provider_app_user_id, event_type, store, store_product_identifier,
             store_transaction_identifier, store_original_transaction_identifier,
             price, currency, country_code, payload_sha256)
         VALUES ('revenuecat', $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 $11::text::numeric, $12, $13, $14)
         ON CONFLICT (provider, project_id, provider_event_id) DO NOTHING",
        config.project_id,
        envelope.event.id,
        envelope.event.app_id,
        environment.as_str(),
        provider_app_user_id,
        envelope.event.event_type,
        envelope.event.store,
        envelope.event.product_id,
        envelope.event.transaction_id,
        envelope.event.original_transaction_id,
        price,
        envelope.event.currency,
        envelope.event.country_code,
        payload_sha256
    )
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let existing = sqlx::query!(
            "SELECT processing_status, processing_started_at FROM billing_events
             WHERE provider = 'revenuecat' AND project_id = $1 AND provider_event_id = $2
             FOR UPDATE",
            config.project_id,
            envelope.event.id
        )
        .fetch_one(&mut *tx)
        .await?;
        if existing.processing_status == "processed" {
            tx.commit().await?;
            return Ok(WebhookOutcome::Duplicate);
        }
        if existing.processing_status == "processing"
            && Utc::now() - existing.processing_started_at < chrono::Duration::minutes(2)
        {
            return Err(AppError::service_unavailable("billing_event_in_progress"));
        }
        sqlx::query!(
            "UPDATE billing_events SET processing_status = 'processing',
                    processing_started_at = now(), processing_error_code = NULL
             WHERE provider = 'revenuecat' AND project_id = $1 AND provider_event_id = $2",
            config.project_id,
            envelope.event.id
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let mut snapshot =
        match claim_provider_snapshot(pool, service, user_id, provider_app_user_id, environment)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                sqlx::query!(
                    "UPDATE billing_events SET processing_status = 'failed',
                        processing_error_code = 'provider_unavailable'
                 WHERE provider = 'revenuecat' AND project_id = $1 AND provider_event_id = $2",
                    config.project_id,
                    envelope.event.id
                )
                .execute(pool)
                .await?;
                return Err(error);
            }
        };
    if let (Some(transaction_id), Some(original_transaction_id)) = (
        envelope.event.transaction_id.as_deref(),
        envelope.event.original_transaction_id.as_deref(),
    ) {
        for subscription in &mut snapshot.subscriptions {
            if subscription.store_transaction_identifier.as_deref() == Some(transaction_id) {
                subscription.store_original_transaction_identifier =
                    Some(original_transaction_id.to_string());
            }
        }
    }
    let revocation = if transfer {
        Some(SnapshotRevocation::TransferAway)
    } else if envelope.event.cancel_reason.as_deref() == Some("CUSTOMER_SUPPORT") {
        let store_transaction_identifier = envelope
            .event
            .transaction_id
            .clone()
            .ok_or_else(|| AppError::bad_request("invalid_webhook"))?;
        Some(SnapshotRevocation::Refund {
            store_transaction_identifier,
        })
    } else {
        None
    };
    apply_snapshot_with_revocation(
        pool,
        user_id,
        provider_app_user_id,
        environment,
        snapshot,
        Some((&config.project_id, &envelope.event.id)),
        revocation,
    )
    .await?;
    Ok(WebhookOutcome::Processed)
}

fn validate_event_field(value: &str, max: usize) -> Result<(), AppError> {
    if value.is_empty() || value.len() > max {
        Err(AppError::bad_request("invalid_webhook"))
    } else {
        Ok(())
    }
}

fn validate_optional_code(value: Option<&str>, length: usize) -> Result<(), AppError> {
    if value.is_some_and(|value| value.len() != length || !value.is_ascii()) {
        Err(AppError::bad_request("invalid_webhook"))
    } else {
        Ok(())
    }
}

fn verify_webhook_signature(
    secret: &[u8],
    header: &str,
    body: &[u8],
    now_timestamp: i64,
) -> Result<(), AppError> {
    let mut timestamp = None;
    let mut signature = None;
    for part in header.split(',') {
        if let Some(value) = part.strip_prefix("t=") {
            timestamp = value.parse::<i64>().ok();
        } else if let Some(value) = part.strip_prefix("v1=") {
            signature = decode_hex(value);
        }
    }
    let timestamp = timestamp.ok_or_else(AppError::unauthorized)?;
    if now_timestamp.abs_diff(timestamp) > 300 {
        return Err(AppError::unauthorized());
    }
    let signature = signature.ok_or_else(AppError::unauthorized)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| AppError::unauthorized())?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| AppError::unauthorized())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let key = b"taskveil-revenuecat-webhook-authorization";
    let mut left_mac = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key");
    left_mac.update(left);
    let left_tag = left_mac.finalize().into_bytes();
    let mut right_mac = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key");
    right_mac.update(right);
    right_mac.verify_slice(&left_tag).is_ok()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

struct AggregatedEntitlement {
    source_id: Option<Uuid>,
    status: String,
    gives_access: bool,
    product: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    grace_expires_at: Option<DateTime<Utc>>,
    will_renew: Option<bool>,
}

async fn aggregate_entitlement(
    tx: &mut PgTransaction<'_>,
    user_id: Uuid,
    environment: BillingEnvironment,
    observed_at: DateTime<Utc>,
) -> Result<(), AppError> {
    let selected = sqlx::query!(
        "SELECT id, status, gives_access, store_product_identifier, access_expires_at,
                will_renew, revocation_reason
         FROM billing_subscriptions
         WHERE user_id = $1 AND environment = $2
           AND store_product_identifier IN ($3, $4)
         ORDER BY
           CASE WHEN gives_access AND access_expires_at > now()
                     AND status IN ('trial', 'active', 'grace') THEN 0 ELSE 1 END,
           access_expires_at DESC NULLS LAST,
           CASE status WHEN 'grace' THEN 0 WHEN 'active' THEN 1 WHEN 'trial' THEN 2
                       WHEN 'revoked' THEN 3 ELSE 4 END,
           updated_at DESC
         LIMIT 1",
        user_id,
        environment.as_str(),
        MONTHLY_PRODUCT_ID,
        YEARLY_PRODUCT_ID
    )
    .fetch_optional(&mut **tx)
    .await?;

    let result = if let Some(row) = selected {
        let gives_access = row.gives_access
            && matches!(row.status.as_str(), "trial" | "active" | "grace")
            && row
                .access_expires_at
                .is_some_and(|deadline| deadline > Utc::now())
            && row.revocation_reason.is_none();
        AggregatedEntitlement {
            source_id: Some(row.id),
            status: row.status.clone(),
            gives_access,
            product: Some(row.store_product_identifier),
            expires_at: if row.status == "grace" {
                None
            } else {
                row.access_expires_at
            },
            grace_expires_at: if row.status == "grace" {
                row.access_expires_at
            } else {
                None
            },
            will_renew: row.will_renew,
        }
    } else {
        AggregatedEntitlement {
            source_id: None,
            status: "free".into(),
            gives_access: false,
            product: None,
            expires_at: None,
            grace_expires_at: None,
            will_renew: None,
        }
    };
    sqlx::query!(
        "INSERT INTO account_entitlements
            (user_id, environment, lookup_key, status, gives_access,
             source_subscription_id, store_product_identifier, expires_at,
             grace_expires_at, will_renew, provider_observed_at)
         VALUES ($1, $2, 'pro', $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (user_id, environment, lookup_key) DO UPDATE SET
             status = EXCLUDED.status,
             gives_access = EXCLUDED.gives_access,
             source_subscription_id = EXCLUDED.source_subscription_id,
             store_product_identifier = EXCLUDED.store_product_identifier,
             expires_at = EXCLUDED.expires_at,
             grace_expires_at = EXCLUDED.grace_expires_at,
             will_renew = EXCLUDED.will_renew,
             provider_observed_at = EXCLUDED.provider_observed_at,
             updated_at = now()",
        user_id,
        environment.as_str(),
        result.status,
        result.gives_access,
        result.source_id,
        result.product,
        result.expires_at,
        result.grace_expires_at,
        result.will_renew,
        observed_at
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_statuses_are_fail_closed() {
        assert_eq!(
            SubscriptionStatus::from_provider("trialing"),
            SubscriptionStatus::Trial
        );
        assert_eq!(
            SubscriptionStatus::from_provider("in_grace_period"),
            SubscriptionStatus::Grace
        );
        assert_eq!(
            SubscriptionStatus::from_provider("unknown"),
            SubscriptionStatus::Expired
        );
        assert_eq!(
            SubscriptionStatus::from_provider("future_state"),
            SubscriptionStatus::Expired
        );
    }

    #[test]
    fn selected_environment_is_the_only_required_provider_configuration() {
        let sandbox = HashMap::from([
            ("REVENUECAT_SANDBOX_PROJECT_ID", "sandbox-project"),
            ("REVENUECAT_SANDBOX_APP_ID", "sandbox-app"),
            ("REVENUECAT_SANDBOX_SECRET_KEY", "sandbox-secret"),
            (
                "REVENUECAT_SANDBOX_WEBHOOK_AUTHORIZATION",
                "sandbox-authorization",
            ),
            ("REVENUECAT_SANDBOX_WEBHOOK_HMAC_SECRET", "sandbox-hmac"),
        ]);
        let service = BillingService::from_values(BillingEnvironment::Sandbox, |name| {
            sandbox.get(name).map(|value| (*value).to_string())
        })
        .expect("sandbox-only configuration");
        assert_eq!(service.environment(), BillingEnvironment::Sandbox);

        let error = BillingService::from_values(BillingEnvironment::Production, |name| {
            sandbox.get(name).map(|value| (*value).to_string())
        })
        .err()
        .expect("production cannot use sandbox configuration");
        assert!(error
            .to_string()
            .contains("REVENUECAT_PRODUCTION_PROJECT_ID"));
    }

    #[test]
    fn pagination_never_forwards_provider_credentials_to_another_origin() {
        assert_eq!(
            revenuecat_page_url("/v2/projects/proj/customers/customer/subscriptions?page=2")
                .unwrap(),
            "https://api.revenuecat.com/v2/projects/proj/customers/customer/subscriptions?page=2"
        );
        assert!(revenuecat_page_url("https://example.com/v2/steal").is_err());
        assert!(revenuecat_page_url("http://api.revenuecat.com/v2/steal").is_err());
    }

    #[test]
    fn webhook_signature_rejects_stale_and_tampered_payloads() {
        let timestamp = 1_700_000_000i64;
        let body = br#"{"event":{"id":"evt"}}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = mac.finalize().into_bytes();
        let header = format!("t={timestamp},v1={}", encode_hex(&signature));
        assert!(verify_webhook_signature(b"secret", &header, body, timestamp).is_ok());
        assert!(verify_webhook_signature(b"secret", &header, b"tampered", timestamp).is_err());
        assert!(verify_webhook_signature(b"secret", &header, body, timestamp + 301).is_err());
    }

    fn encode_hex(value: &[u8]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
