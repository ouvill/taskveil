use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use taskveil_domain::{List, Task, TaskContent, TaskSeries, TaskSeriesConfig, TaskTemplate, Uuid};

use crate::{
    account::{AccountClient, AccountClientError},
    decrypt_plaintext, merge_lww, EncryptedSyncState, EnvelopeError, Hlc, PullRecord, PushOp,
    PushStatus, SyncCollection, SyncEngine, SyncEngineError, SyncPlaintext, SyncRunSummary,
    LISTS_COLLECTION, SYNC_CURSOR_NAME, SYNC_FULL_RESYNC_COMPLETION_TOKEN_SETTING_KEY,
    SYNC_FULL_RESYNC_PAGE_TOKEN_SETTING_KEY, SYNC_UPGRADE_REQUIRED_SETTING_KEY, TASKS_COLLECTION,
    TASK_SERIES_COLLECTION, TEMPLATES_COLLECTION,
};

use crate::enqueue::{
    enqueue_merged_plaintext, enqueue_rebased_tombstone, enqueue_task_sync,
    enqueue_timer_session_sync, list_plaintext, observe_remote_hlc, rebind_local_device,
    task_plaintext, task_series_plaintext, template_plaintext, LocalFullResyncPhase,
    LocalListAlias, LocalMutationSyncStore, LocalSyncAtomicStore, LocalSyncQuarantineEntry,
    LocalSyncRecordState, LocalSyncSemanticState, LocalSyncStore, LocalSyncWriteTransaction,
    PullFailureReason, RebasePlaintextRequest, RebaseTombstoneRequest,
};
use crate::keys::{tenant_root_dek, LocalSyncKeys};

mod decode;
mod orchestration;
mod pull;
mod records;
mod resync;

use decode::*;
#[cfg(test)]
use orchestration::{
    manifest_anchor_key, reconcile_canonical_inbox_in_transaction, verify_manifest_anchor,
};
pub use orchestration::{
    run_sync_now, run_sync_now_with_key_refresh, run_sync_now_with_key_refresh_and_pre_push,
    ActiveSyncContext, SyncKeyRefresher,
};
use pull::*;
use records::*;
use resync::*;

const PUSH_BATCH_LIMIT: usize = 100;
const MAX_PUSH_DRAIN_ITERATIONS: usize = 100;
const QUARANTINE_REPLAY_BATCH_LIMIT: usize = 100;
const FULL_RESYNC_PAGE_LIMIT: i64 = 100;
const FULL_RESYNC_SWEEP_BATCH_LIMIT: usize = 100;

fn sync_engine_error_to_string(error: SyncEngineError) -> String {
    match error {
        SyncEngineError::Server(status) if status == reqwest::StatusCode::UNAUTHORIZED => {
            "unauthorized".to_string()
        }
        SyncEngineError::EntitlementRequired => "entitlement required".to_string(),
        SyncEngineError::UpgradeRequired { .. } => "upgrade required".to_string(),
        SyncEngineError::ClockSkewRetryable => "clock skew retryable".to_string(),
        _ => "sync failed".to_string(),
    }
}

fn account_sync_error_to_string(error: AccountClientError) -> String {
    match error {
        AccountClientError::Server(401) => "unauthorized".to_string(),
        AccountClientError::EntitlementRequired => "entitlement required".to_string(),
        _ => "sync failed".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyDisposition {
    AppliedCurrent,
    Rebased,
    Deferred(PullFailureReason, Option<Uuid>),
    UpgradeRequired(u8),
}

enum TaskDependencyDisposition {
    Valid,
    Missing,
    Deleted,
}

#[cfg(test)]
#[path = "apply/tests.rs"]
mod tests;
