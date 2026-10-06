//! Round-trip tests for [`PostgresRateLimiter`] against a real database.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev); each
//! test uses a fresh subject and cleans up after itself, so it is safe to run
//! repeatedly.

use application::rate_limit::{RateLimitDecision, RateLimitPolicy, RateLimiter};
use chrono::{DateTime, TimeZone, Utc};
use diesel::prelude::*;
use infrastructure::db::PgPool;
use infrastructure::repositories::PostgresRateLimiter;
use uuid::Uuid;

/// Set once the migrations have been applied by [`pool`], so the tests are
/// self-sufficient against a fresh database.
static MIGRATIONS_APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn pool() -> Option<PgPool> {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        // In CI these tests must run: a green build that skipped them proves nothing.
        if std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip Postgres tests in CI");
        }
        return None;
    };
    // One connection per test, like the other Postgres test binaries: the
    // concurrency test below builds its own wider pool.
    let pool = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
        .expect("could not create test pool");
    MIGRATIONS_APPLIED.get_or_init(|| {
        infrastructure::migrations::run_migrations(&pool)
            .expect("could not apply migrations in tests");
    });
    Some(pool)
}

/// A test policy: 10 hits per 60 s window.
const POLICY: RateLimitPolicy = RateLimitPolicy {
    name: "pg_test",
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
    format!("pg-{}", Uuid::new_v4())
}

fn key(subject: &str) -> String {
    format!("{}:{}", POLICY.name, subject)
}

/// Delete one test's counter rows directly (the limiter has no per-key
/// delete).
fn delete_subject(pool: &PgPool, subject: &str) {
    let mut conn = pool.get().expect("pool connection");
    diesel::delete(
        infrastructure::schema::rate_limit_counters::table
            .filter(infrastructure::schema::rate_limit_counters::key.eq(key(subject))),
    )
    .execute(&mut conn)
    .expect("delete counters");
}

/// How many counter rows exist for one key, across all windows.
fn row_count(pool: &PgPool, subject: &str) -> i64 {
    let mut conn = pool.get().expect("pool connection");
    infrastructure::schema::rate_limit_counters::table
        .filter(infrastructure::schema::rate_limit_counters::key.eq(key(subject)))
        .count()
        .first(&mut conn)
        .expect("count rows")
}

#[tokio::test]
async fn concurrent_hits_exactly_reach_the_limit() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        if std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip Postgres tests in CI");
        }
        return;
    };
    // Fifty real connections: with a one-connection pool the tasks would
    // serialize and the test could not catch a torn read-modify-write. The
    // other test binaries hold at most a handful of connections each, so this
    // stays under local Postgres's `max_connections` (100).
    let pool = diesel::r2d2::Pool::builder()
        .max_size(50)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
        .expect("could not create test pool");
    MIGRATIONS_APPLIED.get_or_init(|| {
        infrastructure::migrations::run_migrations(&pool)
            .expect("could not apply migrations in tests");
    });
    let limiter = std::sync::Arc::new(PostgresRateLimiter::new(pool.clone()));
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
        "the atomic upsert must hand out exactly `limit` allowances"
    );
    delete_subject(&pool, &subject);
}

#[tokio::test]
async fn purge_before_removes_only_old_windows() {
    let Some(pool) = pool() else { return };
    let limiter = PostgresRateLimiter::new(pool.clone());
    let subject = subject();
    let now = now();
    // One counter two full windows old, one in the current window.
    limiter
        .hit(&POLICY, &subject, now - chrono::Duration::seconds(120))
        .await
        .expect("old hit");
    limiter
        .hit(&POLICY, &subject, now)
        .await
        .expect("current hit");

    // The purge is table-wide and other tests running in parallel may hold
    // their own old rows at the same moment, so only a lower bound on the
    // count — the per-row check below carries the "only old windows" rule.
    let removed = limiter
        .purge_before(now - chrono::Duration::seconds(60))
        .await
        .expect("purge");
    assert!(
        removed >= 1,
        "at least this test's old counter must be gone"
    );
    assert_eq!(
        row_count(&pool, &subject),
        1,
        "only the current window's counter may survive"
    );

    delete_subject(&pool, &subject);
}
