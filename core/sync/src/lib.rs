//! `taskveil-sync`: HLC・差分検出・push/pull同期エンジンを提供する crate。
//!
//! 詳細は `docs/03_技術仕様書.md` §6 同期プロトコル を参照。
//!
//! `outbox` によるpush/pullフローの永続化は `taskveil-storage` が担う。

pub mod account;
pub mod apply;
pub mod engine;
pub mod enqueue;
pub mod envelope;
pub mod field_map;
pub mod hlc;
pub mod key_manifest;
pub mod keys;
pub mod merge;
pub mod organization;
pub mod protocol;
pub mod resync;
pub mod rotation;
mod secret;
mod server_origin;

pub use apply::{
    run_sync_now, run_sync_now_with_key_refresh, run_sync_now_with_key_refresh_and_pre_push,
    ActiveSyncContext, SyncKeyRefresher,
};
pub use engine::{
    BasePage, DeltaPage, EncryptedSyncState, PreflightResult, PullRecord, PushBatchOutcome, PushOp,
    PushOpOutcome, PushStatus, StableCursor, SyncEngine, SyncEngineError, SyncRunSummary,
};
pub use enqueue::{
    enqueue_backfill, enqueue_list_sync, enqueue_rotation_backfill, enqueue_task_series_sync,
    enqueue_task_sync, enqueue_template_sync, enqueue_timer_session_sync, next_local_revision,
    rebind_local_device, BackfillRecords, BackfillSummary, DeviceRebindSummary, LocalListAlias,
    LocalMutationSyncStore, LocalSyncAtomicStore, LocalSyncOutboxEntry, LocalSyncQuarantineEntry,
    LocalSyncRecordState, LocalSyncSemanticState, LocalSyncStore, LocalSyncWriteTransaction,
    NewLocalSyncOutboxEntry, PullFailureReason,
};
pub use envelope::{
    decrypt_plaintext, encrypt_plaintext, parse_envelope_header, EnvelopeError, EnvelopeHeader,
    ENVELOPE_VERSION, MAX_ENCRYPTED_BLOB_LEN,
};
pub use field_map::{
    validate_rank, Clocked, FieldMapError, ListPlacement, ListPlaintext, SeriesCursorValue,
    SyncPlaintext, TaskBlueprintValue, TaskCompletion, TaskPlacement, TaskPlaintext,
    TaskSeriesConfigValue, TaskSeriesPlaintext, TemplatePlaintext, TimerSessionPlaintext,
    LIST_FIELD_GROUPS, TASK_FIELD_GROUPS, TASK_SERIES_FIELD_GROUPS, TEMPLATE_FIELD_GROUPS,
};
pub use hlc::{Hlc, HlcError};
pub use key_manifest::{
    derive_personal_manifest_auth_key, KeyManifest, KeyManifestError, RotationStatus,
    MIN_AUTHENTICATED_MANIFEST_LEN, PERSONAL_MANIFEST_AUTH_INFO,
};
pub use keys::{
    tenant_root_dek, tenant_root_dek_for_generation, LocalSyncKeys,
    KEY_ROTATION_PENDING_METADATA_KEY, LISTS_COLLECTION, SYNC_CURSOR_NAME,
    SYNC_FULL_RESYNC_COMPLETION_TOKEN_METADATA_KEY, SYNC_FULL_RESYNC_PAGE_TOKEN_METADATA_KEY,
    SYNC_LOCAL_HLC_METADATA_KEY, SYNC_UPGRADE_REQUIRED_METADATA_KEY, TASKS_COLLECTION,
    TASK_SERIES_COLLECTION, TEMPLATES_COLLECTION, TIMER_SESSIONS_COLLECTION,
};
pub use merge::{merge_lww, MergeResult};
pub use protocol::SyncCollection;
pub use resync::{delta_reached_closure, full_resync_reason, FullResyncReason};
pub use rotation::{DeviceContinuity, RotationCoordinator, RotationError, HISTORY_RETENTION_DAYS};
pub use secret::SecretString;
pub use server_origin::{canonical_server_origin, ServerOriginError};

#[cfg(test)]
mod convergence_tests {
    use crate::{merge_lww, Hlc, SyncPlaintext};
    use proptest::{prelude::*, test_runner::Config};
    use taskveil_domain::{new_task, Uuid};

    proptest! {
        #![proptest_config(Config::with_cases(64))]

        #[test]
        fn distinct_field_merge_is_commutative_and_idempotent(
            title in ".{0,64}", note in ".{0,128}", a_counter in 1u32..1000, b_counter in 1u32..1000,
        ) {
            let mut task = new_task(Uuid::now_v7(), None, "base".into(), "7fffffffffffffffffffffffffffffff".into(), 1).unwrap();
            let base = SyncPlaintext::from_task(&task, h(0,"base")).unwrap();
            task.content.title = title.clone();
            let a = base.stamp_task_changes(&task, h(a_counter,"a")).unwrap();
            task.content.title = "base".into(); task.content.note = note.clone();
            let b = base.stamp_task_changes(&task, h(b_counter,"b")).unwrap();
            let ab = merge_lww(&a,&b).unwrap().plaintext;
            let ba = merge_lww(&b,&a).unwrap().plaintext;
            prop_assert_eq!(&ab,&ba);
            prop_assert_eq!(merge_lww(&ab,&ab).unwrap().plaintext,ab.clone());
            let SyncPlaintext::Task(value)=ab else { unreachable!() };
            prop_assert_eq!(value.title.value,title);
            prop_assert_eq!(value.note.value,note);
        }
    }
    fn h(counter: u32, device: &str) -> Hlc {
        Hlc {
            wall_ms: 1,
            counter,
            device_id: device.into(),
        }
    }
}
