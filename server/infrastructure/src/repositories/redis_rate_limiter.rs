//! Redis implementation of [`RateLimiter`].
//!
//! Counters are integer keys named `ratelimit:{policy}:{subject hash}:
//! {window start, epoch ms}` with a native TTL of the window plus a small
//! margin, so Redis evicts expired counters itself and `purge_before` is a
//! no-op. A hit is one atomic MULTI/EXEC pipeline (INCR + EXPIRE), so the
//! increment and its expiry cannot be torn apart by a crash.

use application::rate_limit::{
    RateLimitDecision, RateLimitError, RateLimitPolicy, RateLimiter, decide, window_start,
};
use chrono::{DateTime, Utc};
use redis::Pipeline;
use std::time::Duration;

/// How much longer a counter key lives than its window. The margin covers
/// clock skew between nodes so a key is never evicted while its window can
/// still receive hits.
const KEY_TTL_MARGIN: Duration = Duration::from_secs(60);

/// [`RateLimiter`] backed by Redis integer keys with native TTLs.
pub struct RedisRateLimiter {
    client: redis::Client,
}

impl RedisRateLimiter {
    /// Connect to Redis at `url`, failing fast if it is unreachable — the
    /// same fail-fast-when-configured behavior as
    /// [`crate::repositories::RedisSessionRepository`].
    pub fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        // Check one connection (and drop it) so an unreachable Redis fails
        // here, at startup, instead of on the first rate-limit check.
        client.get_connection()?;
        Ok(Self { client })
    }
}

/// The counter key for one (policy, subject, window): the epoch-millisecond
/// window start keeps the keys of different windows distinct. `subject` is
/// already hashed by the service.
fn counter_key(policy: &RateLimitPolicy, subject: &str, now: DateTime<Utc>) -> String {
    let start = window_start(now, policy.window);
    format!(
        "ratelimit:{}:{}:{}",
        policy.name,
        subject,
        start.timestamp_millis()
    )
}

/// Run a blocking Redis operation off the async runtime's worker threads,
/// mirroring [`crate::repositories::redis_session_repository`]. The closure
/// opens its own connection from the (cheaply cloned) client it captures.
async fn run_on_redis<T, F>(op: F) -> Result<T, RateLimitError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RateLimitError> + Send + 'static,
{
    tokio::task::spawn_blocking(op)
        .await
        .map_err(|err| RateLimitError::Store(format!("blocking task failed: {err}")))?
}

#[async_trait::async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn hit(
        &self,
        policy: &RateLimitPolicy,
        subject: &str,
        now: DateTime<Utc>,
    ) -> Result<RateLimitDecision, RateLimitError> {
        let client = self.client.clone();
        // An owned copy: the blocking closure below is 'static and cannot
        // capture the borrowed policy.
        let policy = *policy;
        let key = counter_key(&policy, subject, now);
        // The TTL covers the whole window plus the margin. Refreshing it on
        // every hit is harmless: the window start is in the key's name, so a
        // later window always gets a fresh key.
        let ttl = (policy.window + KEY_TTL_MARGIN).as_secs();
        run_on_redis(move || {
            let mut conn = client
                .get_connection()
                .map_err(|err| RateLimitError::Store(format!("redis error: {err}")))?;
            // One atomic pipeline, one round trip: INCR and its EXPIRE, so a
            // crash cannot leave a counter without its TTL.
            let mut pipe = Pipeline::with_capacity(2);
            pipe.atomic();
            pipe.cmd("INCR").arg(&key);
            pipe.cmd("EXPIRE").arg(&key).arg(ttl);
            let results: Vec<i64> = pipe
                .query(&mut conn)
                .map_err(|err| RateLimitError::Store(format!("redis error: {err}")))?;
            let count = results
                .first()
                .copied()
                .ok_or_else(|| RateLimitError::Store("INCR returned no result".to_owned()))?;
            Ok(decide(count as u64, &policy, now))
        })
        .await
    }

    async fn purge_before(&self, _cutoff: DateTime<Utc>) -> Result<u64, RateLimitError> {
        // Native TTLs evict expired counter keys; there is nothing to delete.
        Ok(0)
    }
}
