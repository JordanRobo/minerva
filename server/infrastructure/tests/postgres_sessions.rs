//! Round-trip tests for [`PostgresSessionRepository`] against a real
//! database.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev); each
//! test uses a fresh user and cleans up after itself, so it is safe to run
//! repeatedly.

use application::ports::{RepositoryError, SessionRepository, UserRepository};
use chrono::{DateTime, Duration, TimeZone, Utc};
use diesel::prelude::*;
use domain::{Session, SessionId, User, UserId};
use infrastructure::db::PgPool;
use infrastructure::repositories::{PostgresSessionRepository, PostgresUserRepository};
use uuid::Uuid;

/// Set once the migrations have been applied by [`pool`], so the tests are
/// self-sufficient against a fresh database.
static MIGRATIONS_APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn pool() -> Option<PgPool> {
    std::env::var("DATABASE_URL").ok().as_deref().map(|url| {
        // One connection per test: the default settings (max_size 10,
        // min_idle = max_size) times every parallel test binary would exceed
        // local Postgres's `max_connections` (see oidc.rs's test_pool). Every
        // test here uses a single connection at a time.
        let pool = diesel::r2d2::Pool::builder()
            .max_size(1)
            .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(url))
            .expect("could not create test pool");
        MIGRATIONS_APPLIED.get_or_init(|| {
            infrastructure::migrations::run_migrations(&pool)
                .expect("could not apply migrations in tests");
        });
        pool
    })
}

/// `Utc::now()` has nanosecond precision; quantize to milliseconds so
/// round-trip comparisons are exact.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis()).unwrap()
}

/// Sessions reference users, so every test that creates one starts from a
/// real user row.
async fn create_user(pool: &PgPool) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = now();
    let user = User {
        id: UserId::new(),
        email: format!("sessions-{}@example.com", Uuid::new_v4()),
        password_hash: None,
        display_name: "Session test user".into(),
        created_at: now,
        updated_at: now,
    };
    users.create(user).await.expect("create user")
}

/// Delete the user row directly (the repository has no delete); its sessions
/// cascade-delete with it.
fn delete_user(pool: &PgPool, user_id: UserId) {
    let mut conn = pool.get().expect("pool connection");
    diesel::delete(infrastructure::schema::users::table.find(user_id.0))
        .execute(&mut conn)
        .expect("delete user");
}

fn test_session(user_id: UserId, ttl_seconds: i64) -> Session {
    let created_at = now();
    Session {
        id: SessionId(Uuid::new_v4()),
        user_id,
        token_hash: format!("test-{}", Uuid::new_v4()),
        created_at,
        expires_at: created_at + Duration::seconds(ttl_seconds),
        last_seen_at: created_at,
    }
}

#[tokio::test]
async fn session_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresSessionRepository::new(pool.clone());
    let user = create_user(&pool).await;
    let session = test_session(user.id, 3600);

    let created = repo.create(session.clone()).await.expect("create");
    assert_eq!(created.id, session.id);

    let found = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find")
        .expect("session should exist");
    assert_eq!(found, session);

    let listed = repo.list_for_user(user.id).await.expect("list");
    assert_eq!(listed, vec![session.clone()]);

    let touched_at = now();
    repo.touch_last_seen(session.id, touched_at)
        .await
        .expect("touch");
    let touched = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find after touch")
        .expect("session should still exist");
    assert_eq!(touched.last_seen_at, touched_at);

    // A second session of the same user: delete removes only the first.
    let other = test_session(user.id, 3600);
    repo.create(other.clone()).await.expect("create other");
    repo.delete(session.id).await.expect("delete");
    assert!(repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find deleted")
        .is_none());
    assert_eq!(
        repo.list_for_user(user.id).await.expect("list after delete"),
        vec![other.clone()]
    );

    repo.delete_all_for_user(user.id).await.expect("delete all");
    assert!(repo
        .find_by_token_hash(other.token_hash.clone())
        .await
        .expect("find after delete all")
        .is_none());

    delete_user(&pool, user.id);
}

#[tokio::test]
async fn delete_and_touch_missing_session_are_not_found() {
    let Some(pool) = pool() else { return };
    let repo = PostgresSessionRepository::new(pool.clone());
    let missing = SessionId(Uuid::new_v4());

    assert!(matches!(
        repo.delete(missing).await,
        Err(RepositoryError::NotFound)
    ));
    assert!(matches!(
        repo.touch_last_seen(missing, now()).await,
        Err(RepositoryError::NotFound)
    ));
}

#[tokio::test]
async fn find_by_token_hash_unknown_hash_is_none() {
    let Some(pool) = pool() else { return };
    let repo = PostgresSessionRepository::new(pool.clone());

    assert!(repo
        .find_by_token_hash(format!("test-{}", Uuid::new_v4()))
        .await
        .expect("find")
        .is_none());
}

#[tokio::test]
async fn expired_session_is_still_returned() {
    let Some(pool) = pool() else { return };
    let repo = PostgresSessionRepository::new(pool.clone());
    let user = create_user(&pool).await;
    // Negative TTL: the session is already past its expiry.
    let session = test_session(user.id, -3600);
    repo.create(session.clone()).await.expect("create");

    // The repository does not filter on expiry — enforcement is the caller's
    // job via `Session::is_expired` (see the `AuthenticatedUser` extractor).
    let found = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find")
        .expect("expired session should still be returned");
    assert_eq!(found, session);
    assert!(found.is_expired(now()));

    delete_user(&pool, user.id);
}
