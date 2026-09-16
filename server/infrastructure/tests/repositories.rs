//! Round-trip tests for the Postgres repositories against a real database.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev);
//! each test cleans up after itself, so it is safe to run repeatedly.

use application::ports::{
    GoalMilestoneRepository, GoalRepository, MilestoneRepository, ProgressSnapshotRepository,
    TaskRelationRepository, TaskRepository,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    Goal, GoalId, GoalMilestone, GoalStatus, Milestone, MilestoneId, ProgressSnapshot,
    ProgressTarget, Status, StatusSource, Task, TaskId, TaskRelation, TaskRelationType,
    TaskStatus,
};

/// `Utc::now()` has nanosecond precision but Postgres `timestamptz` only
/// stores microseconds, so quantize to milliseconds for exact round-trips.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis()).unwrap()
}
use infrastructure::db::{build_pool, PgPool};
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
};
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

fn test_goal() -> Goal {
    let now = now();
    Goal {
        id: GoalId(Uuid::new_v4()),
        title: "Test goal".into(),
        description: None,
        status: test_status(),
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_milestone() -> Milestone {
    let now = now();
    Milestone {
        id: MilestoneId(Uuid::new_v4()),
        title: "Test milestone".into(),
        description: None,
        status: test_status(),
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_task(milestone_id: Option<MilestoneId>) -> Task {
    let now = now();
    Task {
        id: TaskId(Uuid::new_v4()),
        milestone_id,
        title: "Test task".into(),
        description: None,
        status: TaskStatus::Backlog,
        target_date: None,
        created_at: now,
        updated_at: now,
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

#[tokio::test]
async fn goal_milestone_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let links = PostgresGoalMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();

    links
        .link(GoalMilestone::new(goal.id, milestone.id))
        .await
        .unwrap();
    assert_eq!(links.milestones_for_goal(goal.id).await.unwrap(), vec![milestone.id]);
    assert_eq!(links.goals_for_milestone(milestone.id).await.unwrap(), vec![goal.id]);

    links.unlink(goal.id, milestone.id).await.unwrap();
    assert!(links.milestones_for_goal(goal.id).await.unwrap().is_empty());
    assert!(links.goals_for_milestone(milestone.id).await.unwrap().is_empty());

    goals.delete(goal.id).await.unwrap();
    milestones.delete(milestone.id).await.unwrap();
}

#[tokio::test]
async fn task_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let milestone = test_milestone();
    milestones.create(milestone.clone()).await.unwrap();

    let task = test_task(None);
    repo.create(task.clone()).await.unwrap();

    let found = repo.find_by_id(task.id).await.unwrap().expect("task to exist");
    assert_eq!(found, task);
    assert!(repo.list_unassigned().await.unwrap().iter().any(|t| t.id == task.id));

    // Assign the task to the milestone and move it along the board.
    let mut updated = found;
    updated.milestone_id = Some(milestone.id);
    updated.status = TaskStatus::InProgress;
    repo.update(updated.clone()).await.unwrap();
    assert_eq!(repo.find_by_id(task.id).await.unwrap().unwrap(), updated);
    assert!(repo.list_by_milestone(milestone.id).await.unwrap().iter().any(|t| t.id == task.id));
    assert!(!repo.list_unassigned().await.unwrap().iter().any(|t| t.id == task.id));

    repo.delete(task.id).await.unwrap();
    assert!(repo.find_by_id(task.id).await.unwrap().is_none());
    milestones.delete(milestone.id).await.unwrap();
}

#[tokio::test]
async fn task_relation_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let source = test_task(None);
    let target = test_task(None);
    tasks.create(source.clone()).await.unwrap();
    tasks.create(target.clone()).await.unwrap();

    let relation = TaskRelation::new(
        source.id,
        target.id,
        TaskRelationType::Blocks,
        now(),
    );
    repo.create(relation.clone()).await.unwrap();

    // The task appears in the listing whether it is the source or the
    // target of the relation.
    assert_eq!(repo.list_for_task(source.id).await.unwrap(), vec![relation.clone()]);
    assert_eq!(repo.list_for_task(target.id).await.unwrap(), vec![relation.clone()]);

    repo.delete(relation.id).await.unwrap();
    assert!(repo.list_for_task(source.id).await.unwrap().is_empty());
    assert!(repo.list_for_task(target.id).await.unwrap().is_empty());

    tasks.delete(source.id).await.unwrap();
    tasks.delete(target.id).await.unwrap();
}

#[tokio::test]
async fn progress_snapshot_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresProgressSnapshotRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();

    // Two snapshots for the goal, a second one recorded later, so the
    // listing order (oldest-to-newest) is observable.
    let first = ProgressSnapshot::new(
        ProgressTarget::Goal(goal.id),
        now(),
        Status::OnTrack,
        42,
        None,
    )
    .unwrap();
    let second = ProgressSnapshot::new(
        ProgressTarget::Goal(goal.id),
        first.recorded_at + Duration::seconds(1),
        Status::AtRisk,
        75,
        Some("slipping".into()),
    )
    .unwrap();
    repo.create(first.clone()).await.unwrap();
    repo.create(second.clone()).await.unwrap();

    let for_goal = repo.list_for_target(ProgressTarget::Goal(goal.id)).await.unwrap();
    assert_eq!(for_goal, vec![first, second]);

    // Snapshots of one target never leak into another target's listing.
    let for_milestone = ProgressSnapshot::new(
        ProgressTarget::Milestone(milestone.id),
        now(),
        Status::Complete,
        100,
        None,
    )
    .unwrap();
    repo.create(for_milestone.clone()).await.unwrap();
    assert_eq!(
        repo.list_for_target(ProgressTarget::Milestone(milestone.id))
            .await
            .unwrap(),
        vec![for_milestone]
    );

    goals.delete(goal.id).await.unwrap();
    milestones.delete(milestone.id).await.unwrap();
}
