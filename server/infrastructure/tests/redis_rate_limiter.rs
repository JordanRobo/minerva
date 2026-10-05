//! Round-trip tests for [`RedisRateLimiter`] against a real Redis.
//!
//! Skipped unless `REDIS_URL` is set (compose Redis in local dev); each test
//! uses a fresh subject and cleans up after itself, so it is safe to run
//! repeatedly.

use application::rate_limit::{RateLimitDecision, RateLimitPolicy, RateLimiter, window_start};
use chrono::{DateTime, TimeZone, Utc};
use infrastructure::repositories::RedisRateLimiter;
use uuid::Uuid;

fn limiter() -> Option<RedisRateLimiter> {
    let Some(url) = std::env::var("REDIS_URL").ok() else {
        // In CI these tests must run: a green build that skipped them proves nothing.
        if std::env::var_os("CI").is_some() {
            panic!("REDIS_URL is not set; refusing to skip Redis tests in CI");
        }
        return None;
    };
    match RedisRateLimiter::connect(&url) {
        Ok(limiter) => Some(limiter),
        Err(err) if std::env::var_os("CI").is_some() => {
            panic!("could not connect to Redis at {url}: {err}")
        }
        Err(_) => None,
    }
}

/// A test policy: 10 hits per 60 s window.
const POLICY: RateLimitPolicy = RateLimitPolicy {
    name: "redis_test",
    limit: 10,
    window: std::time::Duration::from_secs(60),
};

/// `Utc::now()` has nanosecond precision; quantize to milliseconds so the
/// fixed-window math is exact.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis())
        .unwrap()
}

/// A fresh subject per test. At the port level subjects are opaque strings;
/// the service hashes them before they reach a store.
fn subject() -> String {
    format!("redis-{}", Uuid::new_v4())
}

#[tokio::test]
async fn concurrent_hits_exactly_reach_the_limit() {
    let Some(limiter) = limiter() else { return };
    let limiter = std::sync::Arc::new(limiter);
    let subject = subject();
    let now = now();

    let mut handles = Vec::new();
    for _ in 0..50 {
        let limiter = limiter.clone();
        let subject = subject.clone();
        handles.push(tokio::spawn(async move {
            limiter.hit(&POLICY, &subject, now).await.expect("hit")
        }));
    }
    let mut allowed = 0;
    for handle in handles {
        if matches!(handle.await.expect("task"), RateLimitDecision::Allowed) {
            allowed += 1;
        }
    }

    assert_eq!(
        allowed, POLICY.limit as usize,
        "the atomic increment must hand out exactly `limit` allowances"
    );
}

#[tokio::test]
async fn counter_keys_carry_a_ttl_that_expires_them() {
    let Some(limiter) = limiter() else { return };
    let subject = subject();
    let at = now();
    limiter.hit(&POLICY, &subject, at).await.expect("hit");

    // Inspect the raw key: a hit is one atomic INCR+EXPIRE pipeline, so after
    // it returns the counter exists with its TTL — as separate commands, a
    // crash mid-hit could leave a counter that never expires.
    let url = std::env::var("REDIS_URL").expect("limiter() connected");
    let mut conn = redis::Client::open(url)
        .expect("client")
        .get_connection()
        .expect("connection");
    let key = format!(
        "ratelimit:{}:{}:{}",
        POLICY.name,
        subject,
        window_start(at, POLICY.window).timestamp_millis()
    );
    let count: i64 = redis::cmd("GET")
        .arg(&key)
        .query(&mut conn)
        .expect("counter value");
    let pttl: i64 = redis::cmd("PTTL")
        .arg(&key)
        .query(&mut conn)
        .expect("pttl of the counter key");
    assert_eq!(count, 1);
    assert!(pttl > 0, "the counter key must carry a TTL, got {pttl}");
    // The TTL is the window plus the adapter's one-minute clock-skew margin,
    // minus the time already elapsed: strictly longer than the window itself.
    assert!(pttl > POLICY.window.as_millis() as i64);
    assert!(pttl <= (POLICY.window + std::time::Duration::from_secs(60)).as_millis() as i64);

    // The purge job is a no-op here: native TTLs evict expired keys.
    assert_eq!(limiter.purge_before(now()).await.expect("purge"), 0);

    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query(&mut conn)
        .expect("cleanup");
}
