use std::fmt::Write as _;

use super::*;

pub(super) fn client_result<T>(
    result: Result<T, taskveil_client::ClientError>,
) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

pub(super) fn parse_uuid(value: &str) -> Result<Uuid, String> {
    value.parse::<Uuid>().map_err(|error| error.to_string())
}

pub(super) fn parse_status(value: &str) -> Result<TaskStatus, String> {
    match value {
        "todo" => Ok(TaskStatus::Todo),
        "in_progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        "wont_do" => Ok(TaskStatus::WontDo),
        other => Err(format!("invalid task status: {other}")),
    }
}

pub(super) fn parse_task_due(input: TaskDueInput) -> Result<TaskDue, String> {
    match input {
        TaskDueInput::Date { due_on } => {
            TaskDue::date(due_on).map_err(|_| "invalid date-only due value".to_string())
        }
        TaskDueInput::DateTime { due_at, time_zone } => {
            TaskDue::date_time(due_at.timestamp_millis(), time_zone)
                .map_err(|_| "invalid datetime due value".to_string())
        }
    }
}

pub(super) fn count_to_i32(count: usize) -> Result<i32, String> {
    i32::try_from(count).map_err(|_| "count exceeds i32 range".to_string())
}

pub(super) fn status_to_string(status: TaskStatus) -> String {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::WontDo => "wont_do",
    }
    .to_string()
}

pub(super) fn list_to_dto(list: List) -> ListDto {
    ListDto {
        id: list.id.to_string(),
        name: list.name,
        color: list.color,
        icon: list.icon,
        sort_order: list.sort_order,
        is_default: list.is_default,
        archived_at: list.archived_at,
        created_at: list.created_at,
        updated_at: list.updated_at,
    }
}

pub(super) fn task_to_dto(task: Task) -> TaskDto {
    TaskDto {
        id: task.id.to_string(),
        list_id: task.list_id.to_string(),
        parent_task_id: task.parent_task_id.map(|id| id.to_string()),
        title: task.content.title,
        note: task.content.note,
        status: status_to_string(task.status),
        priority: task.content.priority,
        due: task.due.map(task_due_to_dto),
        scheduled_at: task.scheduled_at,
        estimated_minutes: task.content.estimated_minutes,
        sort_order: task.sort_order,
        completed_at: task.completed_at,
        closed_reason: task.closed_reason,
        deleted_at: task.deleted_at,
        assignee: task.assignee.map(|id| id.to_string()),
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

pub(super) fn template_to_dto(template: TaskTemplate) -> TemplateDto {
    TemplateDto {
        id: template.id.to_string(),
        name: template.name,
        default_list_id: template.default_list_id.map(|id| id.to_string()),
        blueprint_revision: template.blueprint_revision,
        nodes: template
            .blueprint
            .nodes
            .into_iter()
            .map(template_node_to_dto)
            .collect(),
        created_at: template.created_at,
        updated_at: template.updated_at,
    }
}

pub(super) fn template_node_to_dto(node: TaskBlueprintNode) -> TaskBlueprintNodeDto {
    TaskBlueprintNodeDto {
        node_key: node.node_key,
        parent_node_key: node.parent_node_key,
        sibling_order: node.sibling_order,
        title: node.content.title,
        note: node.content.note,
        priority: node.content.priority,
        estimated_minutes: node.content.estimated_minutes,
    }
}

pub(super) fn task_series_to_dto(series: TaskSeries) -> TaskSeriesDto {
    TaskSeriesDto {
        id: series.id.to_string(),
        target_list_id: series.config.target_list_id.map(|id| id.to_string()),
        nodes: series
            .config
            .blueprint
            .nodes
            .into_iter()
            .map(template_node_to_dto)
            .collect(),
        rrule: series.config.rrule,
        starts_at: series.config.starts_at,
        time_zone: series.config.time_zone,
        next_run_at: series.cursor.next_run_at(),
        enabled: series.config.enabled,
        config_revision: series.config.config_revision,
        created_at: series.created_at,
        updated_at: series.updated_at,
    }
}

pub(super) fn blueprint_from_dtos(
    nodes: Vec<TaskBlueprintNodeDto>,
) -> Result<TaskBlueprint, String> {
    let blueprint = TaskBlueprint {
        schema_revision: taskveil_client::TASK_BLUEPRINT_SCHEMA_REVISION,
        nodes: nodes
            .into_iter()
            .map(|node| TaskBlueprintNode {
                node_key: node.node_key,
                parent_node_key: node.parent_node_key,
                sibling_order: node.sibling_order,
                content: TaskContent {
                    title: node.title,
                    note: node.note,
                    priority: node.priority,
                    estimated_minutes: node.estimated_minutes,
                },
            })
            .collect(),
    };
    blueprint.validate().map_err(|error| error.to_string())?;
    Ok(blueprint)
}

pub(super) fn streak_to_dto(streak: Streak) -> StreakDto {
    StreakDto {
        current: streak.current,
        finalized: streak.finalized,
    }
}

pub(super) fn settlement_to_dto(summary: SettlementSummary) -> SettlementSummaryDto {
    SettlementSummaryDto {
        generated_occurrences: summary.generated_occurrences,
        generated_tasks: summary.generated_tasks,
        has_more: summary.has_more,
        outbox_changed: summary.outbox_changed,
    }
}

pub(super) fn task_due_to_dto(due: TaskDue) -> TaskDueDto {
    match due {
        TaskDue::Date { due_on } => TaskDueDto::Date {
            due_on: due_on.to_string(),
        },
        TaskDue::DateTime { due_at, time_zone } => TaskDueDto::DateTime {
            due_at: DateTime::<Utc>::from_timestamp_millis(due_at.as_millis())
                .expect("UtcInstant is validated at construction"),
            time_zone: time_zone.to_string(),
        },
    }
}

pub(super) fn home_task_to_dto(home_task: HomeTaskView) -> HomeTaskDto {
    HomeTaskDto {
        task: task_to_dto(home_task.task),
        list_name: home_task.list_name,
        is_home_target: home_task.is_home_target,
    }
}

pub(super) fn calendar_occurrence_to_dto(
    occurrence: CalendarOccurrenceView,
) -> CalendarOccurrenceDto {
    CalendarOccurrenceDto {
        task: task_to_dto(occurrence.task),
        list_name: occurrence.list_name,
        list_archived: occurrence.list_archived,
        kind: match occurrence.kind {
            CalendarOccurrenceKind::DateDue { due_on } => CalendarOccurrenceKindDto::DateDue {
                due_on: due_on.to_string(),
            },
            CalendarOccurrenceKind::DateTimeDue { due_at, time_zone } => {
                CalendarOccurrenceKindDto::DateTimeDue {
                    due_at: instant_to_datetime(due_at),
                    time_zone: time_zone.to_string(),
                }
            }
            CalendarOccurrenceKind::Scheduled { scheduled_at } => {
                CalendarOccurrenceKindDto::Scheduled {
                    scheduled_at: instant_to_datetime(scheduled_at),
                }
            }
            CalendarOccurrenceKind::Completed { completed_at } => {
                CalendarOccurrenceKindDto::Completed {
                    completed_at: instant_to_datetime(completed_at),
                }
            }
        },
    }
}

pub(super) fn instant_to_datetime(instant: UtcInstant) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(instant.as_millis())
        .expect("UtcInstant is validated at construction")
}

pub(super) fn parse_active_timer(
    value: ActiveTimerSessionDto,
) -> Result<ActiveTimerSession, String> {
    Ok(ActiveTimerSession {
        session_id: parse_uuid(&value.session_id)?,
        task_id: value.task_id.as_deref().map(parse_uuid).transpose()?,
        mode: parse_timer_mode(value.mode),
        phase: parse_timer_phase(value.phase),
        state: parse_timer_run_state(value.state),
        started_at: value.started_at.timestamp_millis(),
        last_resumed_at: value.last_resumed_at.map(|time| time.timestamp_millis()),
        accumulated_active_ms: value.accumulated_active_ms,
        target_duration_ms: value.target_duration_ms,
    })
}

pub(super) fn parse_completed_timer(
    value: CompletedTimerSessionDto,
) -> Result<CompletedTimerSession, String> {
    Ok(CompletedTimerSession {
        id: parse_uuid(&value.id)?,
        task_id: parse_uuid(&value.task_id)?,
        mode: parse_timer_mode(value.mode),
        finish_kind: parse_timer_finish_kind(value.finish_kind),
        started_at: value.started_at.timestamp_millis(),
        ended_at: value.ended_at.timestamp_millis(),
        active_duration_ms: value.active_duration_ms,
        created_at: value.created_at.timestamp_millis(),
    })
}

pub(super) fn active_timer_to_dto(value: ActiveTimerSession) -> ActiveTimerSessionDto {
    ActiveTimerSessionDto {
        session_id: value.session_id.to_string(),
        task_id: value.task_id.map(|id| id.to_string()),
        mode: timer_mode_to_dto(value.mode),
        phase: timer_phase_to_dto(value.phase),
        state: timer_run_state_to_dto(value.state),
        started_at: millis_to_datetime(value.started_at),
        last_resumed_at: value.last_resumed_at.map(millis_to_datetime),
        accumulated_active_ms: value.accumulated_active_ms,
        target_duration_ms: value.target_duration_ms,
    }
}

pub(super) fn completed_timer_to_dto(value: CompletedTimerSession) -> CompletedTimerSessionDto {
    CompletedTimerSessionDto {
        id: value.id.to_string(),
        task_id: value.task_id.to_string(),
        mode: timer_mode_to_dto(value.mode),
        finish_kind: timer_finish_kind_to_dto(value.finish_kind),
        started_at: millis_to_datetime(value.started_at),
        ended_at: millis_to_datetime(value.ended_at),
        active_duration_ms: value.active_duration_ms,
        created_at: millis_to_datetime(value.created_at),
    }
}

pub(super) fn millis_to_datetime(value: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(value).expect("domain timer timestamps are validated")
}

pub(super) fn parse_timer_mode(value: TimerModeDto) -> TimerMode {
    match value {
        TimerModeDto::Pomodoro => TimerMode::Pomodoro,
        TimerModeDto::Stopwatch => TimerMode::Stopwatch,
    }
}

pub(super) fn timer_mode_to_dto(value: TimerMode) -> TimerModeDto {
    match value {
        TimerMode::Pomodoro => TimerModeDto::Pomodoro,
        TimerMode::Stopwatch => TimerModeDto::Stopwatch,
    }
}

pub(super) fn parse_timer_phase(value: TimerPhaseDto) -> TimerPhase {
    match value {
        TimerPhaseDto::Work => TimerPhase::Work,
        TimerPhaseDto::ShortBreak => TimerPhase::ShortBreak,
        TimerPhaseDto::LongBreak => TimerPhase::LongBreak,
    }
}

pub(super) fn timer_phase_to_dto(value: TimerPhase) -> TimerPhaseDto {
    match value {
        TimerPhase::Work => TimerPhaseDto::Work,
        TimerPhase::ShortBreak => TimerPhaseDto::ShortBreak,
        TimerPhase::LongBreak => TimerPhaseDto::LongBreak,
    }
}

pub(super) fn parse_timer_run_state(value: TimerRunStateDto) -> TimerRunState {
    match value {
        TimerRunStateDto::Running => TimerRunState::Running,
        TimerRunStateDto::Paused => TimerRunState::Paused,
    }
}

pub(super) fn timer_run_state_to_dto(value: TimerRunState) -> TimerRunStateDto {
    match value {
        TimerRunState::Running => TimerRunStateDto::Running,
        TimerRunState::Paused => TimerRunStateDto::Paused,
    }
}

pub(super) fn parse_timer_finish_kind(value: TimerFinishKindDto) -> TimerFinishKind {
    match value {
        TimerFinishKindDto::Completed => TimerFinishKind::Completed,
        TimerFinishKindDto::Interrupted => TimerFinishKind::Interrupted,
    }
}

pub(super) fn timer_finish_kind_to_dto(value: TimerFinishKind) -> TimerFinishKindDto {
    match value {
        TimerFinishKind::Completed => TimerFinishKindDto::Completed,
        TimerFinishKind::Interrupted => TimerFinishKindDto::Interrupted,
    }
}

pub(super) fn task_undo_to_dto(entry: TaskUndoView) -> TaskUndoDto {
    TaskUndoDto {
        id: entry.id.to_string(),
        operation_type: match entry.operation {
            TaskUndoKind::Delete => "delete",
            TaskUndoKind::Complete => "complete",
            TaskUndoKind::Edit => "edit",
        }
        .to_string(),
        task_id: entry.task_id.to_string(),
        list_id: entry.list_id.to_string(),
        task_title: entry.task_title,
        created_at: entry.created_at,
    }
}

pub(super) fn reminder_to_dto(reminder: ReminderView) -> ReminderDto {
    ReminderDto {
        id: reminder.id.to_string(),
        task_id: reminder.task_id.to_string(),
        remind_at: reminder.remind_at,
        snoozed_until: reminder.snoozed_until,
        created_at: reminder.created_at,
    }
}

pub(super) fn account_session_to_dto(session: AccountSessionState) -> AccountSessionStateDto {
    AccountSessionStateDto {
        logged_in: session.logged_in,
        email: session.email,
        user_id: session.user_id,
        tenant_id: session.tenant_id,
        device_id: session.device_id,
        recovery_pending: session.recovery_pending,
    }
}

pub(super) fn realtime_ticket_to_dto(ticket: RealtimeTicket) -> RealtimeTicketDto {
    RealtimeTicketDto {
        websocket_url: ticket.websocket_url,
        ticket: ticket.ticket,
        expires_at: ticket.expires_at,
    }
}

pub(super) fn account_auth_to_dto(result: AccountAuthResult) -> AccountAuthResultDto {
    AccountAuthResultDto {
        session: account_session_to_dto(result.session),
        recovery_key: result.recovery_key,
    }
}

pub(super) fn billing_state_to_dto(state: BillingState) -> BillingStateDto {
    BillingStateDto {
        provider: state.provider,
        provider_app_user_id: state.provider_app_user_id,
        lookup_key: state.lookup_key,
        status: state.status,
        sync_allowed: state.sync_allowed,
        store_product_identifier: state.store_product_identifier,
        expires_at: state.expires_at,
        grace_expires_at: state.grace_expires_at,
        will_renew: state.will_renew,
        environment: state.environment,
        updated_at: state.updated_at,
    }
}

pub(super) fn organization_safety_to_dto(
    state: OrganizationSafetyState,
) -> OrganizationSafetyStateDto {
    OrganizationSafetyStateDto {
        owner_user_id: state.owner_user_id,
        member_user_id: state.member_user_id,
        digest: state.digest,
        decimal: state.decimal,
        qr_payload: state.qr_payload,
        verification_state: state.verification_state,
        owner_confirmed: state.owner_confirmed,
        member_confirmed: state.member_confirmed,
    }
}

pub(super) fn sync_status_to_dto(status: SyncStatus) -> SyncStatusDto {
    SyncStatusDto {
        logged_in: status.logged_in,
        running: status.running,
        last_success_at: status.last_success_at,
        last_failure_at: status.last_failure_at,
        last_error: status.last_error,
        pushed_count: saturating_i32(status.pushed_count),
        push_acked_count: saturating_i32(status.push_acked_count),
        push_superseded_count: saturating_i32(status.push_superseded_count),
        pulled_count: saturating_i32(status.pulled_count),
        applied_count: saturating_i32(status.applied_count),
        deleted_count: saturating_i32(status.deleted_count),
        decrypt_failed_count: saturating_i32(status.decrypt_failed_count),
        repush_count: saturating_i32(status.repush_count),
        missing_key_quarantined_count: saturating_i32(status.missing_key_quarantined_count),
        corruption_quarantined_count: saturating_i32(status.corruption_quarantined_count),
        resolved_quarantine_count: saturating_i32(status.resolved_quarantine_count),
        upgrade_required: status.upgrade_required,
    }
}

pub(super) fn saturating_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

pub(super) fn json_string(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\u{08}' => encoded.push_str("\\b"),
            '\u{0c}' => encoded.push_str("\\f"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            control if control <= '\u{1f}' => {
                write!(encoded, "\\u{:04x}", control as u32).expect("write to String")
            }
            other => encoded.push(other),
        }
    }
    encoded.push('"');
    encoded
}
