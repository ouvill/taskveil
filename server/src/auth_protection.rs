use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const AUTH_START_BODY_LIMIT: usize = 8 * 1024;
// Registration finish carries the OPAQUE upload plus the initial encrypted
// account/device key bundle. Keep it bounded independently from the much
// smaller start requests.
pub const AUTH_REGISTER_FINISH_BODY_LIMIT: usize = 64 * 1024;
pub const TRUSTED_SOURCE_IP_HEADER: &str = "x-taskveil-source-ip";

const TABLE_ENTRY_LIMIT: usize = 4_096;
const CLEANUP_SCAN_LIMIT: usize = 32;
const ENTRY_IDLE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct AuthProtection {
    key: Arc<[u8; 32]>,
    state: Arc<Mutex<ProtectionState>>,
    policy: ProtectionPolicy,
}

#[derive(Clone, Copy)]
struct ProtectionPolicy {
    global: BucketPolicy,
    source: BucketPolicy,
    identifier: BucketPolicy,
    table_entry_limit: usize,
    cleanup_scan_limit: usize,
    entry_idle_ttl: Duration,
}

impl Default for ProtectionPolicy {
    fn default() -> Self {
        Self {
            // API Gateway is configured more tightly. These application limits
            // remain effective when that edge control is absent or imprecise.
            global: BucketPolicy::new(60, Duration::from_millis(100)),
            source: BucketPolicy::new(20, Duration::from_secs(1)),
            identifier: BucketPolicy::new(12, Duration::from_secs(5)),
            table_entry_limit: TABLE_ENTRY_LIMIT,
            cleanup_scan_limit: CLEANUP_SCAN_LIMIT,
            entry_idle_ttl: ENTRY_IDLE_TTL,
        }
    }
}

#[derive(Clone, Copy)]
struct BucketPolicy {
    capacity: u32,
    refill_every: Duration,
}

impl BucketPolicy {
    const fn new(capacity: u32, refill_every: Duration) -> Self {
        Self {
            capacity,
            refill_every,
        }
    }
}

struct ProtectionState {
    global: TokenBucket,
    sources: BoundedBuckets<ClientSource>,
    identifiers: BoundedBuckets<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClientSource {
    Ip(IpAddr),
    Unattributed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitScope {
    Global,
    Source,
    Identifier,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitExceeded {
    pub scope: LimitScope,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Clone, Copy)]
pub struct AuthAdmission {
    identifier_key: [u8; 32],
}

impl std::fmt::Debug for AuthAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthAdmission([redacted])")
    }
}

impl AuthAdmission {
    pub fn identifier_key(&self) -> &[u8; 32] {
        &self.identifier_key
    }
}

impl AuthProtection {
    pub fn new(key: [u8; 32]) -> Self {
        Self::with_policy(key, ProtectionPolicy::default())
    }

    pub fn admit(
        &self,
        source: ClientSource,
        identifier: &str,
    ) -> Result<AuthAdmission, LimitExceeded> {
        self.admit_at(source, identifier, Instant::now())
    }

    pub fn admit_source(&self, source: ClientSource) -> Result<(), LimitExceeded> {
        self.admit_source_at(source, Instant::now())
    }

    pub fn admit_identifier(&self, identifier: &str) -> Result<AuthAdmission, LimitExceeded> {
        self.admit_identifier_at(identifier, Instant::now())
    }

    fn with_policy(key: [u8; 32], policy: ProtectionPolicy) -> Self {
        Self {
            key: Arc::new(key),
            state: Arc::new(Mutex::new(ProtectionState {
                global: TokenBucket::full(Instant::now(), policy.global),
                sources: BoundedBuckets::new(Instant::now(), policy.source),
                identifiers: BoundedBuckets::new(Instant::now(), policy.identifier),
            })),
            policy,
        }
    }

    fn admit_at(
        &self,
        source: ClientSource,
        identifier: &str,
        now: Instant,
    ) -> Result<AuthAdmission, LimitExceeded> {
        self.admit_source_at(source, now)?;
        self.admit_identifier_at(identifier, now)
    }

    fn admit_source_at(&self, source: ClientSource, now: Instant) -> Result<(), LimitExceeded> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        if let Err(retry_after_seconds) = state.sources.take(
            source,
            now,
            self.policy.source,
            self.policy.table_entry_limit,
            self.policy.cleanup_scan_limit,
            self.policy.entry_idle_ttl,
        ) {
            return Err(self.exceeded(LimitScope::Source, Some(retry_after_seconds)));
        }
        if !state.global.take(now, self.policy.global) {
            let retry_after_seconds = state.global.retry_after_seconds(now, self.policy.global);
            return Err(self.exceeded(LimitScope::Global, Some(retry_after_seconds)));
        }
        Ok(())
    }

    fn admit_identifier_at(
        &self,
        identifier: &str,
        now: Instant,
    ) -> Result<AuthAdmission, LimitExceeded> {
        let identifier_key = self.identifier_key(identifier);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .identifiers
            .take(
                identifier_key,
                now,
                self.policy.identifier,
                self.policy.table_entry_limit,
                self.policy.cleanup_scan_limit,
                self.policy.entry_idle_ttl,
            )
            .is_err()
        {
            return Err(self.exceeded(LimitScope::Identifier, None));
        }

        Ok(AuthAdmission { identifier_key })
    }

    fn exceeded(&self, scope: LimitScope, retry_after_seconds: Option<u64>) -> LimitExceeded {
        LimitExceeded {
            scope,
            retry_after_seconds,
        }
    }

    fn identifier_key(&self, identifier: &str) -> [u8; 32] {
        // Callers pass their protocol's canonical identifier. Do not fold it
        // here: email local-parts and opaque registration tickets are
        // case-sensitive, and sharing their buckets enables cross-identity DoS.
        let canonical = identifier.trim();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.key.as_slice()).expect("HMAC accepts any key size");
        mac.update(b"taskveil/auth-limit/identifier/v1\0");
        mac.update(canonical.as_bytes());
        mac.finalize().into_bytes().into()
    }
}

struct BoundedBuckets<K> {
    entries: HashMap<K, TokenBucket>,
    cleanup_queue: VecDeque<K>,
    overflow: TokenBucket,
}

impl<K> BoundedBuckets<K>
where
    K: Copy + Eq + Hash,
{
    fn new(now: Instant, policy: BucketPolicy) -> Self {
        Self {
            entries: HashMap::new(),
            cleanup_queue: VecDeque::new(),
            overflow: TokenBucket::full(now, policy),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn take(
        &mut self,
        key: K,
        now: Instant,
        policy: BucketPolicy,
        entry_limit: usize,
        cleanup_scan_limit: usize,
        idle_ttl: Duration,
    ) -> Result<(), u64> {
        if let Some(bucket) = self.entries.get_mut(&key) {
            return take_bucket(bucket, now, policy);
        }

        self.prune_at_most(now, cleanup_scan_limit, idle_ttl);
        if self.entries.len() >= entry_limit {
            return take_bucket(&mut self.overflow, now, policy);
        }

        let mut bucket = TokenBucket::full(now, policy);
        let result = take_bucket(&mut bucket, now, policy);
        self.entries.insert(key, bucket);
        self.cleanup_queue.push_back(key);
        result
    }

    fn prune_at_most(&mut self, now: Instant, scan_limit: usize, idle_ttl: Duration) {
        for _ in 0..scan_limit {
            let Some(key) = self.cleanup_queue.pop_front() else {
                break;
            };
            let stale = self
                .entries
                .get(&key)
                .is_some_and(|bucket| now.saturating_duration_since(bucket.last_seen) >= idle_ttl);
            if stale {
                self.entries.remove(&key);
            } else {
                self.cleanup_queue.push_back(key);
            }
        }
    }
}

fn take_bucket(bucket: &mut TokenBucket, now: Instant, policy: BucketPolicy) -> Result<(), u64> {
    if bucket.take(now, policy) {
        Ok(())
    } else {
        Err(bucket.retry_after_seconds(now, policy))
    }
}

struct TokenBucket {
    tokens: u32,
    last_refill: Instant,
    last_seen: Instant,
}

impl TokenBucket {
    fn full(now: Instant, policy: BucketPolicy) -> Self {
        Self {
            tokens: policy.capacity,
            last_refill: now,
            last_seen: now,
        }
    }

    fn take(&mut self, now: Instant, policy: BucketPolicy) -> bool {
        self.refill(now, policy);
        self.last_seen = now;
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }

    fn refill(&mut self, now: Instant, policy: BucketPolicy) {
        let elapsed = now.saturating_duration_since(self.last_refill);
        let interval_nanos = policy.refill_every.as_nanos();
        if interval_nanos == 0 {
            self.tokens = policy.capacity;
            self.last_refill = now;
            return;
        }
        let refill = elapsed.as_nanos() / interval_nanos;
        if refill == 0 {
            return;
        }
        let refill = u32::try_from(refill).unwrap_or(u32::MAX);
        self.tokens = self.tokens.saturating_add(refill).min(policy.capacity);
        let remainder_nanos = elapsed.as_nanos() % interval_nanos;
        let remainder = Duration::new(
            u64::try_from(remainder_nanos / 1_000_000_000).unwrap_or(u64::MAX),
            u32::try_from(remainder_nanos % 1_000_000_000).unwrap_or(999_999_999),
        );
        self.last_refill = now.checked_sub(remainder).unwrap_or(now);
    }

    fn retry_after_seconds(&self, now: Instant, policy: BucketPolicy) -> u64 {
        if policy.refill_every.is_zero() {
            return 1;
        }
        let elapsed = now.saturating_duration_since(self.last_refill);
        let remaining = policy.refill_every.checked_sub(elapsed).unwrap_or_default();
        let seconds = remaining.as_nanos().div_ceil(1_000_000_000);
        u64::try_from(seconds).unwrap_or(u64::MAX).max(1)
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::header, response::IntoResponse};

    use super::*;
    use crate::AppError;

    fn test_policy(
        global: BucketPolicy,
        source: BucketPolicy,
        identifier: BucketPolicy,
    ) -> ProtectionPolicy {
        ProtectionPolicy {
            global,
            source,
            identifier,
            table_entry_limit: 4,
            cleanup_scan_limit: 2,
            entry_idle_ttl: Duration::from_secs(10),
        }
    }

    #[test]
    fn burst_and_distributed_sources_are_bounded() {
        let policy = test_policy(
            BucketPolicy::new(3, Duration::from_secs(60)),
            BucketPolicy::new(3, Duration::from_secs(60)),
            BucketPolicy::new(3, Duration::from_secs(60)),
        );
        let protection = AuthProtection::with_policy([0x11; 32], policy);
        let now = Instant::now();

        for index in 0..3 {
            let source = ClientSource::Ip(format!("192.0.2.{}", index + 1).parse().unwrap());
            protection
                .admit_at(source, &format!("person-{index}@example.com"), now)
                .unwrap();
        }
        let error = protection
            .admit_at(
                ClientSource::Ip("198.51.100.9".parse().unwrap()),
                "another@example.com",
                now,
            )
            .expect_err("global limit");
        assert_eq!(error.scope, LimitScope::Global);
        assert_eq!(error.retry_after_seconds, Some(60));
    }

    #[test]
    fn identifier_decision_is_independent_of_account_existence() {
        let policy = test_policy(
            BucketPolicy::new(20, Duration::from_secs(60)),
            BucketPolicy::new(20, Duration::from_secs(60)),
            BucketPolicy::new(2, Duration::from_secs(60)),
        );
        let known = AuthProtection::with_policy([0x22; 32], policy);
        let unknown = AuthProtection::with_policy([0x22; 32], policy);
        let source = ClientSource::Ip("203.0.113.8".parse().unwrap());
        let now = Instant::now();

        for attempt in 0..3 {
            let known_result = known.admit_at(source, " Known@example.com ", now);
            let unknown_result = unknown.admit_at(source, "unknown@example.com", now);
            assert_eq!(
                known_result.as_ref().err(),
                unknown_result.as_ref().err(),
                "attempt {attempt}"
            );
        }
        let error = known
            .admit_at(source, "Known@example.com", now)
            .expect_err("identifier limit");
        assert_eq!(error.scope, LimitScope::Identifier);
        assert_eq!(error.retry_after_seconds, None);
        assert_eq!(
            known.identifier_key(" Known@example.com "),
            known.identifier_key("Known@example.com")
        );
        assert_ne!(
            known.identifier_key("Known@example.com"),
            known.identifier_key("known@example.com")
        );
        let another_key = AuthProtection::with_policy([0x23; 32], policy);
        assert_ne!(
            known.identifier_key("known@example.com"),
            another_key.identifier_key("known@example.com")
        );
    }

    #[tokio::test]
    async fn known_and_unknown_identifier_limits_have_identical_http_responses() {
        let policy = test_policy(
            BucketPolicy::new(20, Duration::from_secs(60)),
            BucketPolicy::new(20, Duration::from_secs(60)),
            BucketPolicy::new(1, Duration::from_secs(60)),
        );
        let known = AuthProtection::with_policy([0x29; 32], policy);
        let unknown = AuthProtection::with_policy([0x29; 32], policy);
        let source = ClientSource::Ip("203.0.113.9".parse().unwrap());
        let now = Instant::now();
        known.admit_at(source, "known@example.com", now).unwrap();
        unknown
            .admit_at(source, "unknown@example.com", now)
            .unwrap();

        let known_limit = known
            .admit_at(source, "known@example.com", now)
            .expect_err("known identifier limit");
        let unknown_limit = unknown
            .admit_at(source, "unknown@example.com", now)
            .expect_err("unknown identifier limit");
        let known_response =
            AppError::rate_limited(known_limit.retry_after_seconds).into_response();
        let unknown_response =
            AppError::rate_limited(unknown_limit.retry_after_seconds).into_response();

        assert_eq!(known_response.status(), unknown_response.status());
        assert_eq!(
            known_response.headers().get(header::RETRY_AFTER),
            unknown_response.headers().get(header::RETRY_AFTER)
        );
        assert!(known_response.headers().get(header::RETRY_AFTER).is_none());
        let known_body = to_bytes(known_response.into_body(), 1024).await.unwrap();
        let unknown_body = to_bytes(unknown_response.into_body(), 1024).await.unwrap();
        assert_eq!(known_body, unknown_body);
        assert_eq!(known_body.as_ref(), br#"{"error":"too many requests"}"#);
    }

    #[test]
    fn per_source_limit_has_retry_after_but_identifier_limit_does_not() {
        let source_limited = AuthProtection::with_policy(
            [0x33; 32],
            test_policy(
                BucketPolicy::new(20, Duration::from_secs(60)),
                BucketPolicy::new(1, Duration::from_secs(7)),
                BucketPolicy::new(20, Duration::from_secs(60)),
            ),
        );
        let now = Instant::now();
        let source = ClientSource::Ip("192.0.2.10".parse().unwrap());
        source_limited
            .admit_at(source, "a@example.com", now)
            .unwrap();
        let error = source_limited
            .admit_at(source, "b@example.com", now)
            .expect_err("source limit");
        assert_eq!(error.scope, LimitScope::Source);
        assert_eq!(error.retry_after_seconds, Some(7));

        let error = source_limited
            .admit_source_at(source, now + Duration::from_secs(3))
            .expect_err("source limit with a partial refill interval");
        assert_eq!(error.scope, LimitScope::Source);
        assert_eq!(error.retry_after_seconds, Some(4));
    }

    #[test]
    fn source_limited_attempts_do_not_consume_global_capacity() {
        let protection = AuthProtection::with_policy(
            [0x35; 32],
            test_policy(
                BucketPolicy::new(3, Duration::from_secs(60)),
                BucketPolicy::new(1, Duration::from_secs(60)),
                BucketPolicy::new(20, Duration::from_secs(60)),
            ),
        );
        let now = Instant::now();
        let abusive_source = ClientSource::Ip("192.0.2.11".parse().unwrap());
        protection
            .admit_at(abusive_source, "first@example.com", now)
            .unwrap();

        for attempt in 0..10 {
            let error = protection
                .admit_at(
                    abusive_source,
                    &format!("blocked-{attempt}@example.com"),
                    now,
                )
                .expect_err("source-limited request");
            assert_eq!(error.scope, LimitScope::Source);
        }

        for (source, identifier) in [
            ("198.51.100.1", "second@example.com"),
            ("198.51.100.2", "third@example.com"),
        ] {
            protection
                .admit_at(ClientSource::Ip(source.parse().unwrap()), identifier, now)
                .expect("other sources retain global capacity");
        }
        let error = protection
            .admit_at(
                ClientSource::Ip("198.51.100.3".parse().unwrap()),
                "fourth@example.com",
                now,
            )
            .expect_err("global capacity is exhausted only by admitted sources");
        assert_eq!(error.scope, LimitScope::Global);
    }

    #[test]
    fn key_tables_and_cleanup_work_are_bounded() {
        let policy = test_policy(
            BucketPolicy::new(50, Duration::from_secs(60)),
            BucketPolicy::new(50, Duration::from_secs(60)),
            BucketPolicy::new(50, Duration::from_secs(60)),
        );
        let protection = AuthProtection::with_policy([0x44; 32], policy);
        let now = Instant::now();
        for index in 0..12 {
            protection
                .admit_at(
                    ClientSource::Ip(format!("198.51.100.{}", index + 1).parse().unwrap()),
                    &format!("user-{index}@example.com"),
                    now,
                )
                .unwrap();
        }
        let state = protection.state.lock().unwrap();
        assert_eq!(state.sources.entries.len(), policy.table_entry_limit);
        assert_eq!(state.identifiers.entries.len(), policy.table_entry_limit);
    }

    #[test]
    fn cleanup_queue_eventually_reaches_stale_tail_behind_hot_prefix() {
        let policy = BucketPolicy::new(200, Duration::from_secs(60));
        let now = Instant::now();
        let later = now + Duration::from_secs(11);
        let mut buckets = BoundedBuckets::new(now, policy);

        for key in 0_u16..128 {
            buckets
                .take(key, now, policy, 128, 4, Duration::from_secs(10))
                .unwrap();
        }
        for key in 0_u16..124 {
            buckets.entries.get_mut(&key).unwrap().last_seen = later;
        }
        buckets.cleanup_queue = (0_u16..128).collect();

        for key in 128_u16..159 {
            buckets
                .take(key, later, policy, 128, 4, Duration::from_secs(10))
                .unwrap();
            assert_eq!(buckets.entries.len(), 128);
        }
        buckets
            .take(159, later, policy, 128, 4, Duration::from_secs(10))
            .unwrap();
        for key in 160_u16..163 {
            buckets
                .take(key, later, policy, 128, 4, Duration::from_secs(10))
                .unwrap();
        }

        assert_eq!(buckets.entries.len(), 128);
        assert_eq!(buckets.cleanup_queue.len(), buckets.entries.len());
        assert!(buckets.entries.contains_key(&159));
        assert!(buckets.entries.contains_key(&162));
        for stale in 124_u16..128 {
            assert!(!buckets.entries.contains_key(&stale));
        }
    }

    #[test]
    fn refill_preserves_fractional_interval_progress() {
        let policy = BucketPolicy::new(2, Duration::from_secs(5));
        let now = Instant::now();
        let mut bucket = TokenBucket::full(now, policy);
        assert!(bucket.take(now, policy));
        assert!(bucket.take(now, policy));

        bucket.refill(now + Duration::from_secs(9), policy);
        assert_eq!(bucket.tokens, 1);
        assert_eq!(bucket.last_refill, now + Duration::from_secs(5));

        bucket.refill(now + Duration::from_secs(10), policy);
        assert_eq!(bucket.tokens, 2);
        assert_eq!(bucket.last_refill, now + Duration::from_secs(10));
    }
}
