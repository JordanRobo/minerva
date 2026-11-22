//! Handler tests for manual-change progress snapshots (roadmap 3.14, step 2),
//! against a real Postgres: the app is wired exactly like `main.rs`, with the
//! real [`RecordingStatusSnapshotTrigger`] behind the override service. One
//! manual change records one snapshot of that entity only — setting and
//! clearing a goal override record two rows (a repeated value none), a
//! milestone override records its own row even when the linked goal's status
//! moves with it, and ordinary updates, board moves, task edits, assignee and
//! owner changes, link/unlink and comments record nothing. A failing snapshot
//! store never fails the override request.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::assignment::AssignmentService;
use application::auth::SessionService;
use application::goal_milestone_links::GoalMilestoneLinkService;
use application::goal_service::GoalService;
use application::milestone_service::MilestoneService;
use application::ports::{
    GoalRepository, MilestoneRepository, ProgressSnapshotRepository, RepositoryError,
    SessionRepository, StatusSnapshotTrigger, SystemClock, TaskRepository, UserRepository,
};
use application::progress_snapshots::{ProgressSnapshotService, RecordingStatusSnapshotTrigger};
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::status_override::StatusOverrideService;
use application::task_comments::TaskCommentService;
use application::task_service::TaskService;
use application::task_status::TaskStatusService;
use chrono::{NaiveDate, Utc};
use diesel::prelude::*;
use domain::{
    Goal, GoalId, Milestone, MilestoneId, NewSnapshot, ProgressSnapshot, Role, SnapshotReason,
    Status, StatusSource, Task, TaskId, TaskStatus, User, UserId,
};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresSessionRepository, PostgresTaskCommentRepository,
    PostgresTaskRepository, PostgresUserRepository,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{COOKIE_NAME, CookieSettings};
use crate::routes;

/// The `DATABASE_URL` the tests run against, or `None` to skip.
fn database_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    // In CI these tests must run: a green build that skipped them proves nothing.
    if url.is_none() && std::env::var_os("CI").is_some() {
        panic!("DATABASE_URL is not set; refusing to skip progress-snapshot tests in CI");
    }
    url
}

/// A one-connection pool (mirrors the other handler test modules: the default
/// pool size times the parallel test pools would exceed local Postgres's
/// `max_connections`). Pending migrations are applied once per process first.
fn test_pool(url: &str) -> PgPool {
    let pool = diesel::r2d2::Pool::builder()
        .max_size(1)
        .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(url))
        .expect("could not create test pool");
    static MIGRATIONS_APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    MIGRATIONS_APPLIED.get_or_init(|| {
        infrastructure::migrations::run_migrations(&pool)
            .expect("could not apply migrations in tests");
    });
    pool
}

/// Build the app with the real route table and every service wired exactly
/// like `main.rs` — including the real snapshot service and trigger, whose
/// store is `$snapshot_repo` so one test can swap in a failing one. A macro
/// because `init_service`'s service type is opaque.
macro_rules! test_app {
    ($url:expr, $snapshot_repo:expr) => {{
        let pool = test_pool($url);
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
        let users_data: web::Data<dyn UserRepository> = users.clone().into();
        let session_service = SessionService::new(
            sessions,
            Arc::new(Sha256SessionTokens),
            SessionService::DEFAULT_SESSION_TTL,
        );
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(SystemClock),
        ));
        // The real snapshot service and trigger (roadmap 3.14, step 2), over
        // their own repository instances on the shared pool, like `main.rs`.
        let snapshots_service = Arc::new(ProgressSnapshotService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            $snapshot_repo,
            computation.clone(),
            Arc::new(SystemClock),
        ));
        let snapshots: Arc<dyn StatusSnapshotTrigger> =
            Arc::new(RecordingStatusSnapshotTrigger::new(snapshots_service));
        // The milestone handlers read task counts through the port (roadmap
        // 3.13), so a trait-object registration sits beside the concrete one,
        // like in `main.rs`.
        let task_counts: Arc<dyn TaskRepository> =
            Arc::new(PostgresTaskRepository::new(pool.clone()));
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(web::Data::from(task_counts))
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(CookieSettings { secure: false }))
                .app_data(web::Data::new(AssignmentService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    users.clone(),
                )))
                .app_data(web::Data::new(TaskService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(MilestoneService::new(
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(GoalService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(TaskStatusService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(GoalMilestoneLinkService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(TaskCommentService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(PostgresTaskCommentRepository::new(pool.clone())),
                    snapshots.clone(),
                    computation.clone(),
                )))
                .app_data(web::Data::new(StatusOverrideService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    snapshots,
                    computation,
                )))
                .configure(routes::configure),
        )
        .await
    }};
}

/// One request against the test app: `token` is the session cookie value (or
/// none), `body` a JSON body for POST/PUT/PATCH.
macro_rules! request {
    ($app:expr, $method:expr, $uri:expr, $token:expr, $body:expr) => {{
        // `Method` is not `Copy`; callers keep their value for the
        // assertions, so the macro clones it.
        let mut req = TestRequest::with_uri(&$uri).method($method.clone());
        if let Some(body) = $body {
            req = req.set_json(body);
        }
        let token: Option<&str> = $token;
        if let Some(t) = token {
            req = req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={t}")));
        }
        $app.call(req.to_request()).await.unwrap()
    }};
}

/// The parsed JSON body of a response.
async fn json_of(res: ServiceResponse) -> serde_json::Value {
    serde_json::from_slice(&read_body(res).await).expect("a JSON body")
}

/// A [`ProgressSnapshotRepository`] whose every call fails, to pin that a
/// store failure never fails the override request.
struct FailingProgressSnapshotRepository;

#[async_trait::async_trait]
impl ProgressSnapshotRepository for FailingProgressSnapshotRepository {
    async fn record(&self, _snapshot: NewSnapshot) -> Result<ProgressSnapshot, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a snapshot store failure".to_owned(),
        ))
    }

    async fn record_daily_batch(
        &self,
        _snapshots: &[NewSnapshot],
    ) -> Result<usize, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a snapshot store failure".to_owned(),
        ))
    }

    async fn entity_ids_missing_daily(
        &self,
        _date: NaiveDate,
    ) -> Result<(Vec<GoalId>, Vec<MilestoneId>), RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a snapshot store failure".to_owned(),
        ))
    }

    async fn list_for_goal(
        &self,
        _goal_id: GoalId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a snapshot store failure".to_owned(),
        ))
    }

    async fn list_for_milestone(
        &self,
        _milestone_id: MilestoneId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a snapshot store failure".to_owned(),
        ))
    }
}

fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4())
}

/// A user created directly in the database (no sign-in needed: sessions are
/// issued straight from the [`SessionService`]).
async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: None,
        display_name: "Snapshot test user".into(),
        role,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    users.create(user).await.expect("create user")
}

/// Delete a user row directly (the repository has no delete); its sessions
/// cascade-delete with it.
fn delete_user(pool: &PgPool, user_id: UserId) {
    let mut conn = pool.get().expect("pool connection");
    diesel::delete(infrastructure::schema::users::table.find(user_id.0))
        .execute(&mut conn)
        .expect("delete user");
}

/// A live session token for `user_id`, issued like any sign-in would be.
async fn issue_token(pool: &PgPool, user_id: UserId) -> String {
    let sessions: Arc<dyn SessionRepository> =
        Arc::new(PostgresSessionRepository::new(pool.clone()));
    let service = SessionService::new(
        sessions,
        Arc::new(Sha256SessionTokens),
        SessionService::DEFAULT_SESSION_TTL,
    );
    service.issue(user_id).await.expect("issue session").token
}

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("progress-snapshot"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A goal with a stored automatic status and no override yet.
fn test_goal(status: Status) -> Goal {
    let now = Utc::now();
    Goal {
        id: GoalId::new(),
        title: "Snapshot goal".into(),
        description: None,
        status,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// A milestone with a stored automatic status and no override yet.
fn test_milestone(status: Status) -> Milestone {
    let now = Utc::now();
    Milestone {
        id: MilestoneId::new(),
        title: "Snapshot milestone".into(),
        description: None,
        status,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// The snapshot rows stored for a goal, straight from the table.
async fn goal_snapshots(pool: &PgPool, goal_id: GoalId) -> Vec<ProgressSnapshot> {
    PostgresProgressSnapshotRepository::new(pool.clone())
        .list_for_goal(goal_id)
        .await
        .expect("list goal snapshots")
}

/// The snapshot rows stored for a milestone, straight from the table.
async fn milestone_snapshots(pool: &PgPool, milestone_id: MilestoneId) -> Vec<ProgressSnapshot> {
    PostgresProgressSnapshotRepository::new(pool.clone())
        .list_for_milestone(milestone_id)
        .await
        .expect("list milestone snapshots")
}

#[actix_web::test]
async fn setting_and_clearing_a_goal_override_records_one_snapshot_each() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(
        &url,
        Arc::new(PostgresProgressSnapshotRepository::new(pool.clone()))
    );

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    // The stored automatic status differs from the override, so the clear
    // has something visible to return to.
    let goal = goals
        .create(test_goal(Status::AtRisk))
        .await
        .expect("create goal");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // One manual change, one row: the effective status and where it came
    // from, taken today.
    let listed = goal_snapshots(&pool, goal.id).await;
    let [snapshot] = listed.as_slice() else {
        panic!("exactly one snapshot expected");
    };
    assert_eq!(snapshot.status, Status::OffTrack);
    assert_eq!(snapshot.source, StatusSource::ManualOverride);
    assert_eq!(snapshot.reason, SnapshotReason::ManualChange);
    assert_eq!(snapshot.percent_complete(), None);
    assert_eq!(snapshot.snapshot_date, snapshot.taken_at.date_naive());

    // Repeating the value is a no-op: still one row.
    let repeated = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(goal_snapshots(&pool, goal.id).await.len(), 1);

    // The clear is the second change: back to the stored automatic status.
    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::OK);

    // Both manual changes persist (several per day are fine for manual rows).
    let listed = goal_snapshots(&pool, goal.id).await;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].status, Status::OffTrack);
    assert_eq!(listed[0].source, StatusSource::ManualOverride);
    assert_eq!(listed[1].status, Status::AtRisk);
    assert_eq!(listed[1].source, StatusSource::Computed);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_milestone_override_records_only_its_own_snapshot() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(
        &url,
        Arc::new(PostgresProgressSnapshotRepository::new(pool.clone()))
    );

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    // Link the two, then push the milestone OffTrack: its single linked goal
    // rolls up to OffTrack in the same request, so the goal's effective
    // status genuinely changes...
    let link = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(link.status(), StatusCode::NO_CONTENT);
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let goal_json = json_of(request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    assert_eq!(goal_json["status"], "off_track");

    // ...but the snapshot belongs to the milestone alone: exactly its own
    // row, and none for the goal.
    let listed = milestone_snapshots(&pool, milestone.id).await;
    let [snapshot] = listed.as_slice() else {
        panic!("exactly one milestone snapshot expected");
    };
    assert_eq!(snapshot.status, Status::OffTrack);
    assert_eq!(snapshot.source, StatusSource::ManualOverride);
    assert!(goal_snapshots(&pool, goal.id).await.is_empty());

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn ordinary_writes_record_no_snapshots() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(
        &url,
        Arc::new(PostgresProgressSnapshotRepository::new(pool.clone()))
    );

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");
    let task = tasks
        .create(Task {
            id: TaskId::new(),
            milestone_id: Some(milestone.id),
            title: "Snapshot task".into(),
            description: None,
            status: TaskStatus::Backlog,
            target_date: None,
            assignee_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .await
        .expect("create task");

    // Ordinary goal and milestone updates (D5: not status changes).
    for (method, uri, body) in [
        (
            Method::PUT,
            format!("/api/goals/{}", goal.id.0),
            serde_json::json!({ "title": "Renamed goal" }),
        ),
        (
            Method::PUT,
            format!("/api/milestones/{}", milestone.id.0),
            serde_json::json!({ "title": "Renamed milestone" }),
        ),
    ] {
        let res = request!(&app, method, uri, Some(&token), Some(&body));
        assert_eq!(res.status(), StatusCode::OK);
    }

    // Task create, update and board moves (D5/D6: a column move is not a
    // status change either).
    let created = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(
            &serde_json::json!({ "milestone_id": milestone.id.0, "title": "Second task", "status": "backlog" })
        )
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let second: TaskId =
        TaskId(Uuid::parse_str(json_of(created).await["id"].as_str().unwrap()).expect("a task id"));
    let updated = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed task", "status": "backlog" }))
    );
    assert_eq!(updated.status(), StatusCode::OK);
    for status in ["done", "backlog"] {
        let moved = request!(
            &app,
            Method::PATCH,
            format!("/api/tasks/{}/status", task.id.0),
            Some(&token),
            Some(&serde_json::json!({ "status": status }))
        );
        assert_eq!(moved.status(), StatusCode::OK);
    }

    // Assignee and owner changes (D17: single-column writes, no snapshot).
    for (uri, body) in [
        (
            format!("/api/tasks/{}/assignee", task.id.0),
            serde_json::json!({ "assignee_id": user.id.0 }),
        ),
        (
            format!("/api/goals/{}/owner", goal.id.0),
            serde_json::json!({ "owner_id": user.id.0 }),
        ),
        (
            format!("/api/milestones/{}/owner", milestone.id.0),
            serde_json::json!({ "owner_id": user.id.0 }),
        ),
    ] {
        let set = request!(&app, Method::PUT, uri.clone(), Some(&token), Some(&body));
        assert_eq!(set.status(), StatusCode::OK);
        let cleared = request!(
            &app,
            Method::DELETE,
            uri,
            Some(&token),
            None::<&serde_json::Value>
        );
        // A clear answers 200 with the entity (assignee/owner now null),
        // like every other assignment write.
        assert_eq!(cleared.status(), StatusCode::OK);
    }

    // Link/unlink (roadmap 3.4) and comments (D20): neither touches a
    // snapshot.
    let linked = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(linked.status(), StatusCode::NO_CONTENT);
    let unlinked = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(unlinked.status(), StatusCode::NO_CONTENT);
    let comment = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "a note" }))
    );
    assert_eq!(comment.status(), StatusCode::CREATED);
    let comment_id = json_of(comment).await["id"].as_str().unwrap().to_owned();
    let edited = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/comments/{comment_id}", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "an edited note" }))
    );
    assert_eq!(edited.status(), StatusCode::OK);
    let deleted = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/comments/{comment_id}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    // Nothing above is a manual status change: no snapshot rows at all.
    assert!(goal_snapshots(&pool, goal.id).await.is_empty());
    assert!(milestone_snapshots(&pool, milestone.id).await.is_empty());

    tasks.delete(task.id).await.expect("cleanup task");
    tasks.delete(second).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_failing_snapshot_store_still_succeeds_the_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(FailingProgressSnapshotRepository));

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // The hook fails best-effort: the override write already committed, so
    // the request still succeeds with the new effective status.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["status"], "off_track");
    assert_eq!(json["status_source"], "manual");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}
