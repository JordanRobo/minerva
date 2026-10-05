//! Fixed-window rate limiting (roadmap 2.8).
//!
//! A [`RateLimitPolicy`] caps how many times one *subject* (a client IP, an
//! IP-plus-email pair, an actor id) may hit a protected operation inside each
//! fixed window. Counters live behind the [`RateLimiter`] port — Redis when
//! configured, Postgres otherwise; in-process counters are deliberately not
//! an option because they break horizontal scaling. [`RateLimitService`]
//! hashes subjects before they reach the store and fails open: a broken
//! store must never take login down.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

/// Login attempts per client IP (roadmap 2.8).
pub const LOGIN_IP: RateLimitPolicy = RateLimitPolicy {
    name: "login_ip",
    limit: 30,
    window: Duration::from_secs(15 * 60),
};

/// Login attempts per client-IP-and-email pair (roadmap 2.8).
pub const LOGIN_IP_EMAIL: RateLimitPolicy = RateLimitPolicy {
    name: "login_ip_email",
    limit: 10,
    window: Duration::from_secs(15 * 60),
};

/// Token-link uses (invite/reset links) per client IP (roadmap 2.8).
pub const TOKEN_LINK_IP: RateLimitPolicy = RateLimitPolicy {
    name: "token_link_ip",
    limit: 30,
    window: Duration::from_secs(15 * 60),
};

/// Invite and password-reset issuances per actor (roadmap 2.8).
pub const ADMIN_ISSUE_ACTOR: RateLimitPolicy = RateLimitPolicy {
    name: "admin_issue_actor",
    limit: 60,
    window: Duration::from_secs(60 * 60),
};

/// A named rate-limit rule: at most `limit` hits per fixed `window`, counted
/// separately for every subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Stable identifier, embedded in the store key.
    pub name: &'static str,
    /// Maximum number of allowed hits per window (inclusive).
    pub limit: u32,
    /// Length of the fixed window.
    pub window: Duration,
}

/// The outcome of one hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// The hit is within the limit.
    Allowed,
    /// The hit exceeds the limit; `retry_after` reaches the end of the window.
    Limited { retry_after: Duration },
}

/// An error from a rate-limit store operation.
#[derive(Debug)]
pub enum RateLimitError {
    /// The underlying store (Postgres or Redis) failed. Callers fail open:
    /// they log and allow the request, because rate limiting must not take
    /// the protected operation down with it.
    Store(String),
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitError::Store(detail) => write!(f, "rate limit store error: {detail}"),
        }
    }
}

impl std::error::Error for RateLimitError {}

/// The storage behind rate limiting. Implementations increment the
/// fixed-window counter for (policy, subject) **atomically** — concurrent
/// hits must not interleave between reading and writing the count — and
/// return the decision for the new count. `subject` is already hashed by
/// [`RateLimitService`]; raw subjects never reach a store.
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// Record one hit of `policy` by `subject` at `now` and decide it.
    async fn hit(
        &self,
        policy: &RateLimitPolicy,
        subject: &str,
        now: DateTime<Utc>,
    ) -> Result<RateLimitDecision, RateLimitError>;

    /// Remove counters whose window started before `cutoff`, returning how
    /// many were removed. A store that expires its keys on its own (Redis's
    /// native TTLs) has nothing to delete and returns 0.
    async fn purge_before(&self, cutoff: DateTime<Utc>) -> Result<u64, RateLimitError>;
}

/// The application-facing rate limiter: hashes subjects, applies the
/// enabled switch and fails open on store errors.
pub struct RateLimitService {
    limiter: Arc<dyn RateLimiter>,
    enabled: bool,
}

impl RateLimitService {
    pub fn new(limiter: Arc<dyn RateLimiter>, enabled: bool) -> Self {
        Self { limiter, enabled }
    }

    /// Record one hit of `policy` by `subject` and decide it. The subject is
    /// SHA-256 hashed before it reaches the store, so a raw IP address or
    /// email never touches Postgres or Redis. While disabled this allows
    /// everything without touching the store at all; a store error is logged
    /// at warn and treated as [`RateLimitDecision::Allowed`] (fail open).
    pub async fn hit(
        &self,
        policy: &RateLimitPolicy,
        subject: &str,
        now: DateTime<Utc>,
    ) -> RateLimitDecision {
        if !self.enabled {
            return RateLimitDecision::Allowed;
        }
        let hashed = hash_subject(subject);
        match self.limiter.hit(policy, &hashed, now).await {
            Ok(decision) => decision,
            Err(err) => {
                eprintln!(
                    "warning: rate limit store failed for policy {}; allowing the request: {err}",
                    policy.name
                );
                RateLimitDecision::Allowed
            }
        }
    }
}

/// The SHA-256 of `subject` as lowercase hex — the same one-way hashing the
/// session tokens use, so a leaked store reveals hashed subjects at most.
fn hash_subject(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The start of the fixed window containing `now`: `now` floored to a
/// multiple of the window length. Windows are aligned to the epoch, so every
/// node computes the same boundaries from its own clock.
pub fn window_start(now: DateTime<Utc>, window: Duration) -> DateTime<Utc> {
    let window_ms = window.as_millis().max(1) as i64;
    let start_ms = now.timestamp_millis() / window_ms * window_ms;
    DateTime::from_timestamp_millis(start_ms).expect("a whole-millisecond timestamp is valid")
}

/// How long from `now` until the current window ends.
pub fn retry_after(now: DateTime<Utc>, window: Duration) -> Duration {
    let window_ms = window.as_millis().max(1) as i64;
    let start_ms = now.timestamp_millis() / window_ms * window_ms;
    Duration::from_millis((start_ms + window_ms - now.timestamp_millis()) as u64)
}

/// The decision for a counter that has just been incremented to `count`.
pub fn decide(count: u64, policy: &RateLimitPolicy, now: DateTime<Utc>) -> RateLimitDecision {
    if count > u64::from(policy.limit) {
        RateLimitDecision::Limited {
            retry_after: retry_after(now, policy.window),
        }
    } else {
        RateLimitDecision::Allowed
    }
}

/// The longest of the built-in policy windows. A window that ended before
/// `now - longest_policy_window()` can no longer receive hits under any
/// policy, so its counters may be purged.
pub fn longest_policy_window() -> Duration {
    [LOGIN_IP, LOGIN_IP_EMAIL, TOKEN_LINK_IP, ADMIN_ISSUE_ACTOR]
        .iter()
        .map(|policy| policy.window)
        .max()
        .expect("there is at least one built-in policy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// The counter key, like the real stores': policy name, hashed subject,
    /// window start.
    type CounterKey = (String, String, DateTime<Utc>);

    /// An in-memory fixed-window limiter that records every key it receives,
    /// so the hashing test can inspect what reached the "store".
    #[derive(Default)]
    struct FakeLimiter {
        counters: Mutex<HashMap<CounterKey, u64>>,
        keys_seen: Mutex<Vec<String>>,
        fail: bool,
    }

    impl FakeLimiter {
        fn new(fail: bool) -> Self {
            Self {
                counters: Mutex::new(HashMap::new()),
                keys_seen: Mutex::new(Vec::new()),
                fail,
            }
        }

        fn keys_seen(&self) -> Vec<String> {
            self.keys_seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RateLimiter for FakeLimiter {
        async fn hit(
            &self,
            policy: &RateLimitPolicy,
            subject: &str,
            now: DateTime<Utc>,
        ) -> Result<RateLimitDecision, RateLimitError> {
            if self.fail {
                return Err(RateLimitError::Store("faking a store failure".to_owned()));
            }
            let start = window_start(now, policy.window);
            self.keys_seen.lock().unwrap().push(subject.to_owned());
            let mut counters = self.counters.lock().unwrap();
            let count = counters
                .entry((policy.name.to_owned(), subject.to_owned(), start))
                .or_insert(0);
            *count += 1;
            Ok(decide(*count, policy, now))
        }

        async fn purge_before(&self, cutoff: DateTime<Utc>) -> Result<u64, RateLimitError> {
            let mut counters = self.counters.lock().unwrap();
            let before = counters.len();
            counters.retain(|(_, _, start), _| *start >= cutoff);
            Ok((before - counters.len()) as u64)
        }
    }

    fn service(limiter: FakeLimiter, enabled: bool) -> RateLimitService {
        RateLimitService::new(Arc::new(limiter), enabled)
    }

    /// A small policy for the tests; `now` sits exactly on a 60 s boundary.
    const POLICY: RateLimitPolicy = RateLimitPolicy {
        name: "test",
        limit: 3,
        window: Duration::from_secs(60),
    };
    fn now() -> DateTime<Utc> {
        // A fixed timestamp floored to a 60 s window boundary.
        let ts = 1_000_000i64 / 60 * 60;
        DateTime::from_timestamp(ts, 0).expect("a fixed test timestamp")
    }

    #[test]
    fn the_builtin_policies_carry_the_roadmap_defaults() {
        assert_eq!(
            LOGIN_IP,
            RateLimitPolicy {
                name: "login_ip",
                limit: 30,
                window: Duration::from_secs(15 * 60)
            }
        );
        assert_eq!(
            LOGIN_IP_EMAIL,
            RateLimitPolicy {
                name: "login_ip_email",
                limit: 10,
                window: Duration::from_secs(15 * 60)
            }
        );
        assert_eq!(
            TOKEN_LINK_IP,
            RateLimitPolicy {
                name: "token_link_ip",
                limit: 30,
                window: Duration::from_secs(15 * 60)
            }
        );
        assert_eq!(
            ADMIN_ISSUE_ACTOR,
            RateLimitPolicy {
                name: "admin_issue_actor",
                limit: 60,
                window: Duration::from_secs(60 * 60)
            }
        );
        assert_eq!(longest_policy_window(), Duration::from_secs(60 * 60));
    }

    #[test]
    fn window_start_floors_to_the_window_boundary() {
        let now = now(); // exactly on a 60 s boundary
        assert_eq!(window_start(now, POLICY.window), now);
        assert_eq!(
            window_start(now + chrono::Duration::milliseconds(5_999), POLICY.window),
            now
        );
        // The next millisecond is already in the next window.
        assert_eq!(
            window_start(now + chrono::Duration::seconds(60), POLICY.window),
            now + chrono::Duration::seconds(60)
        );
    }

    #[test]
    fn retry_after_counts_down_to_the_window_end() {
        let now = now();
        assert_eq!(retry_after(now, POLICY.window), Duration::from_secs(60));
        assert_eq!(
            retry_after(now + chrono::Duration::seconds(37), POLICY.window),
            Duration::from_secs(23)
        );
    }

    #[test]
    fn decide_allows_up_to_and_only_up_to_the_limit() {
        let now = now();
        for count in 0..=POLICY.limit as u64 {
            assert_eq!(decide(count, &POLICY, now), RateLimitDecision::Allowed);
        }
        assert_eq!(
            decide(POLICY.limit as u64 + 1, &POLICY, now),
            RateLimitDecision::Limited {
                retry_after: Duration::from_secs(60)
            }
        );
    }

    #[tokio::test]
    async fn hits_up_to_the_limit_are_allowed_and_the_next_is_limited() {
        let service = service(FakeLimiter::new(false), true);
        for _ in 0..POLICY.limit {
            assert_eq!(
                service.hit(&POLICY, "203.0.113.7", now()).await,
                RateLimitDecision::Allowed
            );
        }
        assert_eq!(
            service.hit(&POLICY, "203.0.113.7", now()).await,
            RateLimitDecision::Limited {
                retry_after: Duration::from_secs(60)
            }
        );
    }

    #[tokio::test]
    async fn a_limited_hit_reports_the_time_left_in_the_window() {
        let service = service(FakeLimiter::new(false), true);
        let mid = now() + chrono::Duration::seconds(37);
        for _ in 0..POLICY.limit {
            assert_eq!(
                service.hit(&POLICY, "203.0.113.7", mid).await,
                RateLimitDecision::Allowed
            );
        }
        assert_eq!(
            service.hit(&POLICY, "203.0.113.7", mid).await,
            RateLimitDecision::Limited {
                retry_after: Duration::from_secs(23)
            }
        );
    }

    #[tokio::test]
    async fn a_new_window_resets_the_counter() {
        let service = service(FakeLimiter::new(false), true);
        for _ in 0..=POLICY.limit {
            service.hit(&POLICY, "203.0.113.7", now()).await;
        }
        assert!(matches!(
            service.hit(&POLICY, "203.0.113.7", now()).await,
            RateLimitDecision::Limited { .. }
        ));
        // One full window later the same subject is allowed again.
        assert_eq!(
            service
                .hit(
                    &POLICY,
                    "203.0.113.7",
                    now() + chrono::Duration::seconds(60)
                )
                .await,
            RateLimitDecision::Allowed
        );
    }

    #[tokio::test]
    async fn different_subjects_get_separate_counters() {
        let service = service(FakeLimiter::new(false), true);
        for _ in 0..POLICY.limit {
            service.hit(&POLICY, "203.0.113.7", now()).await;
        }
        assert!(matches!(
            service.hit(&POLICY, "203.0.113.7", now()).await,
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(
            service.hit(&POLICY, "198.51.100.8", now()).await,
            RateLimitDecision::Allowed
        );
    }

    #[tokio::test]
    async fn a_disabled_service_always_allows_without_touching_the_store() {
        let limiter = Arc::new(FakeLimiter::new(false));
        let service = RateLimitService::new(limiter.clone(), false);
        for _ in 0..=POLICY.limit + 1 {
            assert_eq!(
                service.hit(&POLICY, "203.0.113.7", now()).await,
                RateLimitDecision::Allowed
            );
        }
        assert!(limiter.keys_seen().is_empty(), "no key may reach the store");
    }

    #[tokio::test]
    async fn a_store_failure_fails_open() {
        let service = service(FakeLimiter::new(true), true);
        assert_eq!(
            service.hit(&POLICY, "203.0.113.7", now()).await,
            RateLimitDecision::Allowed
        );
    }

    #[tokio::test]
    async fn the_raw_subject_never_reaches_the_store() {
        let raw = "203.0.113.7";
        let limiter = Arc::new(FakeLimiter::new(false));
        let service = RateLimitService::new(limiter.clone(), true);
        service.hit(&POLICY, raw, now()).await;

        let keys = limiter.keys_seen();
        assert_eq!(keys, vec![hash_subject(raw)]);
        assert!(!keys.contains(&raw.to_owned()), "the raw subject leaked");
        // The hash is a 64-character lowercase hex string.
        assert_eq!(keys[0].len(), 64);
        assert!(
            keys[0]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
