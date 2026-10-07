//! Round-trip tests for the Postgres repositories against a real database.
//!
//! Skipped unless `DATABASE_URL` is set (compose Postgres in local dev);
//! each test cleans up after itself, so it is safe to run repeatedly.

use application::ports::{
    AccessChange, GoalMilestoneRepository, GoalRepository, MilestoneRepository,
    ProgressSnapshotRepository, RepositoryError, TaskRelationCreateError, TaskRelationRepository,
    TaskRepository, UserIdentityRepository, UserRepository,
};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use domain::{
    Goal, GoalId, GoalMilestone, Milestone, MilestoneId, ProgressSnapshot, ProgressTarget, Role,
    Status, StatusSource, Task, TaskId, TaskRelation, TaskRelationId, TaskRelationType, TaskStatus,
    User, UserId, UserIdentity,
};

/// `Utc::now()` has nanosecond precision but Postgres `timestamptz` only
/// stores microseconds, so quantize to milliseconds for exact round-trips.
fn now() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(Utc::now().timestamp_millis())
        .unwrap()
}
use infrastructure::db::PgPool;
use infrastructure::repositories::mapping::{TaskRelationRow, task_relation_from_row};
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
    PostgresUserIdentityRepository, PostgresUserRepository,
};
use infrastructure::schema::{task_relations, tasks};
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
    // One connection per test: the default settings (max_size 10,
    // min_idle = max_size) times every parallel test binary would exceed
    // local Postgres's `max_connections` (see oidc.rs's test_pool). Every
    // test here uses a single connection at a time.
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

fn test_goal() -> Goal {
    let now = now();
    Goal {
        id: GoalId(Uuid::new_v4()),
        title: "Test goal".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
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
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

/// A fixed timestamp in 2026, exact to the second so Postgres round-trips it.
fn at(month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, 9, 0, 0).unwrap()
}

fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

/// A milestone with a fixed id, target date and created_at for ordering tests.
fn milestone_on(id: u128, target_date: Option<NaiveDate>, created_at: DateTime<Utc>) -> Milestone {
    let mut milestone = test_milestone();
    milestone.id = MilestoneId(Uuid::from_u128(id));
    milestone.target_date = target_date;
    milestone.created_at = created_at;
    milestone.updated_at = created_at;
    milestone
}

/// A goal with a fixed id, target date and created_at for ordering tests.
fn goal_on(id: u128, target_date: Option<NaiveDate>, created_at: DateTime<Utc>) -> Goal {
    let mut goal = test_goal();
    goal.id = GoalId(Uuid::from_u128(id));
    goal.target_date = target_date;
    goal.created_at = created_at;
    goal.updated_at = created_at;
    goal
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
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    };

    let created = repo.create(goal.clone()).await.unwrap();
    assert_eq!(created.id, goal.id);

    let found = repo
        .find_by_id(goal.id)
        .await
        .unwrap()
        .expect("goal to exist");
    assert_eq!(found, goal);
    assert!(repo.list().await.unwrap().iter().any(|g| g.id == goal.id));

    let mut updated = found;
    updated.title = "Updated title".into();
    updated.apply_computed_status(Status::AtRisk);
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
        status: Status::OnTrack,
        status_override: None,
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
    assert!(
        repo.list()
            .await
            .unwrap()
            .iter()
            .any(|m| m.id == milestone.id)
    );

    let mut updated = found;
    updated.title = "Updated title".into();
    repo.update(updated.clone()).await.unwrap();
    assert_eq!(
        repo.find_by_id(milestone.id).await.unwrap().unwrap(),
        updated
    );

    repo.delete(milestone.id).await.unwrap();
    assert!(repo.find_by_id(milestone.id).await.unwrap().is_none());
}

#[tokio::test]
async fn goal_status_override_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresGoalRepository::new(pool);
    let now = now();
    let goal = Goal {
        id: GoalId(Uuid::new_v4()),
        title: "Override goal".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    };
    repo.create(goal.clone()).await.unwrap();

    // Setting an override persists and reloads; the automatic status is untouched.
    repo.set_status_override(goal.id, Some(Status::AtRisk))
        .await
        .unwrap();
    let found = repo
        .find_by_id(goal.id)
        .await
        .unwrap()
        .expect("goal to exist");
    assert_eq!(found.status_override, Some(Status::AtRisk));
    assert_eq!(found.status, Status::OnTrack);
    assert_eq!(found.effective_status(), Status::AtRisk);
    assert_eq!(found.status_source(), StatusSource::ManualOverride);

    // A normal update of other fields (including the automatic status) leaves
    // the override intact.
    let mut updated = found;
    updated.title = "Updated title".into();
    updated.apply_computed_status(Status::OffTrack);
    repo.update(updated).await.unwrap();
    let after_update = repo
        .find_by_id(goal.id)
        .await
        .unwrap()
        .expect("goal to exist");
    assert_eq!(after_update.status_override, Some(Status::AtRisk));
    assert_eq!(after_update.status, Status::OffTrack);
    assert_eq!(after_update.effective_status(), Status::AtRisk);

    // Clearing persists NULL and returns the goal to its automatic status.
    repo.set_status_override(goal.id, None).await.unwrap();
    let cleared = repo
        .find_by_id(goal.id)
        .await
        .unwrap()
        .expect("goal to exist");
    assert_eq!(cleared.status_override, None);
    assert_eq!(cleared.status, Status::OffTrack);
    assert_eq!(cleared.effective_status(), Status::OffTrack);
    assert_eq!(cleared.status_source(), StatusSource::Computed);

    repo.delete(goal.id).await.unwrap();
}

#[tokio::test]
async fn milestone_status_override_round_trip() {
    let Some(pool) = pool() else { return };
    let repo = PostgresMilestoneRepository::new(pool);
    let now = now();
    let milestone = Milestone {
        id: MilestoneId(Uuid::new_v4()),
        title: "Override milestone".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    };
    repo.create(milestone.clone()).await.unwrap();

    // Setting an override persists and reloads; the automatic status is untouched.
    repo.set_status_override(milestone.id, Some(Status::AtRisk))
        .await
        .unwrap();
    let found = repo
        .find_by_id(milestone.id)
        .await
        .unwrap()
        .expect("milestone to exist");
    assert_eq!(found.status_override, Some(Status::AtRisk));
    assert_eq!(found.status, Status::OnTrack);
    assert_eq!(found.effective_status(), Status::AtRisk);
    assert_eq!(found.status_source(), StatusSource::ManualOverride);

    // A normal update of other fields (including the automatic status) leaves
    // the override intact.
    let mut updated = found;
    updated.title = "Updated title".into();
    updated.apply_computed_status(Status::OffTrack);
    repo.update(updated).await.unwrap();
    let after_update = repo
        .find_by_id(milestone.id)
        .await
        .unwrap()
        .expect("milestone to exist");
    assert_eq!(after_update.status_override, Some(Status::AtRisk));
    assert_eq!(after_update.status, Status::OffTrack);
    assert_eq!(after_update.effective_status(), Status::AtRisk);

    // Clearing persists NULL and returns the milestone to its automatic status.
    repo.set_status_override(milestone.id, None).await.unwrap();
    let cleared = repo
        .find_by_id(milestone.id)
        .await
        .unwrap()
        .expect("milestone to exist");
    assert_eq!(cleared.status_override, None);
    assert_eq!(cleared.status, Status::OffTrack);
    assert_eq!(cleared.effective_status(), Status::OffTrack);
    assert_eq!(cleared.status_source(), StatusSource::Computed);

    repo.delete(milestone.id).await.unwrap();
}

#[tokio::test]
async fn status_override_check_rejects_unknown_values() {
    let Some(pool) = pool() else { return };
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();

    for (table, id) in [("goals", goal.id.0), ("milestones", milestone.id.0)] {
        let mut conn = pool.get().expect("pool connection");
        let err = diesel::sql_query(format!(
            "UPDATE {table} SET status_override = 'not_a_status' WHERE id = '{id}'"
        ))
        .execute(&mut conn)
        .expect_err("the CHECK constraint must reject an unknown status string");
        assert!(
            err.to_string().contains("check"),
            "expected a check-constraint violation, got: {err}"
        );
    }

    // The rejected writes left both rows without an override.
    assert_eq!(
        goals
            .find_by_id(goal.id)
            .await
            .unwrap()
            .expect("goal to exist")
            .status_override,
        None
    );
    assert_eq!(
        milestones
            .find_by_id(milestone.id)
            .await
            .unwrap()
            .expect("milestone to exist")
            .status_override,
        None
    );

    goals.delete(goal.id).await.unwrap();
    milestones.delete(milestone.id).await.unwrap();
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
    let goal_id = goal.id;
    let milestone_id = milestone.id;

    assert!(
        links
            .link(GoalMilestone::new(goal_id, milestone_id))
            .await
            .unwrap()
    );
    // The listing returns the full entity, not just the id.
    let listed = links.milestones_for_goal(goal_id).await.unwrap();
    assert_eq!(listed, vec![milestone]);
    let back = links.goals_for_milestone(milestone_id).await.unwrap();
    assert_eq!(back, vec![goal]);

    assert!(links.unlink(goal_id, milestone_id).await.unwrap());
    assert!(links.milestones_for_goal(goal_id).await.unwrap().is_empty());
    assert!(
        links
            .goals_for_milestone(milestone_id)
            .await
            .unwrap()
            .is_empty()
    );

    goals.delete(goal_id).await.unwrap();
    milestones.delete(milestone_id).await.unwrap();
}

#[tokio::test]
async fn duplicate_goal_milestone_link_is_a_no_op_leaving_one_row() {
    let Some(pool) = pool() else { return };
    let links = PostgresGoalMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();

    // A repeated link is not an error; the second reports nothing created.
    assert!(
        links
            .link(GoalMilestone::new(goal.id, milestone.id))
            .await
            .unwrap()
    );
    assert!(
        !links
            .link(GoalMilestone::new(goal.id, milestone.id))
            .await
            .unwrap()
    );
    assert_eq!(links.milestones_for_goal(goal.id).await.unwrap().len(), 1);

    goals.delete(goal.id).await.unwrap();
    milestones.delete(milestone.id).await.unwrap();
}

#[tokio::test]
async fn concurrent_duplicate_goal_milestone_links_yield_exactly_one_row() {
    let Some(pool) = pool() else { return };
    // A second single-connection pool so both inserts are genuinely in flight
    // at once; the shared `pool()` allows only one connection.
    let url = std::env::var("DATABASE_URL").expect("checked by pool()");
    let other = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
        .expect("could not create second test pool");

    let links_a = PostgresGoalMilestoneRepository::new(pool.clone());
    let links_b = PostgresGoalMilestoneRepository::new(other);
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();

    let pair = GoalMilestone::new(goal.id, milestone.id);
    let (created_a, created_b) = tokio::join!(links_a.link(pair.clone()), links_b.link(pair));
    // Exactly one of the racers created the row; neither errored.
    assert_eq!(created_a.unwrap() as u8 + created_b.unwrap() as u8, 1);
    assert_eq!(links_a.milestones_for_goal(goal.id).await.unwrap().len(), 1);

    goals.delete(goal.id).await.unwrap();
    milestones.delete(milestone.id).await.unwrap();
}

#[tokio::test]
async fn deleting_a_goal_or_milestone_cascades_its_links() {
    let Some(pool) = pool() else { return };
    let links = PostgresGoalMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    // Without ON DELETE CASCADE the delete itself would fail; a surviving
    // link row would still join to the milestone in the listing below.
    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();
    links
        .link(GoalMilestone::new(goal.id, milestone.id))
        .await
        .unwrap();

    goals.delete(goal.id).await.unwrap();
    assert!(links.milestones_for_goal(goal.id).await.unwrap().is_empty());
    assert!(
        links
            .goals_for_milestone(milestone.id)
            .await
            .unwrap()
            .is_empty()
    );
    milestones.delete(milestone.id).await.unwrap();

    // And the other direction.
    let goal = test_goal();
    let milestone = test_milestone();
    goals.create(goal.clone()).await.unwrap();
    milestones.create(milestone.clone()).await.unwrap();
    links
        .link(GoalMilestone::new(goal.id, milestone.id))
        .await
        .unwrap();

    milestones.delete(milestone.id).await.unwrap();
    assert!(links.milestones_for_goal(goal.id).await.unwrap().is_empty());
    assert!(
        links
            .goals_for_milestone(milestone.id)
            .await
            .unwrap()
            .is_empty()
    );
    goals.delete(goal.id).await.unwrap();
}

#[tokio::test]
async fn milestones_for_goal_is_ordered_by_target_date_created_at_id() {
    let Some(pool) = pool() else { return };
    let links = PostgresGoalMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let goal = test_goal();
    goals.create(goal.clone()).await.unwrap();
    // Same date and created_at: the id decides. A later date sorts before an
    // earlier-created undated one. Null target dates sort last, by
    // created_at.
    let m_first = milestone_on(1, Some(date(1, 5)), at(1, 3));
    let m_second = milestone_on(2, Some(date(1, 5)), at(1, 3));
    let m_third = milestone_on(3, Some(date(1, 10)), at(1, 1));
    let m_fourth = milestone_on(4, None, at(1, 1));
    let m_fifth = milestone_on(5, None, at(1, 9));
    for milestone in [&m_first, &m_second, &m_third, &m_fourth, &m_fifth] {
        milestones.create(milestone.clone()).await.unwrap();
        links
            .link(GoalMilestone::new(goal.id, milestone.id))
            .await
            .unwrap();
    }

    let listed = links.milestones_for_goal(goal.id).await.unwrap();

    assert_eq!(
        listed.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![m_first.id, m_second.id, m_third.id, m_fourth.id, m_fifth.id]
    );

    goals.delete(goal.id).await.unwrap();
    for milestone in [&m_first, &m_second, &m_third, &m_fourth, &m_fifth] {
        milestones.delete(milestone.id).await.unwrap();
    }
}

#[tokio::test]
async fn goals_for_milestone_is_ordered_the_same_way() {
    let Some(pool) = pool() else { return };
    let links = PostgresGoalMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool);

    let milestone = test_milestone();
    milestones.create(milestone.clone()).await.unwrap();
    let g_undated = goal_on(1, None, at(1, 2));
    let g_late = goal_on(2, Some(date(1, 20)), at(1, 5));
    let g_early = goal_on(3, Some(date(1, 1)), at(1, 9));
    for goal in [&g_undated, &g_late, &g_early] {
        goals.create(goal.clone()).await.unwrap();
        links
            .link(GoalMilestone::new(goal.id, milestone.id))
            .await
            .unwrap();
    }

    let listed = links.goals_for_milestone(milestone.id).await.unwrap();

    assert_eq!(
        listed.iter().map(|g| g.id).collect::<Vec<_>>(),
        vec![g_early.id, g_late.id, g_undated.id]
    );

    goals.delete(g_undated.id).await.unwrap();
    goals.delete(g_late.id).await.unwrap();
    goals.delete(g_early.id).await.unwrap();
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

    let found = repo
        .find_by_id(task.id)
        .await
        .unwrap()
        .expect("task to exist");
    assert_eq!(found, task);

    // A batched lookup returns only the ids that exist, in id order.
    let other = test_task(None);
    repo.create(other.clone()).await.unwrap();
    let (first, second) = if task.id.0 <= other.id.0 {
        (task.clone(), other.clone())
    } else {
        (other.clone(), task.clone())
    };
    assert_eq!(
        repo.find_by_ids(&[second.id, first.id, TaskId::new()])
            .await
            .unwrap(),
        vec![first, second]
    );

    assert!(
        repo.list_unassigned()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == task.id)
    );

    // Assign the task to the milestone and move it along the board.
    let mut updated = found;
    updated.milestone_id = Some(milestone.id);
    updated.status = TaskStatus::InProgress;
    repo.update(updated.clone()).await.unwrap();
    assert_eq!(repo.find_by_id(task.id).await.unwrap().unwrap(), updated);
    assert!(
        repo.list_by_milestone(milestone.id)
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == task.id)
    );
    assert!(
        !repo
            .list_unassigned()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == task.id)
    );

    repo.delete(task.id).await.unwrap();
    assert!(repo.find_by_id(task.id).await.unwrap().is_none());
    repo.delete(other.id).await.unwrap();
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

    let relation = TaskRelation::new(source.id, target.id, TaskRelationType::Blocks, now());
    assert_eq!(repo.create(relation.clone()).await.unwrap(), relation);

    // find_by_id returns the stored row, and None for an unknown id.
    assert_eq!(
        repo.find_by_id(relation.id).await.unwrap(),
        Some(relation.clone())
    );
    assert!(
        repo.find_by_id(TaskRelationId::new())
            .await
            .unwrap()
            .is_none()
    );

    // The task appears in the listing whether it is the source or the
    // target of the relation.
    assert_eq!(
        repo.list_for_task(source.id).await.unwrap(),
        vec![relation.clone()]
    );
    assert_eq!(
        repo.list_for_task(target.id).await.unwrap(),
        vec![relation.clone()]
    );

    // delete reports whether a row was removed.
    assert!(repo.delete(relation.id).await.unwrap());
    assert!(!repo.delete(relation.id).await.unwrap());
    assert!(repo.list_for_task(source.id).await.unwrap().is_empty());
    assert!(repo.list_for_task(target.id).await.unwrap().is_empty());

    tasks.delete(source.id).await.unwrap();
    tasks.delete(target.id).await.unwrap();
}

#[tokio::test]
async fn duplicate_blocks_relation_is_rejected() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    let first = TaskRelation::new(a.id, b.id, TaskRelationType::Blocks, now());
    repo.create(first.clone()).await.unwrap();
    assert!(matches!(
        repo.create(TaskRelation::new(
            a.id,
            b.id,
            TaskRelationType::Blocks,
            now()
        ))
        .await,
        Err(TaskRelationCreateError::Duplicate)
    ));
    assert_eq!(repo.list_for_task(a.id).await.unwrap(), vec![first]);

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn reversed_blocks_relation_is_rejected() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    repo.create(TaskRelation::new(
        a.id,
        b.id,
        TaskRelationType::Blocks,
        now(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        repo.create(TaskRelation::new(
            b.id,
            a.id,
            TaskRelationType::Blocks,
            now()
        ))
        .await,
        Err(TaskRelationCreateError::ReverseExists)
    ));

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn normalised_blocked_by_colliding_with_an_existing_blocks_row_is_rejected() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    repo.create(TaskRelation::new(
        a.id,
        b.id,
        TaskRelationType::Blocks,
        now(),
    ))
    .await
    .unwrap();
    // "b is blocked by a" normalises to the same canonical row: blocks(a -> b).
    let (source, target, kind) =
        TaskRelation::canonical_form(b.id, a.id, TaskRelationType::BlockedBy);
    assert_eq!(
        (source, target, kind),
        (a.id, b.id, TaskRelationType::Blocks)
    );
    assert!(matches!(
        repo.create(TaskRelation::new(source, target, kind, now()))
            .await,
        Err(TaskRelationCreateError::Duplicate)
    ));

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn relates_to_in_either_order_yields_one_row_and_the_second_is_a_duplicate() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    // Both submission orders normalise to the same canonical row...
    let (s1, t1, k1) = TaskRelation::canonical_form(a.id, b.id, TaskRelationType::RelatesTo);
    let (s2, t2, k2) = TaskRelation::canonical_form(b.id, a.id, TaskRelationType::RelatesTo);
    assert_eq!((s1, t1, k1), (s2, t2, k2));
    // ...so the second create is a duplicate.
    repo.create(TaskRelation::new(s1, t1, k1, now()))
        .await
        .unwrap();
    assert!(matches!(
        repo.create(TaskRelation::new(s2, t2, k2, now())).await,
        Err(TaskRelationCreateError::Duplicate)
    ));
    assert_eq!(repo.list_for_task(a.id).await.unwrap().len(), 1);

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn blocks_and_relates_to_between_the_same_tasks_coexist() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    repo.create(TaskRelation::new(
        a.id,
        b.id,
        TaskRelationType::Blocks,
        now(),
    ))
    .await
    .unwrap();
    let (source, target, kind) =
        TaskRelation::canonical_form(a.id, b.id, TaskRelationType::RelatesTo);
    repo.create(TaskRelation::new(source, target, kind, now()))
        .await
        .unwrap();

    // One row per type: both endpoints list two relations.
    assert_eq!(repo.list_for_task(a.id).await.unwrap().len(), 2);
    assert_eq!(repo.list_for_task(b.id).await.unwrap().len(), 2);

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn self_relation_is_rejected() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    tasks.create(a.clone()).await.unwrap();

    assert!(matches!(
        repo.create(TaskRelation::new(
            a.id,
            a.id,
            TaskRelationType::Blocks,
            now()
        ))
        .await,
        Err(TaskRelationCreateError::SelfRelation)
    ));
    assert!(repo.list_for_task(a.id).await.unwrap().is_empty());

    tasks.delete(a.id).await.unwrap();
}

#[tokio::test]
async fn concurrent_opposite_blocks_creates_leave_exactly_one_row() {
    let Some(pool) = pool() else { return };
    // A second single-connection pool so both inserts are genuinely in flight
    // at once; the shared `pool()` allows only one connection.
    let url = std::env::var("DATABASE_URL").expect("checked by pool()");
    let other = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
        .expect("could not create second test pool");

    let repo_a = PostgresTaskRelationRepository::new(pool.clone());
    let repo_b = PostgresTaskRelationRepository::new(other);
    let tasks = PostgresTaskRepository::new(pool);

    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();

    let (first, second) = tokio::join!(
        repo_a.create(TaskRelation::new(
            a.id,
            b.id,
            TaskRelationType::Blocks,
            now()
        )),
        repo_b.create(TaskRelation::new(
            b.id,
            a.id,
            TaskRelationType::Blocks,
            now()
        )),
    );
    // Exactly one racer won; the loser read the committed row and was told it
    // reversed an existing blocks relation.
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    for outcome in &outcomes {
        if let Err(error) = outcome {
            assert!(matches!(error, TaskRelationCreateError::ReverseExists));
        }
    }
    assert_eq!(repo_a.list_for_task(a.id).await.unwrap().len(), 1);

    tasks.delete(a.id).await.unwrap();
    tasks.delete(b.id).await.unwrap();
}

#[tokio::test]
async fn deleting_either_task_cascades_its_relations() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    // Without ON DELETE CASCADE the delete itself would fail; a surviving
    // relation row would still join to the task in the listing below.
    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();
    let relation = TaskRelation::new(a.id, b.id, TaskRelationType::Blocks, now());
    repo.create(relation.clone()).await.unwrap();

    tasks.delete(a.id).await.unwrap();
    assert!(repo.find_by_id(relation.id).await.unwrap().is_none());
    assert!(repo.list_for_task(b.id).await.unwrap().is_empty());
    tasks.delete(b.id).await.unwrap();

    // And the other direction.
    let a = test_task(None);
    let b = test_task(None);
    tasks.create(a.clone()).await.unwrap();
    tasks.create(b.clone()).await.unwrap();
    let relation = TaskRelation::new(a.id, b.id, TaskRelationType::Blocks, now());
    repo.create(relation.clone()).await.unwrap();

    tasks.delete(b.id).await.unwrap();
    assert!(repo.find_by_id(relation.id).await.unwrap().is_none());
    assert!(repo.list_for_task(a.id).await.unwrap().is_empty());
    tasks.delete(a.id).await.unwrap();
}

/// A relation with a fixed id and created_at for ordering tests.
fn relation_on(
    id: u128,
    source: TaskId,
    target: TaskId,
    kind: TaskRelationType,
    created_at: DateTime<Utc>,
) -> TaskRelation {
    let mut relation = TaskRelation::new(source, target, kind, created_at);
    relation.id = TaskRelationId(Uuid::from_u128(id));
    relation
}

/// The single boolean column of an `EXISTS (…)` probe.
#[derive(diesel::QueryableByName)]
struct Exists {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    exists: bool,
}

fn constraint_exists(conn: &mut diesel::PgConnection, name: &str) -> bool {
    let Exists { exists } = diesel::sql_query(format!(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = '{name}') AS exists"
    ))
    .get_result(conn)
    .expect("could not probe pg_constraint");
    exists
}

fn index_exists(conn: &mut diesel::PgConnection, name: &str) -> bool {
    let Exists { exists } = diesel::sql_query(format!(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = '{name}') AS exists"
    ))
    .get_result(conn)
    .expect("could not probe pg_indexes");
    exists
}

/// Read one relation row back by id, or `None` when it is gone.
fn stored_relation(conn: &mut diesel::PgConnection, id: TaskRelationId) -> Option<TaskRelation> {
    let row: Option<TaskRelationRow> = task_relations::table
        .find(id.0)
        .first(conn)
        .optional()
        .expect("could not read back a seeded row");
    row.map(task_relation_from_row)
        .transpose()
        .expect("row maps to a domain relation")
}

#[tokio::test]
async fn task_relation_migration_cleans_offending_rows_and_reverts() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        // In CI these tests must run: a green build that skipped them proves nothing.
        if std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip Postgres tests in CI");
        }
        return;
    };

    // This test drops and re-adds the 3.6 constraints, so it runs on a scratch
    // database instead of sharing the schema with the other (parallel) tests.
    // A scratch database left behind by a crashed run is dropped again here.
    let db_name = "minerva_task_relation_migration_test";
    let admin_pool = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(&url))
        .expect("could not create admin test pool");
    {
        let mut conn = admin_pool.get().expect("admin pool connects");
        conn.batch_execute(format!("DROP DATABASE IF EXISTS {db_name}").as_str())
            .expect("could not drop a stale scratch database");
        conn.batch_execute(format!("CREATE DATABASE {db_name}").as_str())
            .expect("could not create the scratch database");
    }

    let slash = url.rfind('/').expect("DATABASE_URL names a database");
    let scratch_pool = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(
            diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(
                url[..slash + 1].to_owned() + db_name,
            ),
        )
        .expect("could not create scratch test pool");

    // Fresh database: the embedded migrations apply cleanly, including the
    // 3.6 one.
    infrastructure::migrations::run_migrations(&scratch_pool)
        .expect("migrations failed on a fresh database");

    {
        let mut conn = scratch_pool.get().expect("scratch pool connects");
        // Back to the schema this migration started from...
        conn.batch_execute(
            "DROP INDEX IF EXISTS task_relations_unique_relates_to_pair;
             DROP INDEX IF EXISTS task_relations_unique_blocks_pair;
             ALTER TABLE task_relations DROP CONSTRAINT IF EXISTS task_relations_no_self_relation;",
        )
        .expect("could not reset to the pre-migration schema");

        // ...with rows that violate it: a self-relation; reversed `blocks`
        // rows for one pair (the older must survive); duplicate `relates_to`
        // rows for another.
        let a = test_task(None);
        let b = test_task(None);
        let c = test_task(None);
        for task in [&a, &b, &c] {
            diesel::insert_into(tasks::table)
                .values((
                    tasks::id.eq(task.id.0),
                    tasks::title.eq("Migration test task"),
                    tasks::status.eq("backlog"),
                    tasks::created_at.eq(&task.created_at),
                    tasks::updated_at.eq(&task.updated_at),
                ))
                .execute(&mut conn)
                .expect("could not seed a task");
        }
        let self_relation = relation_on(10, a.id, a.id, TaskRelationType::Blocks, at(1, 1));
        let old_blocks = relation_on(11, a.id, b.id, TaskRelationType::Blocks, at(1, 2));
        let new_blocks = relation_on(12, b.id, a.id, TaskRelationType::Blocks, at(1, 3));
        let first_relates = relation_on(13, a.id, c.id, TaskRelationType::RelatesTo, at(1, 4));
        let second_relates = relation_on(14, c.id, a.id, TaskRelationType::RelatesTo, at(1, 5));
        for (relation, kind) in [
            (&self_relation, "blocks"),
            (&old_blocks, "blocks"),
            (&new_blocks, "blocks"),
            (&first_relates, "relates_to"),
            (&second_relates, "relates_to"),
        ] {
            diesel::insert_into(task_relations::table)
                .values((
                    task_relations::id.eq(relation.id.0),
                    task_relations::source_task_id.eq(relation.source_task_id.0),
                    task_relations::target_task_id.eq(relation.target_task_id.0),
                    task_relations::relation_type.eq(kind),
                    task_relations::created_at.eq(&relation.created_at),
                ))
                .execute(&mut conn)
                .expect("could not seed an offending row");
        }

        // The migration applies over the mess and leaves only canonical rows.
        conn.batch_execute(include_str!(
            "../../migrations/20261007000001_constrain_task_relations/up.sql"
        ))
        .expect("up migration failed on a database with offending rows");

        // The self-relation is gone and each pair keeps exactly its oldest row.
        assert!(stored_relation(&mut conn, self_relation.id).is_none());
        assert_eq!(
            stored_relation(&mut conn, old_blocks.id),
            Some(old_blocks.clone())
        );
        assert!(stored_relation(&mut conn, new_blocks.id).is_none());
        assert_eq!(
            stored_relation(&mut conn, first_relates.id),
            Some(first_relates.clone())
        );
        assert!(stored_relation(&mut conn, second_relates.id).is_none());

        // The new constraints are in place.
        assert!(constraint_exists(
            &mut conn,
            "task_relations_no_self_relation"
        ));
        assert!(index_exists(&mut conn, "task_relations_unique_blocks_pair"));
        assert!(index_exists(
            &mut conn,
            "task_relations_unique_relates_to_pair"
        ));

        // And the migration reverts cleanly.
        conn.batch_execute(include_str!(
            "../../migrations/20261007000001_constrain_task_relations/down.sql"
        ))
        .expect("down migration failed");
        assert!(!constraint_exists(
            &mut conn,
            "task_relations_no_self_relation"
        ));
        assert!(!index_exists(
            &mut conn,
            "task_relations_unique_blocks_pair"
        ));
        assert!(!index_exists(
            &mut conn,
            "task_relations_unique_relates_to_pair"
        ));
        // The surviving rows are untouched by the revert.
        assert_eq!(stored_relation(&mut conn, old_blocks.id), Some(old_blocks));
    }

    drop(scratch_pool);
    let mut conn = admin_pool.get().expect("admin pool connects");
    conn.batch_execute(format!("DROP DATABASE {db_name}").as_str())
        .expect("could not drop the scratch database");
}

#[tokio::test]
async fn list_for_task_is_ordered_by_created_at_then_id() {
    let Some(pool) = pool() else { return };
    let repo = PostgresTaskRelationRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool);

    let task = test_task(None);
    tasks.create(task.clone()).await.unwrap();
    let others: Vec<Task> = (0..3).map(|_| test_task(None)).collect();
    for other in &others {
        tasks.create(other.clone()).await.unwrap();
    }

    // created_at decides first; a tie on created_at is broken by id. The task
    // is the source of two rows and the target of one.
    let earliest = relation_on(1, task.id, others[0].id, TaskRelationType::Blocks, at(1, 1));
    let tied_lower_id = relation_on(
        2,
        task.id,
        others[1].id,
        TaskRelationType::RelatesTo,
        at(1, 2),
    );
    let tied_higher_id = relation_on(4, others[2].id, task.id, TaskRelationType::Blocks, at(1, 2));
    for relation in [&earliest, &tied_lower_id, &tied_higher_id] {
        repo.create(relation.clone()).await.unwrap();
    }

    assert_eq!(
        repo.list_for_task(task.id).await.unwrap(),
        vec![earliest, tied_lower_id, tied_higher_id]
    );

    tasks.delete(task.id).await.unwrap();
    for other in &others {
        tasks.delete(other.id).await.unwrap();
    }
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

    let for_goal = repo
        .list_for_target(ProgressTarget::Goal(goal.id))
        .await
        .unwrap();
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

#[tokio::test]
async fn user_identity_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let identities = PostgresUserIdentityRepository::new(pool.clone());
    let users = PostgresUserRepository::new(pool.clone());

    // A passwordless user: its identity link is the only way in.
    let now = now();
    let user = User {
        id: UserId::new(),
        email: format!("identity-{}@example.com", Uuid::new_v4()),
        password_hash: None,
        display_name: "Identity test user".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    users.create(user.clone()).await.unwrap();

    let identity = UserIdentity::new(
        user.id,
        "https://idp.example".into(),
        format!("sub-{}", Uuid::new_v4()),
        Some(user.email.clone()),
        now,
    );
    identities.create(identity.clone()).await.unwrap();

    let found = identities
        .find_by_issuer_and_subject("https://idp.example".into(), identity.subject.clone())
        .await
        .unwrap()
        .expect("identity to exist");
    assert_eq!(found, identity);
    assert_eq!(
        identities.list_for_user(user.id).await.unwrap(),
        vec![identity.clone()]
    );

    // The (issuer, subject) pair is unique: linking it a second time is a conflict.
    let duplicate = UserIdentity::new(
        user.id,
        "https://idp.example".into(),
        identity.subject.clone(),
        None,
        now,
    );
    assert!(matches!(
        identities.create(duplicate).await,
        Err(RepositoryError::Conflict(_))
    ));

    // UserRepository has no delete yet; dropping the user row directly also
    // cascade-deletes its identity links.
    let mut conn = pool.get().unwrap();
    diesel::delete(infrastructure::schema::users::table.find(user.id.0))
        .execute(&mut conn)
        .unwrap();
}

#[tokio::test]
async fn user_repository_round_trip() {
    let Some(pool) = pool() else { return };
    let users = PostgresUserRepository::new(pool.clone());
    let now = now();
    let email = format!("user-{}@example.com", Uuid::new_v4());
    // Staff on purpose: a non-default role proves the column round-trips its
    // own value rather than a database default.
    let user = User {
        id: UserId::new(),
        email: email.clone(),
        password_hash: Some("not-a-real-hash".into()),
        display_name: "Round trip user".into(),
        role: Role::Staff,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };

    let created = users.create(user).await.expect("create");
    assert_eq!(created.email, email);

    let by_id = users
        .find_by_id(created.id)
        .await
        .expect("find by id")
        .expect("user exists");
    assert_eq!(by_id, created);

    // Lookups are case-insensitive: the port lowercases before querying.
    let by_email = users
        .find_by_email(email.to_uppercase())
        .await
        .expect("find by email")
        .expect("user found by mixed-case email");
    assert_eq!(by_email, created);

    // A duplicate (normalized) email violates the unique constraint.
    let duplicate = User {
        id: UserId::new(),
        email: email.clone(),
        password_hash: None,
        display_name: "Duplicate".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    assert!(matches!(
        users.create(duplicate).await,
        Err(RepositoryError::Conflict(_))
    ));

    // Update the row (including its role) and read it back.
    let mut updated = created.clone();
    updated.display_name = "Updated name".into();
    updated.role = Role::Admin;
    let updated = users.update(updated).await.expect("update");
    let reloaded = users
        .find_by_id(created.id)
        .await
        .expect("reload")
        .expect("user exists");
    assert_eq!(reloaded, updated);

    // A passwordless account (SSO-only) round-trips as `None`, not an empty
    // string.
    let passwordless = User {
        id: UserId::new(),
        email: format!("passwordless-{}@example.com", Uuid::new_v4()),
        password_hash: None,
        display_name: "Passwordless".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    let created_pw = users
        .create(passwordless)
        .await
        .expect("create passwordless");
    let reloaded_pw = users
        .find_by_id(created_pw.id)
        .await
        .expect("reload passwordless")
        .expect("passwordless user exists");
    assert_eq!(reloaded_pw.password_hash, None);

    // Characterization: `find_by_email` lowercases only the *query*. A row
    // stored with mixed case (bypassing the auth layer's normalization) is
    // never found. Every current write path normalizes, so this stays latent —
    // pinned here so a future change to the lookup (e.g. `LOWER(email) =
    // LOWER(?)`) surfaces as a test failure instead of a silent behaviour
    // change.
    let mixed_case_email = format!("Mixed-Case-{}@example.com", Uuid::new_v4());
    let mixed_case = User {
        id: UserId::new(),
        email: mixed_case_email.clone(),
        password_hash: None,
        display_name: "Mixed case".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    let mixed = users
        .create(mixed_case)
        .await
        .expect("create mixed-case user");
    assert!(
        users
            .find_by_email(mixed_case_email.to_lowercase())
            .await
            .expect("find mixed-case")
            .is_none()
    );

    // UserRepository has no delete yet; drop the rows directly (sessions and
    // identities would cascade).
    let mut conn = pool.get().unwrap();
    for id in [created.id, created_pw.id, mixed.id] {
        diesel::delete(infrastructure::schema::users::table.find(id.0))
            .execute(&mut conn)
            .unwrap();
    }
}

#[tokio::test]
async fn create_if_no_users_refuses_when_any_user_exists() {
    let Some(pool) = pool() else { return };
    let users = PostgresUserRepository::new(pool.clone());
    let now = now();
    // The tests share one database, so the users table is never empty here:
    // only the "users already exist" branch can be exercised. The empty-table
    // branch (the actual insert) is covered by the fake-based unit tests in
    // application/src/bootstrap.rs.
    let first = User {
        id: UserId::new(),
        email: format!("bootstrap-{}@example.com", Uuid::new_v4()),
        password_hash: Some("not-a-real-hash".into()),
        display_name: "Bootstrap blocker".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    users.create(first.clone()).await.expect("create");

    let second = User {
        id: UserId::new(),
        email: format!("bootstrap-second-{}@example.com", Uuid::new_v4()),
        password_hash: Some("not-a-real-hash".into()),
        display_name: "Second admin".into(),
        role: Role::Admin,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    let inserted = users
        .create_if_no_users(second.clone())
        .await
        .expect("create_if_no_users");
    assert!(inserted.is_none(), "no insert while a user exists");
    assert!(users.find_by_id(second.id).await.unwrap().is_none());

    // UserRepository has no delete yet; drop the row directly.
    let mut conn = pool.get().unwrap();
    diesel::delete(infrastructure::schema::users::table.find(first.id.0))
        .execute(&mut conn)
        .unwrap();
}

#[tokio::test]
async fn role_managed_by_sso_change_writes_role_and_flag_under_the_lock() {
    let Some(pool) = pool() else { return };
    let users = PostgresUserRepository::new(pool.clone());
    let now = now();
    let make_user = |name: &str| User {
        id: UserId::new(),
        email: format!("{name}-{}@example.com", Uuid::new_v4()),
        password_hash: None,
        display_name: name.into(),
        role: Role::Admin,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };

    // Two active admins so the last-admin guard stays out of the way.
    let a = users.create(make_user("first")).await.expect("create A");
    let b = users.create(make_user("second")).await.expect("create B");

    // The role and the flag land in one write...
    let changed = users
        .apply_access_change(a.id, AccessChange::RoleManagedBySso(Role::Staff))
        .await
        .expect("demote A while B is still an admin");
    assert_eq!(changed.role, Role::Staff);
    assert!(changed.role_managed_by_sso);

    // ...and re-applying the same role with the flag set is a no-op.
    let again = users
        .apply_access_change(a.id, AccessChange::RoleManagedBySso(Role::Staff))
        .await
        .expect("re-apply");
    assert_eq!(again.role, Role::Staff);
    assert!(again.role_managed_by_sso);

    // The same role without the flag still writes: the flag is part of the
    // state being applied. (The last-admin guard this variant also enforces
    // counts every active admin in the shared test database, so it cannot be
    // asserted here; the fake-based service tests cover it.)
    let flagged = users
        .apply_access_change(b.id, AccessChange::RoleManagedBySso(Role::Admin))
        .await
        .expect("flag B without a role change");
    assert_eq!(flagged.role, Role::Admin);
    assert!(flagged.role_managed_by_sso);

    // UserRepository has no delete yet; drop the rows directly.
    let mut conn = pool.get().unwrap();
    for id in [a.id, b.id] {
        diesel::delete(infrastructure::schema::users::table.find(id.0))
            .execute(&mut conn)
            .unwrap();
    }
}
