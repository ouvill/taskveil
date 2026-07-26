use crate::*;

/// Undo対象のタスク操作種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskUndoOperation {
    Delete,
    Complete,
    Edit,
}

/// ローカル専用のタスクUndo履歴。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskUndoEntry {
    pub id: Uuid,
    pub operation_type: TaskUndoOperation,
    pub task_id: Uuid,
    pub list_id: Uuid,
    pub before_snapshot: Task,
    pub after_updated_at: i64,
    pub after_deleted_at: Option<i64>,
    pub after_completed_at: Option<i64>,
    pub created_at: i64,
    pub consumed_at: Option<i64>,
}

/// A task returned by the cross-list Home smart view, annotated with its
/// containing list name for UI context.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeTask {
    pub task: Task,
    pub list_name: String,
    pub is_home_target: bool,
}

/// Viewer-local calendar bounds represented without collapsing civil dates
/// into synthetic instants. Both dimensions use half-open `[start, end)`
/// intervals and are constructed by the caller from the same viewer timezone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRange {
    start_on: CivilDate,
    end_on: CivilDate,
    start_at: UtcInstant,
    end_at: UtcInstant,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CalendarRangeError {
    #[error("calendar civil-date range must be non-empty and increasing")]
    InvalidCivilDateRange,
    #[error("calendar instant range must be non-empty and increasing")]
    InvalidInstantRange,
}

impl CalendarRange {
    pub fn new(
        start_on: CivilDate,
        end_on: CivilDate,
        start_at: UtcInstant,
        end_at: UtcInstant,
    ) -> Result<Self, CalendarRangeError> {
        if start_on >= end_on {
            return Err(CalendarRangeError::InvalidCivilDateRange);
        }
        if start_at >= end_at {
            return Err(CalendarRangeError::InvalidInstantRange);
        }
        Ok(Self {
            start_on,
            end_on,
            start_at,
            end_at,
        })
    }

    pub fn start_on(&self) -> &CivilDate {
        &self.start_on
    }

    pub fn end_on(&self) -> &CivilDate {
        &self.end_on
    }

    pub fn start_at(&self) -> UtcInstant {
        self.start_at
    }

    pub fn end_at(&self) -> UtcInstant {
        self.end_at
    }
}

/// The semantic reason a task appears in a calendar range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarOccurrenceKind {
    DateDue {
        due_on: CivilDate,
    },
    DateTimeDue {
        due_at: UtcInstant,
        time_zone: IanaTimeZone,
    },
    Scheduled {
        scheduled_at: UtcInstant,
    },
    Completed {
        completed_at: UtcInstant,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarOccurrence {
    pub task: Task,
    pub list_name: String,
    pub list_archived: bool,
    pub kind: CalendarOccurrenceKind,
}

/// A local reminder scheduled on the device for a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub id: Uuid,
    pub task_id: Uuid,
    pub remind_at: i64,
    pub snoozed_until: Option<i64>,
    pub created_at: i64,
}

pub const MAX_REMINDERS_PER_TASK: usize = 5;

/// Desired operation for the OS-local reminder notification projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderNotificationAction {
    Schedule,
    Cancel,
}

/// A durable reminder notification command with schedule context loaded by one
/// joined query. Schedule commands always contain task/list/time fields;
/// cancel commands intentionally require only the stable platform ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderNotificationCommand {
    pub reminder_id: Uuid,
    pub platform_id: i32,
    pub revision: i64,
    pub action: ReminderNotificationAction,
    pub task_id: Option<Uuid>,
    pub list_id: Option<Uuid>,
    pub scheduled_at: Option<i64>,
}

/// 未ACKのrecord headに保持する暗号化済みsemantic state。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutboxState {
    Live { mutation_hlc: String, blob: Vec<u8> },
    Tombstone { delete_hlc: String },
}

/// recordごとにcoalesceされた未ACKのpush head。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: String,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSyncOutboxEntry {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: String,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub created_at: i64,
}

/// 復号・mergeに使うlocal semantic state。tombstoneは平文を保持しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncRecordSemanticState {
    Live {
        mutation_hlc: String,
        plaintext_json: String,
    },
    Tombstone {
        delete_hlc: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRecordState {
    pub record_id: Uuid,
    pub collection: String,
    pub current_revision_hlc: Option<String>,
    pub state: SyncRecordSemanticState,
    pub updated_at: i64,
}

/// Device-local mapping from a non-canonical Inbox record to the current
/// canonical Inbox. Both list sync records remain independently durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListAlias {
    pub alias_list_id: Uuid,
    pub canonical_list_id: Uuid,
    pub updated_at: i64,
}

/// テナントDB内のpull cursor。ローカルDBはテナントごとに分離する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursor {
    pub name: String,
    pub seq: i64,
    pub updated_at: i64,
}

/// Durable phase of a fuzzy-scan full resync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullResyncPhase {
    Base,
    Delta,
    Sweep,
}

/// Stable-key cursor used by the current-state base scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullResyncStableCursor {
    pub collection: String,
    pub record_id: Uuid,
}

/// Crash-recoverable progress for the one active full resync generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullResyncProgress {
    pub generation_id: Uuid,
    pub continuity_generation: i64,
    pub phase: FullResyncPhase,
    pub base_seq: i64,
    pub base_cursor: Option<FullResyncStableCursor>,
    pub delta_cursor: i64,
    pub closure_high_water: Option<i64>,
    pub sweep_cursor: Option<FullResyncStableCursor>,
    pub started_at: i64,
    pub updated_at: i64,
}

/// Rows removed when a closed generation is finalized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FullResyncSweepSummary {
    pub scanned_records: usize,
    pub swept_lists: usize,
    pub swept_tasks: usize,
    pub swept_templates: usize,
    pub swept_task_series: usize,
    pub swept_timer_sessions: usize,
    pub swept_record_states: usize,
}

/// An encrypted remote head that could not yet be safely applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncQuarantineEntry {
    pub record_id: Uuid,
    pub collection: String,
    pub seq: i64,
    pub revision_hlc: String,
    pub state: SyncOutboxState,
    pub reason: String,
    pub required_list_id: Option<Uuid>,
    pub first_failed_at: i64,
    pub last_failed_at: i64,
    pub attempt_count: i64,
}

/// SQLCipher内に保持するaccount-bound local profile identity。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProfileBinding {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub bound_at: i64,
    pub updated_at: i64,
}

/// Master Keyでlocal-wrap済みのTenant Root DEK cache。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTenantRootKeyBundle {
    pub tenant_id: Uuid,
    pub generation: u64,
    pub wrapped_tenant_root_dek: Vec<u8>,
    pub updated_at: i64,
}
