//! Round-trip tests for [`RedisSessionRepository`] against a real Redis.
//!
//! Skipped unless `REDIS_URL` is set (compose Redis in local dev); each test
//! uses a fresh user id and cleans up after itself, so it is safe to run
//! repeatedly.

use application::ports::{RepositoryError, SessionRepository};
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{Session, SessionId, UserId};
use infrastructure::repositories::RedisSessionRepository;
use uuid::Uuid;

fn repo() -> Option<RedisSessionRepository> {
    let Some(url) = std::env::var("REDIS_URL").ok() else {
        // In CI these tests must run: a green build that skipped them proves nothing.
        if std::env::var_os("CI").is_some() {
            panic!("REDIS_URL is not set; refusing to skip Redis tests in CI");
        }
        return None;
    };
    match RedisSessionRepository::connect(&url) {
        Ok(repo) => Some(repo),
        Err(err) if std::env::var_os("CI").is_some() => {
            panic!("could not connect to Redis at {url}: {err}")
        }
        Err(_) => None,
    }
}

/// `Utc::now()` has nanosecond precision; quantize to milliseconds so
/// round-trip comparisons are exact.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis())
        .unwrap()
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
    let Some(repo) = repo() else { return };
    let user_id = UserId(Uuid::new_v4());
    let session = test_session(user_id, 3600);

    let created = repo.create(session.clone()).await.expect("create");
    assert_eq!(created.id, session.id);

    let found = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find")
        .expect("session should exist");
    assert_eq!(found, session);

    let listed = repo.list_for_user(user_id).await.expect("list");
    assert_eq!(listed, vec![session.clone()]);

    let touched_at = now();
    let new_expiry = touched_at + Duration::hours(2);
    repo.touch_last_seen(session.id, touched_at, new_expiry)
        .await
        .expect("touch");
    let touched = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find after touch")
        .expect("session should still exist");
    assert_eq!(touched.last_seen_at, touched_at);
    assert_eq!(touched.expires_at, new_expiry);

    // A second session of the same user: delete removes only the first.
    let other = test_session(user_id, 3600);
    repo.create(other.clone()).await.expect("create other");
    repo.delete(session.id).await.expect("delete");
    assert!(
        repo.find_by_token_hash(session.token_hash.clone())
            .await
            .expect("find deleted")
            .is_none()
    );
    assert_eq!(
        repo.list_for_user(user_id)
            .await
            .expect("list after delete"),
        vec![other.clone()]
    );

    repo.delete_all_for_user(user_id).await.expect("delete all");
    assert!(
        repo.find_by_token_hash(other.token_hash.clone())
            .await
            .expect("find after delete all")
            .is_none()
    );
}

#[tokio::test]
async fn expired_session_is_evicted_by_redis_ttl() {
    let Some(repo) = repo() else { return };
    let user_id = UserId(Uuid::new_v4());
    let session = test_session(user_id, 1);
    repo.create(session.clone()).await.expect("create");

    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(
        repo.find_by_token_hash(session.token_hash.clone())
            .await
            .expect("find expired")
            .is_none()
    );
}

#[tokio::test]
async fn touch_missing_session_is_not_found() {
    let Some(repo) = repo() else { return };
    let missing = SessionId(Uuid::new_v4());

    assert!(matches!(
        repo.touch_last_seen(missing, now(), now() + Duration::hours(2))
            .await,
        Err(RepositoryError::NotFound)
    ));
}

#[tokio::test]
async fn touch_refreshes_the_ttl_so_a_session_survives_its_old_expiry() {
    let Some(repo) = repo() else { return };
    let user_id = UserId(Uuid::new_v4());
    // A session Redis would evict in a second...
    let session = test_session(user_id, 1);
    repo.create(session.clone()).await.expect("create");

    // ...that a touch slides well into the future. Without the TTL refresh
    // the key would still be evicted at its original expiry.
    let touched_at = now();
    let new_expiry = touched_at + Duration::hours(2);
    repo.touch_last_seen(session.id, touched_at, new_expiry)
        .await
        .expect("touch");

    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let surviving = repo
        .find_by_token_hash(session.token_hash.clone())
        .await
        .expect("find after the old expiry")
        .expect("a touched session must survive its original TTL");
    assert_eq!(surviving.expires_at, new_expiry);
}

#[tokio::test]
async fn touch_evicted_session_is_not_found() {
    let Some(repo) = repo() else { return };
    let user_id = UserId(Uuid::new_v4());
    let session = test_session(user_id, 1);
    repo.create(session.clone()).await.expect("create");

    // Once Redis has evicted the expired key, a late touch is a no-op.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(matches!(
        repo.touch_last_seen(session.id, now(), now() + Duration::hours(2))
            .await,
        Err(RepositoryError::NotFound)
    ));
}
