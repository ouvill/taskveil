use taskveil_client::chrono::{DateTime, Utc};
use taskveil_client::{
    pomodoro_target_reached_at as domain_pomodoro_target_reached_at, AccountAuthResult,
    AccountRegistrationPending, AccountRegistrationPhase, AccountRegistrationState,
    AccountSessionState, ActiveTimerSession, BillingState, CalendarOccurrenceKind,
    CalendarOccurrenceView, CalendarRange, CivilDate, ClientError, ClientErrorKind,
    CompletedTimerSession, CreateTaskCommand, CreateTaskSeriesFromTaskCommand,
    CreateTaskSeriesFromTemplateCommand, CreateTemplateCommand, FrontendSettingKey, HomeTaskView,
    List, OrganizationSafetyState, RealtimeTicket, ReminderNotificationActionView,
    ReminderNotificationCommandView, ReminderView, ReorderTaskCommand, ReplaceTaskBlueprintCommand,
    SaveTemplateCommand, SetTaskStatusCommand, SettlementSummary, Streak, SyncFailure, SyncStatus,
    Task, TaskBlueprint, TaskBlueprintNode, TaskContent, TaskDue, TaskSeries, TaskStatus,
    TaskTemplate, TaskUndoKind, TaskUndoView, TimerFinishKind, TimerMode, TimerPhase,
    TimerRunState, UpdateTaskCommand, UpdateTaskSeriesCommand, UpdateTemplateCommand, UtcInstant,
    Uuid,
};

use crate::client_handle::{client, init_client};

mod conversions;

use conversions::*;

/// Closed, stable error codes exported to Flutter.
///
/// Variants are intentionally coarser than internal errors so implementation
/// details cannot become part of the UI or telemetry contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeErrorCodeDto {
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

/// Closed localization argument keys. No free-form internal string can cross
/// the bridge. Current errors need no arguments; this type reserves a safe,
/// reviewable extension point for future bounded values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeErrorArgumentKeyDto {
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeErrorArgumentDto {
    pub key: BridgeErrorArgumentKeyDto,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeErrorDto {
    pub code: BridgeErrorCodeDto,
    pub arguments: Vec<BridgeErrorArgumentDto>,
    pub retryable: bool,
}

impl BridgeErrorDto {
    fn new(code: BridgeErrorCodeDto, retryable: bool) -> Self {
        Self {
            code,
            arguments: Vec::new(),
            retryable,
        }
    }

    fn invalid_input() -> Self {
        Self::new(BridgeErrorCodeDto::InvalidInput, false)
    }
}

impl From<ClientError> for BridgeErrorDto {
    fn from(error: ClientError) -> Self {
        let code = match error.kind() {
            ClientErrorKind::InvalidInput => BridgeErrorCodeDto::InvalidInput,
            ClientErrorKind::NotFound => BridgeErrorCodeDto::NotFound,
            ClientErrorKind::Conflict => BridgeErrorCodeDto::Conflict,
            ClientErrorKind::Unauthorized => BridgeErrorCodeDto::Unauthorized,
            ClientErrorKind::CredentialUnavailable => BridgeErrorCodeDto::CredentialUnavailable,
            ClientErrorKind::AccountBoundUnavailable => BridgeErrorCodeDto::AccountBoundUnavailable,
            ClientErrorKind::EntitlementRequired => BridgeErrorCodeDto::EntitlementRequired,
            ClientErrorKind::UpgradeRequired => BridgeErrorCodeDto::UpgradeRequired,
            ClientErrorKind::Busy => BridgeErrorCodeDto::Busy,
            ClientErrorKind::LeaseLost => BridgeErrorCodeDto::LeaseLost,
            ClientErrorKind::ClockSkew => BridgeErrorCodeDto::ClockSkew,
            ClientErrorKind::CryptoUnavailable => BridgeErrorCodeDto::CryptoUnavailable,
            ClientErrorKind::StorageFailure => BridgeErrorCodeDto::StorageFailure,
            ClientErrorKind::SyncFailure => BridgeErrorCodeDto::SyncFailure,
            ClientErrorKind::Internal => BridgeErrorCodeDto::Internal,
        };
        Self::new(code, error.retryable())
    }
}

impl From<SyncFailure> for BridgeErrorDto {
    fn from(error: SyncFailure) -> Self {
        match error {
            SyncFailure::Unauthorized => Self::new(BridgeErrorCodeDto::Unauthorized, false),
            SyncFailure::UpgradeRequired => Self::new(BridgeErrorCodeDto::UpgradeRequired, false),
            SyncFailure::EntitlementRequired => {
                Self::new(BridgeErrorCodeDto::EntitlementRequired, false)
            }
            SyncFailure::SyncLeaseBusy
            | SyncFailure::DatabaseBusy
            | SyncFailure::ProfileBusy
            | SyncFailure::CredentialBusy => Self::new(BridgeErrorCodeDto::Busy, true),
            SyncFailure::LeaseLost => Self::new(BridgeErrorCodeDto::LeaseLost, true),
            SyncFailure::ClockSkewRetryable => Self::new(BridgeErrorCodeDto::ClockSkew, true),
            SyncFailure::CredentialUnavailable => {
                Self::new(BridgeErrorCodeDto::CredentialUnavailable, false)
            }
            SyncFailure::AccountBoundUnavailable => {
                Self::new(BridgeErrorCodeDto::AccountBoundUnavailable, false)
            }
            SyncFailure::SyncFailed => Self::new(BridgeErrorCodeDto::SyncFailure, true),
        }
    }
}

pub struct ListDto {
    pub id: String,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub sort_order: String,
    pub is_default: bool,
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct TaskDto {
    pub id: String,
    pub list_id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub note: String,
    pub status: String,
    pub priority: i32,
    pub due: Option<TaskDueDto>,
    pub scheduled_at: Option<i64>,
    pub estimated_minutes: Option<i32>,
    pub sort_order: String,
    pub completed_at: Option<i64>,
    pub closed_reason: Option<String>,
    pub deleted_at: Option<i64>,
    pub assignee: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct TaskBlueprintNodeDto {
    pub node_key: String,
    pub parent_node_key: Option<String>,
    pub sibling_order: u32,
    pub title: String,
    pub note: String,
    pub priority: i32,
    pub estimated_minutes: Option<i32>,
}

pub struct TemplateDto {
    pub id: String,
    pub name: String,
    pub default_list_id: Option<String>,
    pub blueprint_revision: String,
    pub nodes: Vec<TaskBlueprintNodeDto>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct TaskSeriesDto {
    pub id: String,
    pub target_list_id: Option<String>,
    pub nodes: Vec<TaskBlueprintNodeDto>,
    pub rrule: String,
    pub starts_at: i64,
    pub time_zone: String,
    pub next_run_at: Option<i64>,
    pub enabled: bool,
    pub config_revision: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct StreakDto {
    pub current: u32,
    pub finalized: bool,
}

pub struct SettlementSummaryDto {
    pub generated_occurrences: u32,
    pub generated_tasks: u32,
    pub has_more: bool,
    pub outbox_changed: bool,
}

pub enum TaskDueInput {
    Date {
        due_on: String,
    },
    DateTime {
        due_at: DateTime<Utc>,
        time_zone: String,
    },
}

pub enum TaskDueDto {
    Date {
        due_on: String,
    },
    DateTime {
        due_at: DateTime<Utc>,
        time_zone: String,
    },
}

pub struct TaskUndoDto {
    pub id: String,
    pub operation_type: String,
    pub task_id: String,
    pub list_id: String,
    pub task_title: String,
    pub created_at: i64,
}

pub struct HomeTaskDto {
    pub task: TaskDto,
    pub list_name: String,
    pub is_home_target: bool,
}

pub struct CalendarRangeInput {
    pub start_on: String,
    pub end_on: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
}

pub enum CalendarOccurrenceKindDto {
    DateDue {
        due_on: String,
    },
    DateTimeDue {
        due_at: DateTime<Utc>,
        time_zone: String,
    },
    Scheduled {
        scheduled_at: DateTime<Utc>,
    },
    Completed {
        completed_at: DateTime<Utc>,
    },
}

pub struct CalendarOccurrenceDto {
    pub task: TaskDto,
    pub list_name: String,
    pub list_archived: bool,
    pub kind: CalendarOccurrenceKindDto,
}

pub enum TimerModeDto {
    Pomodoro,
    Stopwatch,
}

pub enum TimerPhaseDto {
    Work,
    ShortBreak,
    LongBreak,
}

pub enum TimerRunStateDto {
    Running,
    Paused,
}

pub enum TimerFinishKindDto {
    Completed,
    Interrupted,
}

pub enum FrontendSettingKeyDto {
    UiMode,
    OnboardingCompleted,
    CalendarWeekStart,
    TimerSettings,
    TimerRuntime,
}

impl From<FrontendSettingKeyDto> for FrontendSettingKey {
    fn from(value: FrontendSettingKeyDto) -> Self {
        match value {
            FrontendSettingKeyDto::UiMode => Self::UiMode,
            FrontendSettingKeyDto::OnboardingCompleted => Self::OnboardingCompleted,
            FrontendSettingKeyDto::CalendarWeekStart => Self::CalendarWeekStart,
            FrontendSettingKeyDto::TimerSettings => Self::TimerSettings,
            FrontendSettingKeyDto::TimerRuntime => Self::TimerRuntime,
        }
    }
}

pub enum ActiveTimerStartOutcomeDto {
    Started,
    Conflict,
}

pub struct ActiveTimerSessionDto {
    pub session_id: String,
    pub task_id: Option<String>,
    pub mode: TimerModeDto,
    pub phase: TimerPhaseDto,
    pub state: TimerRunStateDto,
    pub started_at: DateTime<Utc>,
    pub last_resumed_at: Option<DateTime<Utc>>,
    pub accumulated_active_ms: i64,
    pub target_duration_ms: Option<i64>,
}

pub struct CompletedTimerSessionDto {
    pub id: String,
    pub task_id: String,
    pub mode: TimerModeDto,
    pub finish_kind: TimerFinishKindDto,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub active_duration_ms: i64,
    pub created_at: DateTime<Utc>,
}

pub struct ReminderDto {
    pub id: String,
    pub task_id: String,
    pub remind_at: i64,
    pub snoozed_until: Option<i64>,
    pub created_at: i64,
}

pub enum ReminderNotificationActionDto {
    Schedule,
    Cancel,
}

pub struct ReminderNotificationCommandDto {
    pub reminder_id: String,
    pub platform_id: i32,
    pub revision: i64,
    pub action: ReminderNotificationActionDto,
    pub task_id: Option<String>,
    pub list_id: Option<String>,
    pub scheduled_at: Option<i64>,
}

#[derive(Clone)]
pub struct AccountSessionStateDto {
    pub logged_in: bool,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub device_id: Option<String>,
    pub recovery_pending: bool,
}

pub struct AccountAuthResultDto {
    pub session: AccountSessionStateDto,
    pub recovery_key: Option<String>,
}

pub struct AccountRegistrationPendingDto {
    pub email: String,
    pub expires_at_ms: i64,
    pub next_retry_at_ms: i64,
}

pub struct AccountRegistrationStateDto {
    pub phase: String,
    pub email: String,
    pub expires_at_ms: i64,
    pub next_retry_at_ms: Option<i64>,
    pub can_cancel: bool,
}

#[derive(Clone)]
pub struct BillingStateDto {
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

pub enum SyncNowOutcomeDto {
    Synced { status: SyncStatusDto },
    BillingRequired,
}

pub struct OrganizationSafetyStateDto {
    pub owner_user_id: String,
    pub member_user_id: String,
    pub digest: String,
    pub decimal: String,
    pub qr_payload: String,
    pub verification_state: String,
    pub owner_confirmed: bool,
    pub member_confirmed: bool,
}

pub struct RealtimeTicketDto {
    pub websocket_url: String,
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SyncStatusDto {
    pub logged_in: bool,
    pub running: bool,
    pub last_success_at: Option<i64>,
    pub last_failure_at: Option<i64>,
    pub last_error: Option<BridgeErrorDto>,
    pub pushed_count: i32,
    pub push_acked_count: i32,
    pub push_superseded_count: i32,
    pub pulled_count: i32,
    pub applied_count: i32,
    pub deleted_count: i32,
    pub decrypt_failed_count: i32,
    pub repush_count: i32,
    pub missing_key_quarantined_count: i32,
    pub corruption_quarantined_count: i32,
    pub resolved_quarantine_count: i32,
    pub upgrade_required: bool,
}

pub fn get_local_time_zone() -> Result<String, BridgeErrorDto> {
    client_result(client()?.local_time_zone())
}

/// Initializes Taskveil core for the process using `db_dir`.
///
/// This creates or loads a platform Device Key, derives the SQLCipher key,
/// initializes `<db_dir>/taskveil.db`, and stores the process-global client
/// profile. Reinitializing with the same DB path succeeds idempotently;
/// reinitializing with a different DB path returns an error.
pub fn init_core(db_dir: String, default_inbox_name: String) -> Result<(), BridgeErrorDto> {
    init_client(db_dir, default_inbox_name).map_err(Into::into)
}

/// Rotates the local Device Key and SQLCipher key using the crash-recovery
/// capsule protocol. No key material crosses the Flutter bridge.
pub fn rotate_device_key() -> Result<i64, BridgeErrorDto> {
    client()?
        .rotate_device_key()
        .and_then(|generation| i64::try_from(generation).map_err(|_| ClientError::LocalKeyState))
        .map_err(Into::into)
}

pub fn get_sync_server_url() -> Result<String, BridgeErrorDto> {
    client_result(client()?.sync_server_url())
}

pub fn set_sync_server_url(server_url: String) -> Result<(), BridgeErrorDto> {
    client_result(client()?.set_sync_server_url(server_url))
}

pub fn get_account_session_state() -> Result<AccountSessionStateDto, BridgeErrorDto> {
    client_result(client()?.account_session_state()).map(account_session_to_dto)
}

fn account_registration_pending_to_dto(
    pending: AccountRegistrationPending,
) -> AccountRegistrationPendingDto {
    AccountRegistrationPendingDto {
        email: pending.email,
        expires_at_ms: pending.expires_at_ms,
        next_retry_at_ms: pending.next_retry_at_ms,
    }
}

fn account_registration_state_to_dto(
    state: AccountRegistrationState,
) -> AccountRegistrationStateDto {
    AccountRegistrationStateDto {
        phase: match state.phase {
            AccountRegistrationPhase::Email => "email",
            AccountRegistrationPhase::Otp => "otp",
            AccountRegistrationPhase::Password => "password",
        }
        .to_string(),
        email: state.email,
        expires_at_ms: state.expires_at_ms,
        next_retry_at_ms: state.next_retry_at_ms,
        can_cancel: state.can_cancel,
    }
}

pub fn account_registration_state() -> Result<Option<AccountRegistrationStateDto>, BridgeErrorDto> {
    client_result(client()?.account_registration_state())
        .map(|state| state.map(account_registration_state_to_dto))
}

pub fn account_registration_cancel() -> Result<(), BridgeErrorDto> {
    client_result(client()?.account_registration_cancel())
}

pub async fn account_registration_begin(
    email: String,
    server_url: Option<String>,
) -> Result<AccountRegistrationPendingDto, BridgeErrorDto> {
    let client = client()?;
    if let Some(server_url) = server_url {
        client
            .set_sync_server_url(server_url)
            .map_err(BridgeErrorDto::from)?;
    }
    client
        .account_registration_begin(email)
        .await
        .map_err(BridgeErrorDto::from)
        .map(account_registration_pending_to_dto)
}

pub async fn account_registration_resend(
) -> Result<AccountRegistrationPendingDto, BridgeErrorDto> {
    client()?
        .account_registration_resend()
        .await
        .map_err(BridgeErrorDto::from)
        .map(account_registration_pending_to_dto)
}

pub async fn account_registration_verify_otp(otp: String) -> Result<(), BridgeErrorDto> {
    client()?
        .account_registration_verify_otp(otp)
        .await
        .map_err(BridgeErrorDto::from)
}

pub async fn account_registration_complete(
    password: String,
    device_name: Option<String>,
) -> Result<AccountAuthResultDto, BridgeErrorDto> {
    client()?
        .account_registration_complete(password, device_name)
        .await
        .map_err(Into::into)
        .map(account_auth_to_dto)
}

pub fn account_registration_ack_recovery_key() -> Result<(), BridgeErrorDto> {
    client_result(client()?.account_registration_ack_recovery_key())
}

pub fn account_registration_recovery_key() -> Result<Option<String>, BridgeErrorDto> {
    client_result(client()?.account_registration_recovery_key())
}

pub async fn account_login(
    email: String,
    password: String,
    server_url: Option<String>,
    device_name: Option<String>,
) -> Result<AccountAuthResultDto, BridgeErrorDto> {
    client()?
        .account_login(email, password, server_url, device_name)
        .await
        .map_err(Into::into)
        .map(account_auth_to_dto)
}

pub async fn account_logout() -> Result<(), BridgeErrorDto> {
    client()?.account_logout().await.map_err(Into::into)
}

pub async fn organization_safety_number(
    tenant_id: String,
    member_user_id: String,
) -> Result<OrganizationSafetyStateDto, BridgeErrorDto> {
    client()?
        .organization_safety_number(tenant_id, member_user_id)
        .await
        .map_err(Into::into)
        .map(organization_safety_to_dto)
}

pub async fn confirm_organization_safety_number(
    tenant_id: String,
    member_user_id: String,
    digest: String,
) -> Result<OrganizationSafetyStateDto, BridgeErrorDto> {
    client()?
        .confirm_organization_safety_number(tenant_id, member_user_id, digest)
        .await
        .map_err(Into::into)
        .map(organization_safety_to_dto)
}

pub fn get_sync_status() -> Result<SyncStatusDto, BridgeErrorDto> {
    client_result(client()?.sync_status()).map(sync_status_to_dto)
}

pub async fn sync_now() -> Result<SyncStatusDto, BridgeErrorDto> {
    client()?
        .sync_now()
        .await
        .map_err(Into::into)
        .map(sync_status_to_dto)
}

pub async fn sync_now_outcome() -> Result<SyncNowOutcomeDto, BridgeErrorDto> {
    match client()?.sync_now().await {
        Ok(status) => Ok(SyncNowOutcomeDto::Synced {
            status: sync_status_to_dto(status),
        }),
        Err(ClientError::EntitlementRequired) => Ok(SyncNowOutcomeDto::BillingRequired),
        Err(error) => Err(error.into()),
    }
}

pub async fn billing_bootstrap() -> Result<BillingStateDto, BridgeErrorDto> {
    client()?
        .billing_bootstrap()
        .await
        .map(billing_state_to_dto)
        .map_err(Into::into)
}

pub async fn refresh_billing() -> Result<BillingStateDto, BridgeErrorDto> {
    client()?
        .refresh_billing()
        .await
        .map(billing_state_to_dto)
        .map_err(Into::into)
}

pub fn get_cached_billing() -> Result<Option<BillingStateDto>, BridgeErrorDto> {
    client_result(client()?.cached_billing()).map(|state| state.map(billing_state_to_dto))
}

pub async fn get_realtime_ticket() -> Result<RealtimeTicketDto, BridgeErrorDto> {
    client()?
        .realtime_ticket()
        .await
        .map_err(Into::into)
        .map(realtime_ticket_to_dto)
}

/// Creates a list using a client-owned fractional `sort_order`.
///
/// `sort_order` remains in the FRB contract for compatibility, but rank
/// generation and rebalance are owned by `TaskveilClient`.
pub fn create_list(name: String, sort_order: String) -> Result<ListDto, BridgeErrorDto> {
    let _legacy_caller_rank = sort_order;
    client_result(client()?.create_list(name)).map(list_to_dto)
}

pub fn get_lists() -> Result<Vec<ListDto>, BridgeErrorDto> {
    client_result(client()?.get_lists()).map(|lists| lists.into_iter().map(list_to_dto).collect())
}

pub fn get_archived_lists() -> Result<Vec<ListDto>, BridgeErrorDto> {
    client_result(client()?.get_archived_lists())
        .map(|lists| lists.into_iter().map(list_to_dto).collect())
}

pub fn get_templates() -> Result<Vec<TemplateDto>, BridgeErrorDto> {
    client_result(client()?.get_templates())
        .map(|templates| templates.into_iter().map(template_to_dto).collect())
}

pub fn get_task_series() -> Result<Vec<TaskSeriesDto>, BridgeErrorDto> {
    client_result(client()?.get_task_series())
        .map(|series| series.into_iter().map(task_series_to_dto).collect())
}

pub fn validate_recurrence_rule(
    rrule: String,
    starts_at: i64,
    time_zone: String,
) -> Result<String, BridgeErrorDto> {
    client_result(client()?.validate_recurrence_rule(rrule, starts_at, time_zone))
}

pub fn save_task_as_template(
    task_id: String,
    name: String,
    default_list_id: Option<String>,
) -> Result<TemplateDto, BridgeErrorDto> {
    let command = SaveTemplateCommand {
        task_id: parse_uuid(&task_id)?,
        name,
        default_list_id: default_list_id.as_deref().map(parse_uuid).transpose()?,
    };
    client_result(client()?.save_task_as_template(command)).map(template_to_dto)
}

pub fn create_template(
    name: String,
    default_list_id: Option<String>,
    nodes: Vec<TaskBlueprintNodeDto>,
) -> Result<TemplateDto, BridgeErrorDto> {
    let command = CreateTemplateCommand {
        name,
        default_list_id: default_list_id.as_deref().map(parse_uuid).transpose()?,
        blueprint: blueprint_from_dtos(nodes)?,
    };
    client_result(client()?.create_template(command)).map(template_to_dto)
}

pub fn update_template(
    template_id: String,
    name: String,
    default_list_id: Option<String>,
    nodes: Vec<TaskBlueprintNodeDto>,
) -> Result<TemplateDto, BridgeErrorDto> {
    let command = UpdateTemplateCommand {
        template_id: parse_uuid(&template_id)?,
        name,
        default_list_id: default_list_id.as_deref().map(parse_uuid).transpose()?,
        blueprint: Some(blueprint_from_dtos(nodes)?),
    };
    client_result(client()?.update_template(command)).map(template_to_dto)
}

pub fn replace_template_blueprint(
    template_id: String,
    task_id: String,
) -> Result<TemplateDto, BridgeErrorDto> {
    let command = ReplaceTaskBlueprintCommand {
        template_id: parse_uuid(&template_id)?,
        task_id: parse_uuid(&task_id)?,
    };
    client_result(client()?.replace_template_blueprint(command)).map(template_to_dto)
}

pub fn instantiate_template(template_id: String) -> Result<Vec<TaskDto>, BridgeErrorDto> {
    client_result(client()?.instantiate_template(parse_uuid(&template_id)?))
        .map(|tasks| tasks.into_iter().map(task_to_dto).collect())
}

pub fn create_task_series_from_template(
    template_id: String,
    rrule: String,
    starts_at: i64,
    time_zone: String,
) -> Result<TaskSeriesDto, BridgeErrorDto> {
    let command = CreateTaskSeriesFromTemplateCommand {
        template_id: parse_uuid(&template_id)?,
        rrule,
        starts_at,
        time_zone,
    };
    client_result(client()?.create_task_series_from_template(command)).map(task_series_to_dto)
}

pub fn create_task_series_from_task(
    task_id: String,
    target_list_id: Option<String>,
    rrule: String,
    starts_at: i64,
    time_zone: String,
) -> Result<TaskSeriesDto, BridgeErrorDto> {
    let command = CreateTaskSeriesFromTaskCommand {
        task_id: parse_uuid(&task_id)?,
        target_list_id: target_list_id.as_deref().map(parse_uuid).transpose()?,
        rrule,
        starts_at,
        time_zone,
    };
    client_result(client()?.create_task_series_from_task(command)).map(task_series_to_dto)
}

#[allow(clippy::too_many_arguments)]
pub fn update_task_series(
    series_id: String,
    target_list_id: Option<String>,
    nodes: Vec<TaskBlueprintNodeDto>,
    rrule: String,
    starts_at: i64,
    time_zone: String,
    enabled: bool,
) -> Result<TaskSeriesDto, BridgeErrorDto> {
    let command = UpdateTaskSeriesCommand {
        series_id: parse_uuid(&series_id)?,
        blueprint: blueprint_from_dtos(nodes)?,
        target_list_id: target_list_id.as_deref().map(parse_uuid).transpose()?,
        rrule,
        starts_at,
        time_zone,
        enabled,
    };
    client_result(client()?.update_task_series(command)).map(task_series_to_dto)
}

pub fn delete_task_series(series_id: String) -> Result<(), BridgeErrorDto> {
    client_result(client()?.delete_series(parse_uuid(&series_id)?))
}

pub fn delete_template(template_id: String) -> Result<(), BridgeErrorDto> {
    client_result(client()?.delete_template(parse_uuid(&template_id)?))
}

pub fn settle_due_series(at_ms: i64) -> Result<SettlementSummaryDto, BridgeErrorDto> {
    client_result(client()?.settle_due_series(at_ms)).map(settlement_to_dto)
}

pub fn get_task_series_streak(series_id: String, at_ms: i64) -> Result<StreakDto, BridgeErrorDto> {
    client_result(client()?.get_series_streak(parse_uuid(&series_id)?, at_ms)).map(streak_to_dto)
}

pub fn rename_list(list_id: String, name: String) -> Result<ListDto, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.rename_list(list_id, name)).map(list_to_dto)
}

pub fn archive_list(list_id: String) -> Result<ListDto, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.archive_list(list_id)).map(list_to_dto)
}

pub fn unarchive_list(list_id: String) -> Result<ListDto, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.unarchive_list(list_id)).map(list_to_dto)
}

/// Creates a task at the end of its sibling group using a client-generated
/// fractional `sort_order`.
#[allow(clippy::too_many_arguments)] // FRB exposes the complete atomic create command.
pub fn create_task(
    list_id: String,
    title: String,
    parent_task_id: Option<String>,
    due: Option<TaskDueInput>,
    note: Option<String>,
    priority: Option<i32>,
    scheduled_at: Option<i64>,
    estimated_minutes: Option<i32>,
) -> Result<TaskDto, BridgeErrorDto> {
    let command = CreateTaskCommand {
        list_id: parse_uuid(&list_id)?,
        title,
        parent_task_id: parent_task_id.as_deref().map(parse_uuid).transpose()?,
        due: due.map(parse_task_due).transpose()?,
        note,
        priority: priority.unwrap_or(0),
        scheduled_at,
        estimated_minutes,
    };
    client_result(client()?.create_task(command)).map(task_to_dto)
}

pub fn reorder_task(
    task_id: String,
    previous_task_id: Option<String>,
    next_task_id: Option<String>,
) -> Result<TaskDto, BridgeErrorDto> {
    let command = ReorderTaskCommand {
        task_id: parse_uuid(&task_id)?,
        previous_task_id: previous_task_id.as_deref().map(parse_uuid).transpose()?,
        next_task_id: next_task_id.as_deref().map(parse_uuid).transpose()?,
    };
    client_result(client()?.reorder_task(command)).map(task_to_dto)
}

pub fn get_tasks(list_id: String) -> Result<Vec<TaskDto>, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.get_tasks(list_id))
        .map(|tasks| tasks.into_iter().map(task_to_dto).collect())
}

pub fn get_active_timer_session() -> Result<Option<ActiveTimerSessionDto>, BridgeErrorDto> {
    client_result(client()?.get_active_timer_session())
        .map(|session| session.map(active_timer_to_dto))
}

pub fn start_active_timer_session(
    session: ActiveTimerSessionDto,
) -> Result<ActiveTimerStartOutcomeDto, BridgeErrorDto> {
    match client()?.start_active_timer_session(parse_active_timer(session)?) {
        Ok(()) => Ok(ActiveTimerStartOutcomeDto::Started),
        Err(ClientError::ActiveTimerConflict(_)) => Ok(ActiveTimerStartOutcomeDto::Conflict),
        Err(error) => Err(error.into()),
    }
}

pub fn update_active_timer_session(session: ActiveTimerSessionDto) -> Result<(), BridgeErrorDto> {
    client_result(client()?.update_active_timer_session(parse_active_timer(session)?))
}

pub fn pomodoro_target_reached_at(
    session: ActiveTimerSessionDto,
) -> Result<DateTime<Utc>, BridgeErrorDto> {
    let reached_at = domain_pomodoro_target_reached_at(&parse_active_timer(session)?)
        .map_err(|_| BridgeErrorDto::invalid_input())?;
    DateTime::<Utc>::from_timestamp_millis(reached_at).ok_or_else(BridgeErrorDto::invalid_input)
}

pub fn discard_active_timer_session(expected_session_id: String) -> Result<bool, BridgeErrorDto> {
    client_result(client()?.discard_active_timer_session(parse_uuid(&expected_session_id)?))
}

pub fn finish_active_timer_session(
    session: CompletedTimerSessionDto,
) -> Result<bool, BridgeErrorDto> {
    client_result(client()?.finish_active_timer_session(parse_completed_timer(session)?))
}

pub fn get_completed_timer_sessions(
    task_id: String,
) -> Result<Vec<CompletedTimerSessionDto>, BridgeErrorDto> {
    client_result(client()?.get_completed_timer_sessions(parse_uuid(&task_id)?))
        .map(|sessions| sessions.into_iter().map(completed_timer_to_dto).collect())
}

pub fn search_tasks(query: String) -> Result<Vec<TaskDto>, BridgeErrorDto> {
    client_result(client()?.search_tasks(&query))
        .map(|tasks| tasks.into_iter().map(task_to_dto).collect())
}

pub fn get_home_tasks(
    today_start_ms: i64,
    tomorrow_start_ms: i64,
) -> Result<Vec<HomeTaskDto>, BridgeErrorDto> {
    client_result(client()?.get_home_tasks(today_start_ms, tomorrow_start_ms))
        .map(|tasks| tasks.into_iter().map(home_task_to_dto).collect())
}

pub fn get_calendar_occurrences(
    range: CalendarRangeInput,
) -> Result<Vec<CalendarOccurrenceDto>, BridgeErrorDto> {
    let range = CalendarRange::new(
        CivilDate::parse(range.start_on).map_err(|_| BridgeErrorDto::invalid_input())?,
        CivilDate::parse(range.end_on).map_err(|_| BridgeErrorDto::invalid_input())?,
        UtcInstant::from_millis(range.start_at.timestamp_millis())
            .map_err(|_| BridgeErrorDto::invalid_input())?,
        UtcInstant::from_millis(range.end_at.timestamp_millis())
            .map_err(|_| BridgeErrorDto::invalid_input())?,
    )
    .map_err(|_| BridgeErrorDto::invalid_input())?;
    client_result(client()?.get_calendar_occurrences(range)).map(|occurrences| {
        occurrences
            .into_iter()
            .map(calendar_occurrence_to_dto)
            .collect()
    })
}

pub fn count_task_descendants(task_id: String) -> Result<i32, BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.count_task_descendants(task_id)).and_then(count_to_i32)
}

pub fn count_tasks_in_list(list_id: String) -> Result<i32, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.count_tasks_in_list(list_id)).and_then(count_to_i32)
}

pub fn update_task(
    task_id: String,
    title: String,
    note: String,
    priority: i32,
    due: Option<TaskDueInput>,
    scheduled_at: Option<i64>,
    estimated_minutes: Option<i32>,
) -> Result<TaskDto, BridgeErrorDto> {
    let command = UpdateTaskCommand {
        task_id: parse_uuid(&task_id)?,
        title,
        note,
        priority,
        due: due.map(parse_task_due).transpose()?,
        scheduled_at,
        estimated_minutes,
    };
    client_result(client()?.update_task(command)).map(task_to_dto)
}

pub fn set_task_status(
    task_id: String,
    status: String,
    closed_reason: Option<String>,
) -> Result<TaskDto, BridgeErrorDto> {
    let command = SetTaskStatusCommand {
        task_id: parse_uuid(&task_id)?,
        status: parse_status(&status)?,
        closed_reason,
    };
    client_result(client()?.set_task_status(command)).map(task_to_dto)
}

pub fn delete_task(task_id: String) -> Result<(), BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.delete_task(task_id))
}

pub fn delete_list(list_id: String) -> Result<(), BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.delete_list(list_id))
}

pub fn get_latest_task_undo() -> Result<Option<TaskUndoDto>, BridgeErrorDto> {
    client_result(client()?.get_latest_task_undo()).map(|entry| entry.map(task_undo_to_dto))
}

pub fn undo_task_operation(undo_id: String) -> Result<TaskDto, BridgeErrorDto> {
    let undo_id = parse_uuid(&undo_id)?;
    client_result(client()?.undo_task_operation(undo_id)).map(task_to_dto)
}

pub fn get_frontend_setting(key: FrontendSettingKeyDto) -> Result<Option<String>, BridgeErrorDto> {
    client_result(client()?.get_frontend_setting(key.into()))
}

pub fn set_frontend_setting(
    key: FrontendSettingKeyDto,
    value: String,
) -> Result<(), BridgeErrorDto> {
    client_result(client()?.set_frontend_setting(key.into(), &value))
}

pub fn create_task_reminder(
    task_id: String,
    remind_at: i64,
) -> Result<ReminderDto, BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.create_task_reminder(task_id, remind_at)).map(reminder_to_dto)
}

pub fn update_reminder(reminder_id: String, remind_at: i64) -> Result<ReminderDto, BridgeErrorDto> {
    let reminder_id = parse_uuid(&reminder_id)?;
    client_result(client()?.update_reminder(reminder_id, remind_at)).map(reminder_to_dto)
}

pub fn delete_reminder(reminder_id: String) -> Result<ReminderDto, BridgeErrorDto> {
    let reminder_id = parse_uuid(&reminder_id)?;
    client_result(client()?.delete_reminder(reminder_id)).map(reminder_to_dto)
}

pub fn clear_task_reminders(task_id: String) -> Result<Vec<ReminderDto>, BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.clear_task_reminders(task_id))
        .map(|reminders| reminders.into_iter().map(reminder_to_dto).collect())
}

pub fn get_task_reminders(task_id: String) -> Result<Vec<ReminderDto>, BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.get_task_reminders(task_id))
        .map(|reminders| reminders.into_iter().map(reminder_to_dto).collect())
}

pub fn get_task_subtree_reminders(task_id: String) -> Result<Vec<ReminderDto>, BridgeErrorDto> {
    let task_id = parse_uuid(&task_id)?;
    client_result(client()?.get_task_subtree_reminders(task_id))
        .map(|reminders| reminders.into_iter().map(reminder_to_dto).collect())
}

pub fn get_list_reminders(list_id: String) -> Result<Vec<ReminderDto>, BridgeErrorDto> {
    let list_id = parse_uuid(&list_id)?;
    client_result(client()?.get_list_reminders(list_id))
        .map(|reminders| reminders.into_iter().map(reminder_to_dto).collect())
}

pub fn list_pending_reminders(now_ms: i64) -> Result<Vec<ReminderDto>, BridgeErrorDto> {
    client_result(client()?.list_pending_reminders(now_ms))
        .map(|reminders| reminders.into_iter().map(reminder_to_dto).collect())
}

pub fn snooze_reminder(
    reminder_id: String,
    snoozed_until: i64,
) -> Result<ReminderDto, BridgeErrorDto> {
    let reminder_id = parse_uuid(&reminder_id)?;
    client_result(client()?.snooze_reminder(reminder_id, snoozed_until)).map(reminder_to_dto)
}

pub fn prepare_reminder_notification_reconciliation(
    now_ms: i64,
) -> Result<Vec<ReminderNotificationCommandDto>, BridgeErrorDto> {
    client_result(client()?.prepare_reminder_notification_reconciliation(now_ms)).map(|commands| {
        commands
            .into_iter()
            .map(reminder_notification_command_to_dto)
            .collect()
    })
}

pub fn list_reminder_notification_commands(
    now_ms: i64,
    limit: u32,
) -> Result<Vec<ReminderNotificationCommandDto>, BridgeErrorDto> {
    let limit = usize::try_from(limit).map_err(|_| BridgeErrorDto::invalid_input())?;
    client_result(client()?.list_reminder_notification_commands(now_ms, limit)).map(|commands| {
        commands
            .into_iter()
            .map(reminder_notification_command_to_dto)
            .collect()
    })
}

pub fn ack_reminder_notification_command(
    reminder_id: String,
    revision: i64,
) -> Result<bool, BridgeErrorDto> {
    let reminder_id = parse_uuid(&reminder_id)?;
    client_result(client()?.ack_reminder_notification_command(reminder_id, revision))
}

fn reminder_notification_command_to_dto(
    command: ReminderNotificationCommandView,
) -> ReminderNotificationCommandDto {
    ReminderNotificationCommandDto {
        reminder_id: command.reminder_id.to_string(),
        platform_id: command.platform_id,
        revision: command.revision,
        action: match command.action {
            ReminderNotificationActionView::Schedule => ReminderNotificationActionDto::Schedule,
            ReminderNotificationActionView::Cancel => ReminderNotificationActionDto::Cancel,
        },
        task_id: command.task_id.map(|value| value.to_string()),
        list_id: command.list_id.map(|value| value.to_string()),
        scheduled_at: command.scheduled_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_error_details_are_reduced_to_allowlisted_codes() {
        let cases = [
            (
                ClientError::Unauthorized,
                BridgeErrorCodeDto::Unauthorized,
                false,
            ),
            (
                ClientError::AccountRequest,
                BridgeErrorCodeDto::Internal,
                false,
            ),
            (
                ClientError::CredentialUnavailable,
                BridgeErrorCodeDto::CredentialUnavailable,
                false,
            ),
            (
                ClientError::UpgradeRequired,
                BridgeErrorCodeDto::UpgradeRequired,
                false,
            ),
            (ClientError::ProfileBusy, BridgeErrorCodeDto::Busy, true),
            (
                ClientError::LocalKeyState,
                BridgeErrorCodeDto::CryptoUnavailable,
                false,
            ),
            (
                ClientError::Io(std::io::Error::other(
                    "/private/profile/path must remain secret",
                )),
                BridgeErrorCodeDto::StorageFailure,
                false,
            ),
        ];
        for (source, code, retryable) in cases {
            let mapped = BridgeErrorDto::from(source);
            assert_eq!(mapped.code, code);
            assert_eq!(mapped.retryable, retryable);
            assert!(mapped.arguments.is_empty());
        }
    }

    #[test]
    fn calendar_bridge_rejects_invalid_or_reversed_half_open_ranges_before_client_access() {
        let start = DateTime::<Utc>::from_timestamp_millis(1_773_035_600_000).unwrap();
        let end = DateTime::<Utc>::from_timestamp_millis(1_773_118_400_000).unwrap();

        let invalid_date = get_calendar_occurrences(CalendarRangeInput {
            start_on: "2026-03-8".into(),
            end_on: "2026-03-09".into(),
            start_at: start,
            end_at: end,
        });
        assert!(matches!(
            invalid_date,
            Err(BridgeErrorDto {
                code: BridgeErrorCodeDto::InvalidInput,
                arguments,
                retryable: false,
            }) if arguments.is_empty()
        ));

        let reversed = get_calendar_occurrences(CalendarRangeInput {
            start_on: "2026-03-09".into(),
            end_on: "2026-03-08".into(),
            start_at: start,
            end_at: end,
        });
        assert!(matches!(
            reversed,
            Err(BridgeErrorDto {
                code: BridgeErrorCodeDto::InvalidInput,
                arguments,
                retryable: false,
            }) if arguments.is_empty()
        ));
    }
}
