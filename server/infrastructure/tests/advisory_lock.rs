//! Tests for the session-level advisory-lock helper the maintenance jobs use
//! to serialize across nodes (roadmap 2.8).
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev); no
//! tables are involved — only the lock itself.

use infrastructure::db::{PgPool, try_acquire_advisory_lock};

/// "MNVRLOCK" read as ASCII, like the other advisory lock keys in the
/// deployment; distinct from every real one so the tests never contend with
/// a running server.
const TEST_LOCK_KEY: i64 = 0x4D4E5652_4C4F434B;

fn pool() -> Option<PgPool> {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        // In CI these tests must run: a green build that skipped them proves nothing.
        if std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip Postgres tests in CI");
        }
        return None;
    };
    // Two connections: one holds the lock while the other tries to take it.
    Some(
        diesel::r2d2::Pool::builder()
            .max_size(2)
            .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
            .expect("could not create test pool"),
    )
}

#[tokio::test]
async fn a_held_lock_is_not_taken_twice_and_drop_releases_it() {
    let Some(pool) = pool() else { return };

    // The first node takes the lock...
    let first = try_acquire_advisory_lock(&pool, TEST_LOCK_KEY)
        .await
        .expect("first acquire");
    assert!(first.is_some(), "an uncontended lock must be taken");

    // ...and a second node's try-lock skips instead of waiting or blocking.
    let second = try_acquire_advisory_lock(&pool, TEST_LOCK_KEY)
        .await
        .expect("second acquire");
    assert!(second.is_none(), "a held lock must not be taken twice");

    // Dropping the guard releases the lock for the next node.
    drop(first);
    let third = try_acquire_advisory_lock(&pool, TEST_LOCK_KEY)
        .await
        .expect("third acquire");
    assert!(third.is_some(), "dropping the guard must release the lock");
}
