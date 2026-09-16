//! Round-trip tests for the Postgres repositories against a real database.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev);
//! each test cleans up after itself, so it is safe to run repeatedly.

use application::ports::{GoalRepository, MilestoneRepository};
use chrono::{DateTime, TimeZone, Utc};
use domain::{Goal, GoalId, GoalStatus, Milestone, MilestoneId, Status, StatusSource};

/// `Utc::now()` has nanosecond precision but Postgres `timestamptz` only
/// stores microseconds, so quantize to milliseconds for exact round-trips.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis()).unwrap()
}
use infrastructure::db::{build_pool, PgPool};
use infrastructure::repositories::{PostgresGoalRepository, PostgresMilestoneRepository};
use uuid::Uuid;

fn pool() -> Option<PgPool> {
    std::env::var("DATABASE_URL").ok().as_deref().map(build_pool)
}

fn test_status() -> GoalStatus {
    GoalStatus {
        status: Status::OnTrack,
        source: StatusSource::Computed,
    }
}

#[tokio::test]
async fn goal_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresGoalRepository::new(pool);
    let now = now();
    let goal = Goal {
        id: GoalId(Uuid::new_v4()),
        title: "Test goal".into(),
        description: Some("round trip".into()),
        status: test_status(),
        target_date: None,
        created_at: now,
        updated_at: now,
    };

    let created = repo.create(goal.clone()).await.unwrap();
    assert_eq!(created.id, goal.id);

    let found = repo.find_by_id(goal.id).await.unwrap().expect("goal to exist");
    assert_eq!(found, goal);
    assert!(repo.list().await.unwrap().iter().any(|g| g.id == goal.id));

    let mut updated = found;
    updated.title = "Updated title".into();
    updated.status.source = StatusSource::ManualOverride;
    repo.update(updated.clone()).await.unwrap();
    assert_eq!(repo.find_by_id(goal.id).await.unwrap().unwrap(), updated);

    repo.delete(goal.id).await.unwrap();
    assert!(repo.find_by_id(goal.id).await.unwrap().is_none());
}

#[tokio::test]
async fn milestone_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresMilestoneRepository::new(pool);
    let now = now();
    let milestone = Milestone {
        id: MilestoneId(Uuid::new_v4()),
        title: "Test milestone".into(),
        description: None,
        status: test_status(),
        target_date: None,
        created_at: now,
        updated_at: now,
    };

    let created = repo.create(milestone.clone()).await.unwrap();
    assert_eq!(created.id, milestone.id);

    let found = repo
        .find_by_id(milestone.id)
        .await
        .unwrap()
        .expect("milestone to exist");
    assert_eq!(found, milestone);
    assert!(repo.list().await.unwrap().iter().any(|m| m.id == milestone.id));

    let mut updated = found;
    updated.title = "Updated title".into();
    repo.update(updated.clone()).await.unwrap();
    assert_eq!(repo.find_by_id(milestone.id).await.unwrap().unwrap(), updated);

    repo.delete(milestone.id).await.unwrap();
    assert!(repo.find_by_id(milestone.id).await.unwrap().is_none());
}
