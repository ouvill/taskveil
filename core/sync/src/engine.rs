//! HTTP sync engine client for the strict protocol v2 wire contract.

use std::collections::{HashMap, HashSet};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::StatusCode;
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    protocol::{
        self, BaseScanRequest, BaseScanResponse, ClosureProof, CompleteBaseRequest,
        CompleteBaseResponse, ContinuityAckRequest, ContinuityAckResponse, PullResponse,
        PushRequest, SyncCollection, SyncRecordState as WireRecordState,
        SYNC_PROTOCOL_VERSION_HEADER,
    },
    Hlc, SecretString,
};

pub use crate::protocol::PushStatus;

#[derive(Debug, Error)]
pub enum SyncEngineError {
    #[error("server URL is empty")]
    EmptyServerUrl,
    #[error("server URL is not a secure origin")]
    InvalidServerOrigin,
    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("server rejected sync request: {0}")]
    Server(StatusCode),
    #[error("HLC is temporarily ahead of the server clock")]
    ClockSkewRetryable,
    #[error("durable full resync chain must be restarted")]
    ResyncRestartRequired,
    #[error("a Pro entitlement is required")]
    EntitlementRequired,
    #[error("invalid sync request")]
    InvalidRequest,
    #[error("invalid push response")]
    InvalidPushResponse,
    #[error("invalid pull response")]
    InvalidPullResponse,
    #[error("invalid preflight response")]
    InvalidPreflightResponse,
    #[error("sync client upgrade required")]
    UpgradeRequired {
        protocol_version: u16,
        envelope_version: u8,
    },
}

#[derive(Debug, Clone)]
pub struct SyncEngine {
    base_url: String,
    tenant_id: Uuid,
    session_token: SecretString,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptedSyncState {
    Live { mutation_hlc: String, blob: Vec<u8> },
    Tombstone { delete_hlc: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOp {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub base_revision_hlc: Option<String>,
    pub revision_hlc: String,
    pub state: EncryptedSyncState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushBatchOutcome {
    pub outcomes: Vec<PushOpOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOpOutcome {
    pub op_id: Uuid,
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub status: PushStatus,
    pub seq: Option<i64>,
    pub current: Option<PullRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightResult {
    pub gc_horizon_seq: i64,
    pub continuity_seq: i64,
    pub continuity_generation: i64,
    pub required_generation: i64,
    pub full_resync_required: bool,
    pub suite_id: u16,
    pub active_key_generation: u64,
    pub minimum_write_generation: u64,
    pub migrating_key_generation: Option<u64>,
    pub key_manifests: Vec<protocol::KeyManifestDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullResyncStart {
    pub base_seq: i64,
    pub generation: i64,
    pub page_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableCursor {
    pub collection: SyncCollection,
    pub record_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePage {
    pub records: Vec<PullRecord>,
    pub next_cursor: Option<StableCursor>,
    pub has_more: bool,
    pub next_page_token: Option<String>,
    pub completion_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaPage {
    pub records: Vec<PullRecord>,
    pub next_since: i64,
    pub has_more: bool,
    pub high_water: i64,
    pub closure_proof: Option<ClosureProof>,
}

impl DeltaPage {
    pub const fn reached_closure(&self) -> bool {
        crate::delta_reached_closure(self.next_since, self.has_more, self.high_water)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRecord {
    pub record_id: Uuid,
    pub collection: SyncCollection,
    pub seq: i64,
    pub revision_hlc: String,
    pub state: EncryptedSyncState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncRunSummary {
    pub pushed_count: usize,
    pub push_acked_count: usize,
    pub push_superseded_count: usize,
    pub push_conflict_count: usize,
    pub pulled_count: usize,
    pub applied_count: usize,
    pub deleted_count: usize,
    pub decrypt_failed_count: usize,
    pub repush_count: usize,
    pub missing_key_quarantined_count: usize,
    pub corruption_quarantined_count: usize,
    pub resolved_quarantine_count: usize,
}

impl SyncEngine {
    pub fn new(
        server_url: impl Into<String>,
        tenant_id: Uuid,
        session_token: impl Into<String>,
    ) -> Result<Self, SyncEngineError> {
        let base_url = normalize_base_url(server_url.into())?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url,
            tenant_id,
            session_token: SecretString::new(session_token),
            http,
        })
    }

    pub async fn preflight(&self, since: i64) -> Result<PreflightResult, SyncEngineError> {
        if since < 0 {
            return Err(SyncEngineError::InvalidRequest);
        }
        let response = self.request_preflight(since).await?;
        let full_resync_required = response.status() == StatusCode::GONE;
        if !response.status().is_success() && !full_resync_required {
            return Err(sync_response_error(response.status()));
        }
        let capabilities = response.json::<protocol::SyncCapabilities>().await?;
        if capabilities.protocol_version != protocol::SYNC_PROTOCOL_VERSION
            || capabilities.envelope_version != crate::ENVELOPE_VERSION
        {
            return Err(SyncEngineError::UpgradeRequired {
                protocol_version: capabilities.protocol_version,
                envelope_version: capabilities.envelope_version,
            });
        }
        if capabilities.gc_horizon_seq < 0
            || capabilities.continuity_seq < 0
            || capabilities.continuity_generation < 0
            || capabilities.required_generation < capabilities.continuity_generation
            || capabilities.full_resync_required != full_resync_required
            || capabilities.suite_id != taskveil_crypto::CRYPTO_SUITE_ID
            || capabilities.active_key_generation == 0
            || capabilities.minimum_write_generation != capabilities.active_key_generation
            || capabilities.key_manifests.is_empty()
            || capabilities.key_manifests.iter().any(|manifest| {
                manifest.suite_id != capabilities.suite_id
                    || manifest.generation != capabilities.active_key_generation
                    || manifest.minimum_write_generation != capabilities.minimum_write_generation
                    || !matches!(
                        manifest.status,
                        crate::RotationStatus::Active | crate::RotationStatus::Migrating
                    )
                    || manifest.signed_manifest.is_empty()
            })
        {
            return Err(SyncEngineError::InvalidPreflightResponse);
        }
        for descriptor in &capabilities.key_manifests {
            let bytes = STANDARD
                .decode(&descriptor.signed_manifest)
                .map_err(|_| SyncEngineError::InvalidPreflightResponse)?;
            let manifest = crate::KeyManifest::from_authenticated_bytes(&bytes)
                .map_err(|_| SyncEngineError::InvalidPreflightResponse)?;
            if manifest.tenant_id != self.tenant_id
                || manifest.suite_id != descriptor.suite_id
                || manifest.generation != descriptor.generation
                || manifest.status != descriptor.status
                || manifest.minimum_write_generation != descriptor.minimum_write_generation
            {
                return Err(SyncEngineError::InvalidPreflightResponse);
            }
        }
        Ok(PreflightResult {
            gc_horizon_seq: capabilities.gc_horizon_seq,
            continuity_seq: capabilities.continuity_seq,
            continuity_generation: capabilities.continuity_generation,
            required_generation: capabilities.required_generation,
            full_resync_required,
            suite_id: capabilities.suite_id,
            active_key_generation: capabilities.active_key_generation,
            minimum_write_generation: capabilities.minimum_write_generation,
            migrating_key_generation: capabilities.migrating_key_generation,
            key_manifests: capabilities.key_manifests,
        })
    }

    async fn request_preflight(&self, since: i64) -> Result<reqwest::Response, SyncEngineError> {
        self.http
            .get(format!(
                "{}/v2/tenants/{}/preflight",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .query(&[("since", since)])
            .send()
            .await
            .map_err(SyncEngineError::from)
    }

    pub async fn push_batch(&self, ops: Vec<PushOp>) -> Result<PushBatchOutcome, SyncEngineError> {
        if ops.is_empty() {
            return Ok(PushBatchOutcome {
                outcomes: Vec::new(),
            });
        }
        validate_push_ops(&ops)?;
        let request = PushRequest {
            ops: ops.iter().map(to_wire_push_op).collect(),
        };
        let response = self
            .http
            .post(format!(
                "{}/v2/tenants/{}/push",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(push_response_error(response).await);
        }
        let response = response.json::<protocol::PushResponse>().await?;
        validate_push_response(&ops, response)
    }

    pub async fn begin_full_resync(&self) -> Result<FullResyncStart, SyncEngineError> {
        let response = self
            .http
            .post(format!(
                "{}/v2/tenants/{}/resync/start",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(sync_response_error(response.status()));
        }
        let response = response.json::<protocol::ResyncStartResponse>().await?;
        if response.base_seq < 0 || response.generation <= 0 || response.page_token.is_empty() {
            return Err(SyncEngineError::InvalidPullResponse);
        }
        Ok(FullResyncStart {
            base_seq: response.base_seq,
            generation: response.generation,
            page_token: response.page_token,
        })
    }

    pub async fn scan_base_page(
        &self,
        page_token: &str,
        cursor: Option<&StableCursor>,
        limit: i64,
    ) -> Result<BasePage, SyncEngineError> {
        if page_token.is_empty() {
            return Err(SyncEngineError::InvalidRequest);
        }
        let request = self.build_base_scan_request(page_token, limit)?;
        let response = self.http.execute(request).await?;
        if !response.status().is_success() {
            return Err(resync_response_error(response.status()));
        }
        let response = response.json::<BaseScanResponse>().await?;
        validate_base_response(cursor, response)
    }

    fn build_base_scan_request(
        &self,
        page_token: &str,
        limit: i64,
    ) -> Result<reqwest::Request, SyncEngineError> {
        self.http
            .post(format!(
                "{}/v2/tenants/{}/resync/base",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(&BaseScanRequest {
                page_token: page_token.to_string(),
                limit: Some(limit),
            })
            .build()
            .map_err(SyncEngineError::from)
    }

    pub async fn complete_resync_base(
        &self,
        completion_token: &str,
    ) -> Result<(), SyncEngineError> {
        if completion_token.is_empty() {
            return Err(SyncEngineError::InvalidRequest);
        }
        let response = self
            .http
            .post(format!(
                "{}/v2/tenants/{}/resync/base/complete",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(&CompleteBaseRequest {
                completion_token: completion_token.to_string(),
            })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(resync_response_error(response.status()));
        }
        let response = response.json::<CompleteBaseResponse>().await?;
        if !response.base_complete {
            return Err(SyncEngineError::InvalidPullResponse);
        }
        Ok(())
    }

    pub async fn pull_page(&self, since: i64, limit: i64) -> Result<DeltaPage, SyncEngineError> {
        self.pull_page_for_generation(since, limit, None).await
    }

    pub async fn pull_page_for_generation(
        &self,
        since: i64,
        limit: i64,
        generation: Option<i64>,
    ) -> Result<DeltaPage, SyncEngineError> {
        let mut request = self
            .http
            .get(format!(
                "{}/v2/tenants/{}/pull",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .query(&[("since", since), ("limit", limit)]);
        if let Some(generation) = generation {
            request = request.query(&[("generation", generation)]);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(sync_response_error(response.status()));
        }
        let response = response.json::<PullResponse>().await?;
        validate_pull_response(since, response)
    }

    pub async fn ack_continuity(
        &self,
        proof: ClosureProof,
    ) -> Result<ContinuityAckResponse, SyncEngineError> {
        let response = self
            .http
            .post(format!(
                "{}/v2/tenants/{}/continuity/ack",
                self.base_url, self.tenant_id
            ))
            .bearer_auth(self.session_token.expose())
            .header(
                SYNC_PROTOCOL_VERSION_HEADER,
                protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .json(&ContinuityAckRequest { proof })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(sync_response_error(response.status()));
        }
        response.json().await.map_err(SyncEngineError::from)
    }
}

fn sync_response_error(status: StatusCode) -> SyncEngineError {
    if status == StatusCode::PAYMENT_REQUIRED {
        SyncEngineError::EntitlementRequired
    } else {
        SyncEngineError::Server(status)
    }
}

#[derive(Deserialize)]
struct SyncProblem {
    code: Option<String>,
}

async fn push_response_error(response: reqwest::Response) -> SyncEngineError {
    let status = response.status();
    let code = response
        .json::<SyncProblem>()
        .await
        .ok()
        .and_then(|problem| problem.code);
    classify_push_response_error(status, code.as_deref())
}

fn classify_push_response_error(status: StatusCode, code: Option<&str>) -> SyncEngineError {
    if status == StatusCode::CONFLICT && code == Some(protocol::SYNC_CLOCK_SKEW_RETRYABLE_CODE) {
        SyncEngineError::ClockSkewRetryable
    } else {
        sync_response_error(status)
    }
}

fn resync_response_error(status: StatusCode) -> SyncEngineError {
    if status == StatusCode::CONFLICT {
        SyncEngineError::ResyncRestartRequired
    } else {
        sync_response_error(status)
    }
}

fn normalize_base_url(mut value: String) -> Result<String, SyncEngineError> {
    value = value.trim().to_string();
    if value.is_empty() {
        return Err(SyncEngineError::EmptyServerUrl);
    }
    crate::canonical_server_origin(&value).map_err(|_| SyncEngineError::InvalidServerOrigin)
}

fn validate_push_ops(ops: &[PushOp]) -> Result<(), SyncEngineError> {
    let mut op_ids = HashSet::with_capacity(ops.len());
    for op in ops {
        if !op_ids.insert(op.op_id)
            || Hlc::decode_observable(&op.revision_hlc).is_err()
            || op
                .base_revision_hlc
                .as_deref()
                .is_some_and(|base| Hlc::decode_observable(base).is_err())
            || !valid_state_for_revision(&op.revision_hlc, &op.state)
        {
            return Err(SyncEngineError::InvalidRequest);
        }
    }
    Ok(())
}

fn valid_state_for_revision(revision_hlc: &str, state: &EncryptedSyncState) -> bool {
    let Ok(revision) = Hlc::decode_observable(revision_hlc) else {
        return false;
    };
    let (semantic_hlc, shape_is_valid) = match state {
        EncryptedSyncState::Live { mutation_hlc, blob } => (mutation_hlc, !blob.is_empty()),
        EncryptedSyncState::Tombstone { delete_hlc } => (delete_hlc, true),
    };
    shape_is_valid
        && Hlc::decode_observable(semantic_hlc).is_ok_and(|semantic| revision >= semantic)
}

fn to_wire_push_op(op: &PushOp) -> protocol::PushOp {
    protocol::PushOp {
        op_id: op.op_id,
        record_id: op.record_id,
        collection: op.collection,
        base_revision_hlc: op.base_revision_hlc.clone(),
        revision_hlc: op.revision_hlc.clone(),
        state: to_wire_state(&op.state),
    }
}

fn to_wire_state(state: &EncryptedSyncState) -> WireRecordState {
    match state {
        EncryptedSyncState::Live { mutation_hlc, blob } => WireRecordState::Live {
            mutation_hlc: mutation_hlc.clone(),
            blob: STANDARD.encode(blob),
        },
        EncryptedSyncState::Tombstone { delete_hlc } => WireRecordState::Tombstone {
            delete_hlc: delete_hlc.clone(),
        },
    }
}

fn validate_push_response(
    ops: &[PushOp],
    response: protocol::PushResponse,
) -> Result<PushBatchOutcome, SyncEngineError> {
    if response.results.len() != ops.len() {
        return Err(SyncEngineError::InvalidPushResponse);
    }
    let expected = ops
        .iter()
        .map(|op| (op.op_id, (op.record_id, op.collection)))
        .collect::<HashMap<_, _>>();
    let mut decoded = HashMap::with_capacity(response.results.len());
    for result in response.results {
        let Some((record_id, collection)) = expected.get(&result.op_id).copied() else {
            return Err(SyncEngineError::InvalidPushResponse);
        };
        if result.record_id != record_id
            || result.collection != collection
            || decoded.contains_key(&result.op_id)
        {
            return Err(SyncEngineError::InvalidPushResponse);
        }
        let current = result
            .current
            .map(decode_record)
            .transpose()
            .map_err(|_| SyncEngineError::InvalidPushResponse)?;
        if current.as_ref().is_some_and(|current| {
            current.record_id != record_id
                || current.collection != collection
                || result.seq.is_some_and(|seq| seq != current.seq)
        }) {
            return Err(SyncEngineError::InvalidPushResponse);
        }
        let valid_shape = match result.status {
            PushStatus::Accepted | PushStatus::NoOp => result.seq.is_some() && current.is_none(),
            PushStatus::Superseded => result.seq.is_some() && current.is_some(),
            PushStatus::Conflict => matches!(
                (result.seq, current.as_ref()),
                (Some(_), Some(_)) | (None, None)
            ),
        };
        if !valid_shape {
            return Err(SyncEngineError::InvalidPushResponse);
        }
        decoded.insert(
            result.op_id,
            PushOpOutcome {
                op_id: result.op_id,
                record_id,
                collection,
                status: result.status,
                seq: result.seq,
                current,
            },
        );
    }
    let outcomes = ops
        .iter()
        .map(|op| {
            decoded
                .remove(&op.op_id)
                .ok_or(SyncEngineError::InvalidPushResponse)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PushBatchOutcome { outcomes })
}

fn validate_pull_response(
    since: i64,
    response: PullResponse,
) -> Result<DeltaPage, SyncEngineError> {
    if response.next_since < since
        || response.high_water < since
        || response.next_since > response.high_water
        || (response.has_more && response.next_since >= response.high_water)
        || (!response.has_more && response.next_since != response.high_water)
    {
        return Err(SyncEngineError::InvalidPullResponse);
    }
    let mut previous_seq = since;
    let mut records = Vec::with_capacity(response.records.len());
    for record in response.records {
        let record = decode_record(record)?;
        if record.seq <= previous_seq || record.seq > response.next_since {
            return Err(SyncEngineError::InvalidPullResponse);
        }
        previous_seq = record.seq;
        records.push(record);
    }
    if response.has_more
        && records
            .last()
            .is_none_or(|record| record.seq != response.next_since)
    {
        return Err(SyncEngineError::InvalidPullResponse);
    }
    Ok(DeltaPage {
        records,
        next_since: response.next_since,
        has_more: response.has_more,
        high_water: response.high_water,
        closure_proof: response.closure_proof,
    })
}

fn validate_base_response(
    cursor: Option<&StableCursor>,
    response: BaseScanResponse,
) -> Result<BasePage, SyncEngineError> {
    let mut previous = cursor.cloned();
    let mut records = Vec::with_capacity(response.records.len());
    for record in response.records {
        let stable = StableCursor {
            collection: record.collection,
            record_id: record.record_id,
        };
        if previous
            .as_ref()
            .is_some_and(|previous| stable_key(&stable) <= stable_key(previous))
        {
            return Err(SyncEngineError::InvalidPullResponse);
        }
        previous = Some(stable);
        records.push(decode_record(record)?);
    }
    let next_cursor = response.next_cursor.map(|cursor| StableCursor {
        collection: cursor.collection,
        record_id: cursor.record_id,
    });
    if records
        .last()
        .map(|record| (record.collection, record.record_id))
        != next_cursor
            .as_ref()
            .map(|cursor| (cursor.collection, cursor.record_id))
        || (response.has_more && records.is_empty())
        || (response.has_more
            && (response
                .next_page_token
                .as_deref()
                .is_none_or(str::is_empty)
                || response.completion_token.is_some()))
        || (!response.has_more
            && (response.next_page_token.is_some()
                || response
                    .completion_token
                    .as_deref()
                    .is_none_or(str::is_empty)))
    {
        return Err(SyncEngineError::InvalidPullResponse);
    }
    Ok(BasePage {
        records,
        next_cursor,
        has_more: response.has_more,
        next_page_token: response.next_page_token,
        completion_token: response.completion_token,
    })
}

fn stable_key(cursor: &StableCursor) -> (&'static str, Uuid) {
    (cursor.collection.as_str(), cursor.record_id)
}

fn decode_record(record: protocol::SyncRecord) -> Result<PullRecord, SyncEngineError> {
    if record.seq <= 0 || Hlc::decode_observable(&record.revision_hlc).is_err() {
        return Err(SyncEngineError::InvalidPullResponse);
    }
    let state = match record.state {
        WireRecordState::Live { mutation_hlc, blob } => {
            let blob = STANDARD
                .decode(blob)
                .map_err(|_| SyncEngineError::InvalidPullResponse)?;
            let state = EncryptedSyncState::Live { mutation_hlc, blob };
            if !valid_state_for_revision(&record.revision_hlc, &state) {
                return Err(SyncEngineError::InvalidPullResponse);
            }
            state
        }
        WireRecordState::Tombstone { delete_hlc } => {
            let state = EncryptedSyncState::Tombstone { delete_hlc };
            if !valid_state_for_revision(&record.revision_hlc, &state) {
                return Err(SyncEngineError::InvalidPullResponse);
            }
            state
        }
    };
    Ok(PullRecord {
        record_id: record.record_id,
        collection: record.collection,
        seq: record.seq,
        revision_hlc: record.revision_hlc,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(device: &str, counter: u32) -> String {
        Hlc {
            wall_ms: 1_799_000_000_000,
            counter,
            device_id: device.to_string(),
        }
        .encode()
        .unwrap()
    }

    #[test]
    fn invalid_base_page_does_not_prevent_retrying_the_same_page_token_position() {
        let record_id = Uuid::now_v7();
        let poisoned = BaseScanResponse {
            records: vec![protocol::SyncRecord {
                record_id,
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: "poisoned-hlc".to_string(),
                state: WireRecordState::Tombstone {
                    delete_hlc: "poisoned-hlc".to_string(),
                },
            }],
            next_cursor: Some(protocol::StableRecordCursor {
                collection: SyncCollection::Tasks,
                record_id,
            }),
            has_more: false,
            next_page_token: None,
            completion_token: Some("completion".to_string()),
        };
        assert!(matches!(
            validate_base_response(None, poisoned),
            Err(SyncEngineError::InvalidPullResponse)
        ));

        let retried = validate_base_response(
            None,
            BaseScanResponse {
                records: Vec::new(),
                next_cursor: None,
                has_more: false,
                next_page_token: None,
                completion_token: Some("completion".to_string()),
            },
        )
        .unwrap();
        assert!(retried.records.is_empty());
        assert_eq!(retried.completion_token.as_deref(), Some("completion"));
    }

    fn push_op(op_id: Uuid, record_id: Uuid) -> PushOp {
        PushOp {
            op_id,
            record_id,
            collection: SyncCollection::Tasks,
            base_revision_hlc: None,
            revision_hlc: clock("local", 1),
            state: EncryptedSyncState::Live {
                mutation_hlc: clock("local", 1),
                blob: vec![1, 2, 3],
            },
        }
    }

    fn accepted(op: &PushOp) -> protocol::PushResult {
        protocol::PushResult {
            op_id: op.op_id,
            record_id: op.record_id,
            collection: op.collection,
            status: PushStatus::Accepted,
            seq: Some(1),
            current: None,
        }
    }

    #[test]
    fn rejects_empty_server_url() {
        let error = SyncEngine::new(" ", Uuid::now_v7(), "token").unwrap_err();
        assert!(matches!(error, SyncEngineError::EmptyServerUrl));
    }

    #[test]
    fn debug_never_renders_bearer_token() {
        let engine =
            SyncEngine::new("https://sync.example.com", Uuid::now_v7(), "bearer-secret").unwrap();
        let rendered = format!("{engine:?}");

        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("bearer-secret"));
    }

    #[test]
    fn maps_payment_required_to_typed_entitlement_error() {
        assert!(matches!(
            sync_response_error(StatusCode::PAYMENT_REQUIRED),
            SyncEngineError::EntitlementRequired
        ));
    }

    #[test]
    fn base_scan_uses_post_body_and_never_places_page_token_in_url() {
        let tenant_id = Uuid::now_v7();
        let embedded_device_id = Uuid::now_v7();
        let page_token = format!("opaque.{tenant_id}.{embedded_device_id}");
        let engine =
            SyncEngine::new("https://sync.example.com", tenant_id, "bearer-secret").unwrap();

        let request = engine.build_base_scan_request(&page_token, 100).unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert!(request.url().query().is_none());
        assert!(!request.url().as_str().contains(&page_token));
        assert!(!request
            .url()
            .as_str()
            .contains(&embedded_device_id.to_string()));
        let body: BaseScanRequest =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body.page_token, page_token);
        assert_eq!(body.limit, Some(100));
    }

    #[test]
    fn maps_only_stable_clock_skew_problem_code_to_typed_retryable_error() {
        assert!(matches!(
            classify_push_response_error(StatusCode::CONFLICT, Some("sync_clock_skew_retryable")),
            SyncEngineError::ClockSkewRetryable
        ));
        assert!(matches!(
            classify_push_response_error(StatusCode::CONFLICT, Some("other_conflict")),
            SyncEngineError::Server(StatusCode::CONFLICT)
        ));
        assert!(matches!(
            classify_push_response_error(
                StatusCode::BAD_REQUEST,
                Some("sync_clock_skew_retryable")
            ),
            SyncEngineError::Server(StatusCode::BAD_REQUEST)
        ));
    }

    #[test]
    fn resync_conflict_requires_a_bounded_local_restart() {
        assert!(matches!(
            resync_response_error(StatusCode::CONFLICT),
            SyncEngineError::ResyncRestartRequired
        ));
        assert!(matches!(
            resync_response_error(StatusCode::UNAUTHORIZED),
            SyncEngineError::Server(StatusCode::UNAUTHORIZED)
        ));
    }

    #[test]
    fn push_response_can_be_reordered_and_is_returned_in_request_order() {
        let first = push_op(Uuid::now_v7(), Uuid::now_v7());
        let second = push_op(Uuid::now_v7(), Uuid::now_v7());
        let response = protocol::PushResponse {
            results: vec![accepted(&second), accepted(&first)],
        };

        let outcome = validate_push_response(&[first.clone(), second.clone()], response).unwrap();

        assert_eq!(outcome.outcomes[0].op_id, first.op_id);
        assert_eq!(outcome.outcomes[1].op_id, second.op_id);
    }

    #[test]
    fn push_response_rejects_missing_duplicate_unknown_and_record_mismatch() {
        let first = push_op(Uuid::now_v7(), Uuid::now_v7());
        let second = push_op(Uuid::now_v7(), Uuid::now_v7());
        let cases = [
            protocol::PushResponse {
                results: vec![accepted(&first)],
            },
            protocol::PushResponse {
                results: vec![accepted(&first), accepted(&first)],
            },
            protocol::PushResponse {
                results: vec![
                    accepted(&first),
                    accepted(&push_op(Uuid::now_v7(), second.record_id)),
                ],
            },
            protocol::PushResponse {
                results: vec![
                    accepted(&first),
                    protocol::PushResult {
                        record_id: Uuid::now_v7(),
                        ..accepted(&second)
                    },
                ],
            },
        ];

        for response in cases {
            assert!(matches!(
                validate_push_response(&[first.clone(), second.clone()], response),
                Err(SyncEngineError::InvalidPushResponse)
            ));
        }
    }

    #[test]
    fn push_response_rejects_invalid_status_shapes() {
        let op = push_op(Uuid::now_v7(), Uuid::now_v7());
        let current = protocol::SyncRecord {
            record_id: op.record_id,
            collection: op.collection,
            seq: 1,
            revision_hlc: clock("remote", 2),
            state: WireRecordState::Live {
                mutation_hlc: clock("remote", 1),
                blob: STANDARD.encode([1, 2, 3]),
            },
        };
        let invalid = [
            protocol::PushResponse {
                results: vec![protocol::PushResult {
                    op_id: op.op_id,
                    record_id: op.record_id,
                    collection: op.collection,
                    status: PushStatus::Superseded,
                    seq: None,
                    current: Some(current.clone()),
                }],
            },
            protocol::PushResponse {
                results: vec![protocol::PushResult {
                    op_id: op.op_id,
                    record_id: op.record_id,
                    collection: op.collection,
                    status: PushStatus::Conflict,
                    seq: Some(1),
                    current: None,
                }],
            },
        ];

        for response in invalid {
            assert!(matches!(
                validate_push_response(std::slice::from_ref(&op), response),
                Err(SyncEngineError::InvalidPushResponse)
            ));
        }
    }

    #[test]
    fn pull_rejects_invalid_base64_and_clock() {
        let response = PullResponse {
            records: vec![protocol::SyncRecord {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: clock("remote", 1),
                state: WireRecordState::Live {
                    mutation_hlc: clock("remote", 1),
                    blob: "%%%".to_string(),
                },
            }],
            next_since: 1,
            has_more: false,
            high_water: 1,
            closure_proof: None,
        };
        assert!(matches!(
            validate_pull_response(0, response),
            Err(SyncEngineError::InvalidPullResponse)
        ));

        let response = PullResponse {
            records: vec![protocol::SyncRecord {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: "invalid".to_string(),
                state: WireRecordState::Tombstone {
                    delete_hlc: clock("remote", 1),
                },
            }],
            next_since: 1,
            has_more: false,
            high_water: 1,
            closure_proof: None,
        };
        assert!(matches!(
            validate_pull_response(0, response),
            Err(SyncEngineError::InvalidPullResponse)
        ));

        let response = PullResponse {
            records: vec![protocol::SyncRecord {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: clock("remote", 1),
                state: WireRecordState::Tombstone {
                    delete_hlc: clock("remote", 2),
                },
            }],
            next_since: 1,
            has_more: false,
            high_water: 1,
            closure_proof: None,
        };
        assert!(matches!(
            validate_pull_response(0, response),
            Err(SyncEngineError::InvalidPullResponse)
        ));
    }

    #[test]
    fn pull_rejects_counter_without_observation_headroom_before_returning_page() {
        for counter in [u32::MAX - 1, u32::MAX] {
            let response = PullResponse {
                records: vec![
                    protocol::SyncRecord {
                        record_id: Uuid::now_v7(),
                        collection: SyncCollection::Tasks,
                        seq: 1,
                        revision_hlc: clock("honest", 1),
                        state: WireRecordState::Tombstone {
                            delete_hlc: clock("honest", 1),
                        },
                    },
                    protocol::SyncRecord {
                        record_id: Uuid::now_v7(),
                        collection: SyncCollection::Tasks,
                        seq: 2,
                        revision_hlc: clock("attacker", counter),
                        state: WireRecordState::Tombstone {
                            delete_hlc: clock("attacker", counter),
                        },
                    },
                ],
                next_since: 2,
                has_more: false,
                high_water: 2,
                closure_proof: None,
            };

            assert!(matches!(
                validate_pull_response(0, response.clone()),
                Err(SyncEngineError::InvalidPullResponse)
            ));
            // The same poisoned page stays fail-closed on retry. Since validation never
            // returns a DeltaPage, callers cannot advance the cursor or apply the honest
            // record that preceded the poison record.
            assert!(matches!(
                validate_pull_response(0, response),
                Err(SyncEngineError::InvalidPullResponse)
            ));
        }
    }

    #[test]
    fn pull_accepts_last_counter_with_observation_and_local_tick_headroom() {
        let counter = u32::MAX - 2;
        let response = PullResponse {
            records: vec![protocol::SyncRecord {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: clock("remote", counter),
                state: WireRecordState::Tombstone {
                    delete_hlc: clock("remote", counter),
                },
            }],
            next_since: 1,
            has_more: false,
            high_water: 1,
            closure_proof: None,
        };

        let page = validate_pull_response(0, response).unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].revision_hlc, clock("remote", counter));
    }

    #[test]
    fn delta_closure_requires_cursor_to_equal_page_high_water() {
        let open = PullResponse {
            records: vec![protocol::SyncRecord {
                record_id: Uuid::now_v7(),
                collection: SyncCollection::Tasks,
                seq: 1,
                revision_hlc: clock("remote", 1),
                state: WireRecordState::Tombstone {
                    delete_hlc: clock("remote", 1),
                },
            }],
            next_since: 1,
            has_more: true,
            high_water: 2,
            closure_proof: None,
        };
        let open = validate_pull_response(0, open).unwrap();
        assert!(!open.reached_closure());

        let premature = PullResponse {
            records: Vec::new(),
            next_since: 1,
            has_more: false,
            high_water: 2,
            closure_proof: None,
        };
        assert!(matches!(
            validate_pull_response(1, premature),
            Err(SyncEngineError::InvalidPullResponse)
        ));

        let closed = PullResponse {
            records: Vec::new(),
            next_since: 2,
            has_more: false,
            high_water: 2,
            closure_proof: None,
        };
        assert!(validate_pull_response(1, closed).unwrap().reached_closure());
    }
}
