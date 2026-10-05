//! Postgres-backed behaviour tests for the SSO group rule repository
//! (roadmap 2.7, step 2).
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev);
//! each test cleans up after itself, so it is safe to run repeatedly.

use application::ports::{RepositoryError, SsoGroupRuleRepository};
use chrono::{DateTime, TimeZone, Utc};
use diesel::prelude::*;
use domain::{Role, SsoGroupRule, SsoGroupRuleId};
use infrastructure::db::PgPool;
use infrastructure::repositories::PostgresSsoGroupRuleRepository;
use uuid::Uuid;

/// `Utc::now()` has nanosecond precision but Postgres `timestamptz` only
/// stores microseconds, so quantize to milliseconds for exact round-trips.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis())
        .unwrap()
}

/// Set once the migrations have been applied by [`pool`], so the tests are
/// self-sufficient against a fresh database.
static MIGRATIONS_APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Serializes the tests in this file: `any_exist` is a predicate over the
/// whole table, and libtest runs tests on parallel threads, so two tests
/// creating rules at once would see each other's rows. A tokio mutex because
/// the guard is held across await points (a std one trips clippy).
static TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

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

/// A fresh group name no other test or rule can collide with.
fn fresh_group_name() -> String {
    format!("group-{}", Uuid::new_v4())
}

fn test_rule(group_name: &str, role: Role) -> SsoGroupRule {
    let now = now();
    SsoGroupRule {
        id: SsoGroupRuleId::new(),
        group_name: group_name.to_owned(),
        role,
        created_at: now,
        updated_at: now,
    }
}

/// Remove the rows a test created, so repeated runs start clean.
fn delete_rows(pool: &PgPool, ids: &[SsoGroupRuleId]) {
    let mut conn = pool.get().expect("pool connection for cleanup");
    for id in ids {
        diesel::delete(infrastructure::schema::sso_group_role_rules::table.find(id.0))
            .execute(&mut conn)
            .expect("delete rule row");
    }
}

#[tokio::test]
async fn create_and_find_round_trip() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    let rule = test_rule(&fresh_group_name(), Role::Staff);
    let created = repo.create(rule.clone()).await.expect("create");
    assert_eq!(created, rule);
    let found = repo
        .find_by_id(rule.id)
        .await
        .expect("find by id")
        .expect("rule found by id");
    assert_eq!(found, rule);

    delete_rows(&pool, &[rule.id]);
}

#[tokio::test]
async fn list_orders_by_group_name() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    // Inserted out of order.
    let zeta = test_rule(&format!("z-{}", fresh_group_name()), Role::Admin);
    let alpha = test_rule(&format!("a-{}", fresh_group_name()), Role::ReadOnly);
    let mid = test_rule(&format!("m-{}", fresh_group_name()), Role::Staff);
    repo.create(zeta.clone()).await.expect("create zeta");
    repo.create(alpha.clone()).await.expect("create alpha");
    repo.create(mid.clone()).await.expect("create mid");

    let listed = repo.list().await.expect("list");
    assert_eq!(
        listed.iter().map(|rule| rule.id).collect::<Vec<_>>(),
        [alpha.id, mid.id, zeta.id]
    );

    delete_rows(&pool, &[zeta.id, alpha.id, mid.id]);
}

#[tokio::test]
async fn duplicate_group_name_is_a_conflict() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    let first = test_rule(&fresh_group_name(), Role::Staff);
    repo.create(first.clone()).await.expect("create");

    // A second rule for the same name is a typed conflict, not a raw error.
    let duplicate = SsoGroupRule {
        id: SsoGroupRuleId::new(),
        ..first.clone()
    };
    let error = repo.create(duplicate).await.expect_err("duplicate name");
    assert!(matches!(error, RepositoryError::Conflict(_)));

    // ...and so is renaming an existing rule onto a taken name.
    let other = test_rule(&fresh_group_name(), Role::Admin);
    repo.create(other.clone()).await.expect("create other");
    let renamed = SsoGroupRule {
        group_name: first.group_name.clone(),
        ..other.clone()
    };
    let error = repo.update(renamed).await.expect_err("taken name");
    assert!(matches!(error, RepositoryError::Conflict(_)));

    delete_rows(&pool, &[first.id, other.id]);
}

#[tokio::test]
async fn update_changes_rule_and_not_found_for_unknown_id() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    let rule = test_rule(&fresh_group_name(), Role::ReadOnly);
    repo.create(rule.clone()).await.expect("create");

    let updated = SsoGroupRule {
        group_name: fresh_group_name(),
        role: Role::Admin,
        updated_at: now(),
        ..rule.clone()
    };
    let stored = repo.update(updated.clone()).await.expect("update");
    assert_eq!(stored, updated);
    let found = repo
        .find_by_id(rule.id)
        .await
        .expect("find after update")
        .expect("rule still exists");
    assert_eq!(found, updated);

    let error = repo
        .update(SsoGroupRule {
            id: SsoGroupRuleId::new(),
            ..updated
        })
        .await
        .expect_err("unknown id must not succeed");
    assert!(matches!(error, RepositoryError::NotFound));

    delete_rows(&pool, &[rule.id]);
}

#[tokio::test]
async fn delete_removes_rule_and_not_found_when_gone() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    let rule = test_rule(&fresh_group_name(), Role::Staff);
    repo.create(rule.clone()).await.expect("create");
    repo.delete(rule.id).await.expect("delete");
    assert!(repo.find_by_id(rule.id).await.unwrap().is_none());

    let error = repo.delete(rule.id).await.expect_err("already deleted");
    assert!(matches!(error, RepositoryError::NotFound));
}

#[tokio::test]
async fn any_exist_flips_with_rules() {
    let _guard = TEST_LOCK.lock().await;
    let Some(pool) = pool() else { return };
    let repo = PostgresSsoGroupRuleRepository::new(pool.clone());

    // The lock above guarantees no other test in this file holds a row.
    assert!(!repo.any_exist().await.expect("any_exist"));
    let rule = test_rule(&fresh_group_name(), Role::Staff);
    repo.create(rule.clone()).await.expect("create");
    assert!(repo.any_exist().await.expect("any_exist"));
    repo.delete(rule.id).await.expect("delete");
    assert!(!repo.any_exist().await.expect("any_exist"));
}
