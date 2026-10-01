//! Startup migration tests: idempotence and concurrent-startup safety.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev).

use infrastructure::db::{build_pool, PgPool};
use infrastructure::migrations::run_migrations;

fn pool() -> Option<PgPool> {
    std::env::var("DATABASE_URL").ok().as_deref().map(build_pool)
}

#[test]
fn second_run_applies_nothing() {
    let Some(pool) = pool() else { return };
    run_migrations(&pool).expect("first run should apply migrations");
    let applied = run_migrations(&pool).expect("second run should succeed");
    assert!(applied.is_empty(), "expected no pending migrations, got {applied:?}");
}

#[test]
fn concurrent_runs_all_succeed() {
    let Some(pool) = pool() else { return };
    // Simulates several API nodes starting at once against the same database.
    let handles = (0..4)
        .map(|_| {
            let pool = pool.clone();
            std::thread::spawn(move || run_migrations(&pool))
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle
            .join()
            .expect("migration thread panicked")
            .expect("concurrent migration run failed");
    }
}
