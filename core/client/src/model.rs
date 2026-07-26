//! Frontend-neutral account and synchronization views.

use serde::{Deserialize, Serialize};

/// Closed frontend setting surface. Internal metadata keys are intentionally
/// not representable by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendSettingKey {
    UiMode,
    OnboardingCompleted,
    CalendarWeekStart,
    TimerSettings,
    TimerRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSessionState {
    pub logged_in: bool,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub device_id: Option<String>,
    pub recovery_pending: bool,
}

impl AccountSessionState {
    pub fn logged_out() -> Self {
        Self {
            logged_in: false,
            email: None,
            user_id: None,
            tenant_id: None,
            device_id: None,
            recovery_pending: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAuthResult {
    pub session: AccountSessionState,
    /// Intentionally exported once after registration so the user can store it.
    pub recovery_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BillingState {
    pub provider: String,
    pub provider_app_user_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationSafetyState {
    pub owner_user_id: String,
    pub member_user_id: String,
    pub digest: String,
    pub decimal: String,
    pub qr_payload: String,
    pub verification_state: String,
    pub owner_confirmed: bool,
    pub member_confirmed: bool,
}

/// Frontend-neutral short-lived authorization for the realtime wake-up
/// channel. The ticket is intentionally not `Debug` so routine diagnostics
/// cannot accidentally print it.
pub struct RealtimeTicket {
    pub websocket_url: String,
    pub ticket: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStatus {
    pub logged_in: bool,
    pub running: bool,
    pub last_success_at: Option<i64>,
    pub last_failure_at: Option<i64>,
    pub last_error: Option<SyncFailure>,
    pub pushed_count: usize,
    pub push_acked_count: usize,
    pub push_superseded_count: usize,
    pub pulled_count: usize,
    pub applied_count: usize,
    pub deleted_count: usize,
    pub decrypt_failed_count: usize,
    pub repush_count: usize,
    pub missing_key_quarantined_count: usize,
    pub corruption_quarantined_count: usize,
    pub resolved_quarantine_count: usize,
    pub upgrade_required: bool,
}

/// Stable, frontend-neutral classification of the most recent sync failure.
///
/// This intentionally contains no server response, database detail, path,
/// identifier, or user input. Frontends can safely map it to localized copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncFailure {
    InvalidInput,
    NotFound,
    Conflict,
    Unauthorized,
    CredentialUnavailable,
    AccountBoundUnavailable,
    EntitlementRequired,
    UpgradeRequired,
    Busy,
    LeaseLost,
    ClockSkew,
    CryptoUnavailable,
    StorageFailure,
    SyncFailure,
    Internal,
}

impl SyncFailure {
    pub const fn kind(self) -> crate::ClientErrorKind {
        match self {
            Self::InvalidInput => crate::ClientErrorKind::InvalidInput,
            Self::NotFound => crate::ClientErrorKind::NotFound,
            Self::Conflict => crate::ClientErrorKind::Conflict,
            Self::Unauthorized => crate::ClientErrorKind::Unauthorized,
            Self::CredentialUnavailable => crate::ClientErrorKind::CredentialUnavailable,
            Self::AccountBoundUnavailable => crate::ClientErrorKind::AccountBoundUnavailable,
            Self::EntitlementRequired => crate::ClientErrorKind::EntitlementRequired,
            Self::UpgradeRequired => crate::ClientErrorKind::UpgradeRequired,
            Self::Busy => crate::ClientErrorKind::Busy,
            Self::LeaseLost => crate::ClientErrorKind::LeaseLost,
            Self::ClockSkew => crate::ClientErrorKind::ClockSkew,
            Self::CryptoUnavailable => crate::ClientErrorKind::CryptoUnavailable,
            Self::StorageFailure => crate::ClientErrorKind::StorageFailure,
            Self::SyncFailure => crate::ClientErrorKind::SyncFailure,
            Self::Internal => crate::ClientErrorKind::Internal,
        }
    }
}

impl From<crate::ClientErrorKind> for SyncFailure {
    fn from(kind: crate::ClientErrorKind) -> Self {
        match kind {
            crate::ClientErrorKind::InvalidInput => Self::InvalidInput,
            crate::ClientErrorKind::NotFound => Self::NotFound,
            crate::ClientErrorKind::Conflict => Self::Conflict,
            crate::ClientErrorKind::Unauthorized => Self::Unauthorized,
            crate::ClientErrorKind::CredentialUnavailable => Self::CredentialUnavailable,
            crate::ClientErrorKind::AccountBoundUnavailable => Self::AccountBoundUnavailable,
            crate::ClientErrorKind::EntitlementRequired => Self::EntitlementRequired,
            crate::ClientErrorKind::UpgradeRequired => Self::UpgradeRequired,
            crate::ClientErrorKind::Busy => Self::Busy,
            crate::ClientErrorKind::LeaseLost => Self::LeaseLost,
            crate::ClientErrorKind::ClockSkew => Self::ClockSkew,
            crate::ClientErrorKind::CryptoUnavailable => Self::CryptoUnavailable,
            crate::ClientErrorKind::StorageFailure => Self::StorageFailure,
            crate::ClientErrorKind::SyncFailure => Self::SyncFailure,
            crate::ClientErrorKind::Internal => Self::Internal,
        }
    }
}
