//! Postgres-backed behaviour tests for the account token repository
//! (roadmap 2.6, step 1).
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev);
//! each test cleans up after itself, so it is safe to run repeatedly.

use application::ports::{
    AcceptInviteOutcome, AccountTokenRepository, RepositoryError, ResetPasswordOutcome,
    UserRepository,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use diesel::prelude::*;
use domain::{
    AccountToken, AccountTokenId, AccountTokenKind, AccountTokenStatus, Role, User, UserId,
};

/// `Utc::now()` has nanosecond precision but Postgres `timestamptz` only
/// stores microseconds, so quantize to milliseconds for exact round-trips.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis())
        .unwrap()
}
use infrastructure::db::PgPool;
use infrastructure::repositories::{PostgresAccountTokenRepository, PostgresUserRepository};
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
    // One connection per test, for the same reason as in repositories.rs:
    // every test here uses a single connection at a time.
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

/// A fresh email no other test or account can collide with.
fn fresh_email() -> String {
    format!("invite-{}@example.com", Uuid::new_v4())
}

fn test_invite(email: &str, role: Role, expires_in: Duration) -> AccountToken {
    let now = now();
    AccountToken {
        id: AccountTokenId::new(),
        kind: AccountTokenKind::Invite {
            email: email.to_owned(),
            role,
        },
        token_hash: format!("hash-{}", Uuid::new_v4()),
        created_by: None,
        created_at: now,
        expires_at: now + expires_in,
        consumed_at: None,
        revoked_at: None,
    }
}

fn test_reset(user_id: UserId, expires_in: Duration) -> AccountToken {
    let now = now();
    AccountToken {
        id: AccountTokenId::new(),
        kind: AccountTokenKind::PasswordReset { user_id },
        token_hash: format!("hash-{}", Uuid::new_v4()),
        created_by: None,
        created_at: now,
        expires_at: now + expires_in,
        consumed_at: None,
        revoked_at: None,
    }
}

fn test_user(email: &str, role: Role) -> User {
    let now = now();
    User {
        id: UserId::new(),
        email: email.to_owned(),
        password_hash: Some("hashed-password".to_owned()),
        display_name: "Token test user".into(),
        role,
        deactivated_at: None,
        created_at: now,
        updated_at: now,
    }
}

/// Remove the rows a test created, so repeated runs start clean. Tokens are
/// deleted before their users; the `user_id` foreign key would cascade
/// either way.
fn delete_rows(pool: &PgPool, tokens: &[AccountTokenId], users: &[UserId]) {
    let mut conn = pool.get().expect("pool connection for cleanup");
    for id in tokens {
        diesel::delete(infrastructure::schema::account_tokens::table.find(id.0))
            .execute(&mut conn)
            .expect("delete token row");
    }
    for id in users {
        diesel::delete(infrastructure::schema::users::table.find(id.0))
            .execute(&mut conn)
            .expect("delete user row");
    }
}

#[tokio::test]
async fn issue_and_find_round_trip_both_kinds() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    // Invite.
    let email = fresh_email();
    let invite = test_invite(&email, Role::Staff, Duration::hours(24));
    let issued = repo
        .issue(invite.clone(), now())
        .await
        .expect("issue invite");
    assert_eq!(issued, invite);
    let by_hash = repo
        .find_by_token_hash(invite.token_hash.clone())
        .await
        .expect("find by hash")
        .expect("invite found by hash");
    assert_eq!(by_hash, invite);
    let by_id = repo
        .find_by_id(invite.id)
        .await
        .expect("find by id")
        .expect("invite found by id");
    assert_eq!(by_id, invite);

    // Password reset.
    let user = test_user(&fresh_email(), Role::ReadOnly);
    users.create(user.clone()).await.expect("create user");
    let reset = test_reset(user.id, Duration::hours(1));
    repo.issue(reset.clone(), now()).await.expect("issue reset");
    let by_hash = repo
        .find_by_token_hash(reset.token_hash.clone())
        .await
        .expect("find by hash")
        .expect("reset found by hash");
    assert_eq!(by_hash, reset);
    let by_id = repo
        .find_by_id(reset.id)
        .await
        .expect("find by id")
        .expect("reset found by id");
    assert_eq!(by_id, reset);

    delete_rows(&pool, &[invite.id, reset.id], &[user.id]);
}

#[tokio::test]
async fn issue_replaces_live_token_for_same_subject() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    // Invites: re-issuing for the same email revokes the old token instead
    // of violating the unique index.
    let email = fresh_email();
    let first = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(first.clone(), now()).await.expect("first issue");
    let second = test_invite(&email, Role::Admin, Duration::hours(24));
    repo.issue(second.clone(), now())
        .await
        .expect("re-issue does not violate the unique index");
    assert_eq!(
        repo.find_by_id(first.id)
            .await
            .unwrap()
            .expect("first exists")
            .status(now()),
        AccountTokenStatus::Revoked
    );
    assert_eq!(
        repo.find_by_id(second.id)
            .await
            .unwrap()
            .expect("second exists")
            .status(now()),
        AccountTokenStatus::Pending
    );

    // An expired-but-unconsumed invite still occupies its email: replacing
    // it must revoke it, not hit the unique index.
    let stale_email = fresh_email();
    let stale = test_invite(&stale_email, Role::Staff, Duration::hours(-1));
    repo.issue(stale.clone(), now())
        .await
        .expect("issue an already-expired invite");
    let replacement = test_invite(&stale_email, Role::Staff, Duration::hours(24));
    repo.issue(replacement.clone(), now())
        .await
        .expect("replace the expired invite");
    assert_eq!(
        repo.find_by_id(stale.id)
            .await
            .unwrap()
            .expect("stale exists")
            .status(now()),
        AccountTokenStatus::Revoked
    );
    assert_eq!(
        repo.find_by_id(replacement.id)
            .await
            .unwrap()
            .expect("replacement exists")
            .status(now()),
        AccountTokenStatus::Pending
    );

    // Resets: the same rule for the same user.
    let user = test_user(&fresh_email(), Role::ReadOnly);
    users.create(user.clone()).await.expect("create user");
    let first_reset = test_reset(user.id, Duration::hours(1));
    repo.issue(first_reset.clone(), now())
        .await
        .expect("first reset issue");
    let second_reset = test_reset(user.id, Duration::hours(1));
    repo.issue(second_reset.clone(), now())
        .await
        .expect("second reset issue");
    assert_eq!(
        repo.find_by_id(first_reset.id)
            .await
            .unwrap()
            .expect("first reset exists")
            .status(now()),
        AccountTokenStatus::Revoked
    );
    assert_eq!(
        repo.find_by_id(second_reset.id)
            .await
            .unwrap()
            .expect("second reset exists")
            .status(now()),
        AccountTokenStatus::Pending
    );

    delete_rows(
        &pool,
        &[
            first.id,
            second.id,
            stale.id,
            replacement.id,
            first_reset.id,
            second_reset.id,
        ],
        &[user.id],
    );
}

#[tokio::test]
async fn find_pending_invite_ignores_expired_revoked_and_consumed() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let email = fresh_email();

    // Expired: a token past its expiry is not pending.
    let expired = test_invite(&email, Role::Staff, Duration::hours(-1));
    repo.issue(expired.clone(), now())
        .await
        .expect("issue expired invite");
    assert!(
        repo.find_pending_invite_for_email(email.clone(), now())
            .await
            .unwrap()
            .is_none()
    );

    // Revoked.
    let revoked = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(revoked.clone(), now())
        .await
        .expect("issue live invite");
    assert!(
        repo.find_pending_invite_for_email(email.clone(), now())
            .await
            .unwrap()
            .is_some()
    );
    repo.revoke(revoked.id, now()).await.expect("revoke");
    assert!(
        repo.find_pending_invite_for_email(email.clone(), now())
            .await
            .unwrap()
            .is_none()
    );

    // Consumed.
    let consumed = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(consumed.clone(), now())
        .await
        .expect("issue live invite");
    assert!(repo.consume(consumed.id, now()).await.expect("consume"));
    assert!(
        repo.find_pending_invite_for_email(email.clone(), now())
            .await
            .unwrap()
            .is_none()
    );

    // A live invite is still found, and the lookup normalizes the email the
    // same way storage does (trim + lowercase).
    let live = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(live.clone(), now())
        .await
        .expect("issue live invite");
    let found = repo
        .find_pending_invite_for_email(format!("  {}  ", email.to_uppercase()), now())
        .await
        .unwrap()
        .expect("live invite found");
    assert_eq!(found.id, live.id);

    delete_rows(&pool, &[expired.id, revoked.id, consumed.id, live.id], &[]);
}

#[tokio::test]
async fn revoke_is_idempotent_and_not_found_for_unknown_id() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());

    let token = test_invite(&fresh_email(), Role::Staff, Duration::hours(24));
    repo.issue(token.clone(), now()).await.expect("issue");
    repo.revoke(token.id, now()).await.expect("first revoke");
    repo.revoke(token.id, now())
        .await
        .expect("revoking a revoked token is a no-op");
    assert_eq!(
        repo.find_by_id(token.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Revoked
    );

    // Revoking an already consumed token is a no-op as well.
    let consumed = test_invite(&fresh_email(), Role::Staff, Duration::hours(24));
    repo.issue(consumed.clone(), now()).await.expect("issue");
    assert!(repo.consume(consumed.id, now()).await.expect("consume"));
    repo.revoke(consumed.id, now())
        .await
        .expect("revoking a consumed token is a no-op");

    let error = repo
        .revoke(AccountTokenId::new(), now())
        .await
        .expect_err("unknown id must not succeed");
    assert!(matches!(error, RepositoryError::NotFound));

    delete_rows(&pool, &[token.id, consumed.id], &[]);
}

#[tokio::test]
async fn consume_returns_true_once_then_false() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());

    let token = test_invite(&fresh_email(), Role::Staff, Duration::hours(24));
    repo.issue(token.clone(), now()).await.expect("issue");
    assert!(repo.consume(token.id, now()).await.expect("first consume"));
    assert!(!repo.consume(token.id, now()).await.expect("second consume"));
    // An unknown id claims nothing either.
    assert!(
        !repo
            .consume(AccountTokenId::new(), now())
            .await
            .expect("unknown id")
    );

    delete_rows(&pool, &[token.id], &[]);
}

#[tokio::test]
async fn accept_invite_creates_user_and_consumes_token() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    let email = fresh_email();
    let invite = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(invite.clone(), now()).await.expect("issue");

    let user = test_user(&email, Role::Staff);
    let created_id = user.id;
    match repo
        .accept_invite(invite.id, user, now())
        .await
        .expect("accept")
    {
        AcceptInviteOutcome::Accepted(created) => assert_eq!(created.email, email),
        other => panic!("expected Accepted, got {other:?}"),
    }

    let stored = users
        .find_by_id(created_id)
        .await
        .expect("find created user")
        .expect("user was created");
    assert_eq!(stored.role, Role::Staff);
    assert_eq!(
        repo.find_by_id(invite.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Accepted
    );

    delete_rows(&pool, &[invite.id], &[created_id]);
}

#[tokio::test]
async fn accept_invite_with_taken_email_leaves_token_pending() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    let email = fresh_email();
    let existing = test_user(&email, Role::ReadOnly);
    users.create(existing.clone()).await.expect("create user");

    let invite = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(invite.clone(), now()).await.expect("issue");

    let challenger = test_user(&email, Role::Staff);
    match repo
        .accept_invite(invite.id, challenger, now())
        .await
        .expect("accept")
    {
        AcceptInviteOutcome::EmailTaken => {}
        other => panic!("expected EmailTaken, got {other:?}"),
    }

    // The token was not burned by the failed attempt...
    assert_eq!(
        repo.find_by_id(invite.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Pending
    );
    // ...and no second account appeared.
    let matches = users
        .list()
        .await
        .unwrap()
        .iter()
        .filter(|u| u.email == email)
        .count();
    assert_eq!(matches, 1);

    delete_rows(&pool, &[invite.id], &[existing.id]);
}

#[tokio::test]
async fn accept_invite_with_unusable_token_creates_no_user() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    // Three invites, each made unusable in a different way.
    let email_consumed = fresh_email();
    let consumed = test_invite(&email_consumed, Role::Staff, Duration::hours(24));
    repo.issue(consumed.clone(), now()).await.expect("issue");
    assert!(repo.consume(consumed.id, now()).await.expect("consume"));

    let email_expired = fresh_email();
    let expired = test_invite(&email_expired, Role::Staff, Duration::hours(-1));
    repo.issue(expired.clone(), now()).await.expect("issue");

    let email_revoked = fresh_email();
    let revoked = test_invite(&email_revoked, Role::Staff, Duration::hours(24));
    repo.issue(revoked.clone(), now()).await.expect("issue");
    repo.revoke(revoked.id, now()).await.expect("revoke");

    for (token, email) in [
        (consumed.clone(), &email_consumed),
        (expired.clone(), &email_expired),
        (revoked.clone(), &email_revoked),
    ] {
        let user = test_user(email, Role::Staff);
        match repo
            .accept_invite(token.id, user, now())
            .await
            .expect("accept")
        {
            AcceptInviteOutcome::TokenUnusable => {}
            other => panic!(
                "expected TokenUnusable for a {:?} token, got {other:?}",
                token.status(now())
            ),
        }
    }

    let emails = [email_consumed, email_expired, email_revoked];
    let all = users.list().await.unwrap();
    assert!(!all.iter().any(|u| emails.contains(&u.email)));

    delete_rows(&pool, &[consumed.id, expired.id, revoked.id], &[]);
}

#[tokio::test]
async fn reset_password_updates_hash_consumes_token_and_revokes_siblings() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    let user = test_user(&fresh_email(), Role::ReadOnly);
    users.create(user.clone()).await.expect("create user");

    let first = test_reset(user.id, Duration::hours(1));
    repo.issue(first.clone(), now()).await.expect("first issue");
    let second = test_reset(user.id, Duration::hours(1));
    repo.issue(second.clone(), now())
        .await
        .expect("second issue revokes the first");

    // The unique index normally keeps at most one live reset per user, so a
    // still-live sibling can only exist if something bypassed `issue` (e.g.
    // rows from before the index existed). Drop the index briefly to create
    // that state and prove the reset revokes the sibling anyway; it is
    // restored at the end of the test. A run killed in between would leave
    // it missing, so drop it unconditionally up front as well.
    let mut conn = pool.get().expect("pool connection");
    diesel::sql_query("DROP INDEX IF EXISTS idx_account_tokens_live_reset_user_id")
        .execute(&mut conn)
        .expect("drop the live-reset unique index");
    diesel::update(infrastructure::schema::account_tokens::table.find(first.id.0))
        .set(infrastructure::schema::account_tokens::revoked_at.eq(None::<DateTime<Utc>>))
        .execute(&mut conn)
        .expect("re-open the sibling token");
    drop(conn);

    match repo
        .reset_password(second.id, user.id, "new-hash".to_owned(), now())
        .await
        .expect("reset")
    {
        ResetPasswordOutcome::Done => {}
        other => panic!("expected Done, got {other:?}"),
    }

    let stored = users
        .find_by_id(user.id)
        .await
        .unwrap()
        .expect("user exists");
    assert_eq!(stored.password_hash.as_deref(), Some("new-hash"));
    assert_eq!(
        repo.find_by_id(second.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Accepted
    );
    assert_eq!(
        repo.find_by_id(first.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Revoked
    );

    // The reset revoked the sibling, so the table is in a valid state again
    // and the index can be restored.
    delete_rows(&pool, &[first.id, second.id], &[user.id]);
    let mut conn = pool.get().expect("pool connection");
    diesel::sql_query(
        "CREATE UNIQUE INDEX idx_account_tokens_live_reset_user_id \
         ON account_tokens (user_id) \
         WHERE purpose = 'password_reset' AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .execute(&mut conn)
    .expect("restore the live-reset unique index");
}

#[tokio::test]
async fn concurrent_accept_invite_yields_exactly_one_success() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    let email = fresh_email();
    let invite = test_invite(&email, Role::Staff, Duration::hours(24));
    repo.issue(invite.clone(), now()).await.expect("issue");

    // Two callers race on the same token with two different new user ids but
    // the same email; exactly one may win.
    let user_a = test_user(&email, Role::Staff);
    let user_b = test_user(&email, Role::Staff);
    let (result_a, result_b) = tokio::join!(
        repo.accept_invite(invite.id, user_a, now()),
        repo.accept_invite(invite.id, user_b, now()),
    );
    let accepted = [result_a.expect("accept a"), result_b.expect("accept b")]
        .into_iter()
        .filter(|outcome| matches!(outcome, AcceptInviteOutcome::Accepted(_)))
        .count();
    assert_eq!(accepted, 1, "exactly one accept may win");

    let all = users.list().await.unwrap();
    let created: Vec<&User> = all.iter().filter(|u| u.email == email).collect();
    assert_eq!(created.len(), 1);
    assert_eq!(
        repo.find_by_id(invite.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Accepted
    );

    delete_rows(&pool, &[invite.id], &[created[0].id]);
}

#[tokio::test]
async fn concurrent_reset_password_yields_exactly_one_success() {
    let Some(pool) = pool() else { return };
    let repo = PostgresAccountTokenRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    let user = test_user(&fresh_email(), Role::ReadOnly);
    users.create(user.clone()).await.expect("create user");
    let token = test_reset(user.id, Duration::hours(1));
    repo.issue(token.clone(), now()).await.expect("issue");

    let (result_a, result_b) = tokio::join!(
        repo.reset_password(token.id, user.id, "hash-a".to_owned(), now()),
        repo.reset_password(token.id, user.id, "hash-b".to_owned(), now()),
    );
    let done = [result_a.expect("reset a"), result_b.expect("reset b")]
        .into_iter()
        .filter(|outcome| matches!(outcome, ResetPasswordOutcome::Done))
        .count();
    assert_eq!(done, 1, "exactly one reset may win");

    let stored = users
        .find_by_id(user.id)
        .await
        .unwrap()
        .expect("user exists");
    assert!(matches!(
        stored.password_hash.as_deref(),
        Some("hash-a") | Some("hash-b")
    ));
    assert_eq!(
        repo.find_by_id(token.id)
            .await
            .unwrap()
            .expect("exists")
            .status(now()),
        AccountTokenStatus::Accepted
    );

    delete_rows(&pool, &[token.id], &[user.id]);
}
