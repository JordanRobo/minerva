//! Handler tests for automatic status recomputation (roadmap 3.13, step 4),
//! against a real Postgres: every mutation endpoint is wired to the real
//! [`StatusComputationService`], exactly like `main.rs`, and each test drives
//! one trigger end to end — creating, moving and deleting tasks keep their
//! milestones fresh (finishing the last task completes the milestone and the
//! rollup follows to its goal in the same request; reopening reverses it),
//! deleting a milestone updates its linked goal, linking and unlinking change
//! the goal's rollup, a milestone override moves the goal's rollup and
//! clearing it reverts while leaving the milestone's own automatic status
//! untouched, and a goal override stays sticky through every recompute.

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
    GoalRepository, MilestoneRepository, NoopStatusSnapshotTrigger, SessionRepository,
    StatusSnapshotTrigger, SystemClock, TaskRepository, UserRepository,
};
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::status_override::StatusOverrideService;
use application::task_service::TaskService;
use application::task_status::TaskStatusService;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{
    Goal, GoalId, Milestone, MilestoneId, Role, Status, Task, TaskId, TaskStatus, User, UserId,
};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresSessionRepository, PostgresTaskRepository, PostgresUserRepository,
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
        panic!("DATABASE_URL is not set; refusing to skip status-recompute tests in CI");
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

/// Build the app with the real route table and the `web::Data` the goal,
/// milestone and task routes (plus their status-override and link routes)
/// can need. The shared [`StatusComputationService`] is the real one, handed
/// to every mutation service exactly like `main.rs` — that is what these
/// tests are about: the stored statuses stay fresh after each write. A macro
/// because `init_service`'s service type is opaque.
macro_rules! test_app {
    ($url:expr) => {{
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
        // Snapshots are not built yet (roadmap 3.14), so the trigger is the
        // no-op, like `main.rs`.
        let snapshots: Arc<dyn StatusSnapshotTrigger> = Arc::new(NoopStatusSnapshotTrigger);
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

/// GET one goal or milestone and assert its effective status and where it
/// came from — the whole point of these tests.
macro_rules! expect_status {
    ($app:expr, $token:expr, $uri:expr, $status:expr, $source:expr) => {{
        let res = request!(
            $app,
            Method::GET,
            $uri,
            Some($token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK, "GET {}", stringify!($uri));
        let json = json_of(res).await;
        assert_eq!(json["status"], $status);
        assert_eq!(json["status_source"], $source);
    }};
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
        display_name: "Status recompute test user".into(),
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
    let user = create_user(pool, unique_email("status-recompute"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A fixed timestamp in 2026, exact to the second.
fn at(month: u32, day: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(2026, month, day)
        .expect("valid date")
        .and_hms_opt(9, 0, 0)
        .expect("valid time")
        .and_utc()
}

/// A milestone with no target date: its automatic status is decided by its
/// tasks alone (all done completes it, anything else stays OnTrack).
fn test_milestone() -> Milestone {
    let now = at(1, 1);
    Milestone {
        id: MilestoneId::new(),
        title: "Recompute milestone".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_goal() -> Goal {
    let now = at(1, 1);
    Goal {
        id: GoalId::new(),
        title: "Recompute goal".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_task(milestone_id: Option<MilestoneId>, status: TaskStatus) -> Task {
    let now = at(1, 1);
    Task {
        id: TaskId::new(),
        milestone_id,
        title: "Recompute task".into(),
        description: None,
        status,
        target_date: None,
        assignee_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// Link a goal to a milestone through the API (204 either way).
macro_rules! link {
    ($app:expr, $token:expr, $goal_id:expr, $milestone_id:expr) => {{
        let res = request!(
            $app,
            Method::PUT,
            format!("/api/goals/{}/milestones/{}", $goal_id.0, $milestone_id.0),
            Some($token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }};
}

/// Move a task to a board column through the API (200).
macro_rules! move_task {
    ($app:expr, $token:expr, $task_id:expr, $status:expr) => {{
        let res = request!(
            $app,
            Method::PATCH,
            format!("/api/tasks/{}/status", $task_id.0),
            Some($token),
            Some(&serde_json::json!({ "status": $status }))
        );
        assert_eq!(res.status(), StatusCode::OK);
    }};
}

#[actix_web::test]
async fn finishing_the_last_task_completes_the_milestone_and_its_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let t1 = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");
    let t2 = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    // The first of the two done: nothing is complete yet.
    move_task!(&app, &token, t1.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    // The last one done: the milestone completes and the rollup follows in
    // the same request — milestones are recomputed before goals, so the goal
    // sees the fresh milestone status.
    move_task!(&app, &token, t2.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "complete",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "complete",
        "automatic"
    );

    tasks.delete(t1.id).await.expect("cleanup task");
    tasks.delete(t2.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn reopening_a_task_reverses_the_completion() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    move_task!(&app, &token, task.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "complete",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "complete",
        "automatic"
    );

    // Back to the board: both fall back to their automatic OnTrack.
    move_task!(&app, &token, task.id, "backlog");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn creating_a_task_recomputes_its_milestone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // Created through the API: no target date starts it OnTrack.
    let res = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({ "title": "Fresh milestone" }))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let milestone_id =
        MilestoneId(Uuid::parse_str(json_of(res).await["id"].as_str().unwrap()).unwrap());

    // A single done task completes it...
    let res = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(
            &serde_json::json!({ "milestone_id": milestone_id.0.to_string(), "title": "Only task", "status": "done" })
        )
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let first = TaskId(Uuid::parse_str(json_of(res).await["id"].as_str().unwrap()).unwrap());
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone_id.0),
        "complete",
        "automatic"
    );

    // ...and a new open task puts it back.
    let res = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(
            &serde_json::json!({ "milestone_id": milestone_id.0.to_string(), "title": "More work", "status": "backlog" })
        )
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let second = TaskId(Uuid::parse_str(json_of(res).await["id"].as_str().unwrap()).unwrap());
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone_id.0),
        "on_track",
        "automatic"
    );

    let tasks = PostgresTaskRepository::new(pool.clone());
    tasks.delete(first).await.expect("cleanup task");
    tasks.delete(second).await.expect("cleanup task");
    milestones
        .delete(milestone_id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn moving_a_task_between_milestones_updates_both() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let from = test_milestone();
    milestones
        .create(from.clone())
        .await
        .expect("create milestone");
    let to = test_milestone();
    milestones
        .create(to.clone())
        .await
        .expect("create milestone");
    let task = tasks
        .create(test_task(Some(from.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    // The lone done task completes its milestone.
    move_task!(&app, &token, task.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", from.id.0),
        "complete",
        "automatic"
    );

    // An ordinary update moves it: both the old and the new milestone are
    // recomputed — the old one loses its completion, the new one gains it.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        Some(
            &serde_json::json!({ "milestone_id": to.id.0.to_string(), "title": "Moved task", "status": "done" })
        )
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", from.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", to.id.0),
        "complete",
        "automatic"
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones.delete(from.id).await.expect("cleanup milestone");
    milestones.delete(to.id).await.expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_a_task_updates_its_milestone_and_goals() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    move_task!(&app, &token, task.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "complete",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "complete",
        "automatic"
    );

    // The delete captures the milestone id before the task is gone: both fall
    // back to their automatic OnTrack.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_a_milestone_updates_its_linked_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    move_task!(&app, &token, task.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "complete",
        "automatic"
    );

    // The delete captures the linked goal ids before the links cascade away:
    // with no milestones left, the rollup is OnTrack again.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn linking_and_unlinking_a_milestone_changes_the_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());

    // Created through the API with a target date already past and no tasks:
    // the create trigger starts it OffTrack.
    let res = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({ "title": "Late milestone", "target_date": "2026-01-01" }))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let milestone_id =
        MilestoneId(Uuid::parse_str(json_of(res).await["id"].as_str().unwrap()).unwrap());
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone_id.0),
        "off_track",
        "automatic"
    );

    // A fresh goal computes its initial status: no milestones is OnTrack.
    let res = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        Some(&serde_json::json!({ "title": "Fresh goal" }))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let goal_id = GoalId(Uuid::parse_str(json_of(res).await["id"].as_str().unwrap()).unwrap());
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal_id.0),
        "on_track",
        "automatic"
    );

    // Linking brings the off-track milestone into the rollup...
    link!(&app, &token, goal_id, milestone_id);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal_id.0),
        "off_track",
        "automatic"
    );

    // ...and unlinking takes it out again.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/milestones/{}", goal_id.0, milestone_id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal_id.0),
        "on_track",
        "automatic"
    );

    milestones
        .delete(milestone_id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal_id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn updating_a_milestones_target_date_recomputes_it_and_its_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    // No target date: nothing can be late.
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    // An ordinary update gives it a target date already past with work left:
    // both the milestone and its goal move to OffTrack.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Recompute milestone", "target_date": "2026-01-01" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "off_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "off_track",
        "automatic"
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_milestone_override_moves_the_goal_rollup_and_clearing_reverts_it() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    // The override makes the milestone effectively OffTrack...
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "off_track",
        "manual"
    );

    // ...and the rollup counts it by its effective status: the goal follows,
    // still automatically.
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "off_track",
        "automatic"
    );

    // Clearing reverts both — and the milestone's own automatic status was
    // never touched by the override, so it is back to its computed OnTrack.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "on_track",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "on_track",
        "automatic"
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_goal_override_is_sticky_through_recomputation() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goals = PostgresGoalRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let milestone = test_milestone();
    milestones
        .create(milestone.clone())
        .await
        .expect("create milestone");
    let goal = test_goal();
    goals.create(goal.clone()).await.expect("create goal");
    link!(&app, &token, goal.id, milestone.id);
    let task = tasks
        .create(test_task(Some(milestone.id), TaskStatus::Backlog))
        .await
        .expect("create task");

    // A manual AtRisk on the goal, while everything is actually OnTrack.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "at_risk",
        "manual"
    );

    // Finishing the only task completes the milestone and would roll the
    // goal up to Complete — but the override holds.
    move_task!(&app, &token, task.id, "done");
    expect_status!(
        &app,
        &token,
        format!("/api/milestones/{}", milestone.id.0),
        "complete",
        "automatic"
    );
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "at_risk",
        "manual"
    );

    // Clearing the override returns to the automatic status the recompute
    // already stored underneath it.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal.id.0),
        "complete",
        "automatic"
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_goal_reports_the_rollup_as_its_computed_status_under_an_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // A fresh goal has no links: computed OnTrack. The body carries no
    // status — it is server-computed (roadmap 3.13, D19).
    let created = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        Some(&serde_json::json!({ "title": "Rollup override goal" }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["status"], "on_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["automatic_status"], "on_track");
    let goal_id = GoalId(Uuid::parse_str(json["id"].as_str().unwrap()).unwrap());

    // A past-due milestone (no tasks) computes OffTrack at creation...
    let created = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(
            &serde_json::json!({ "title": "Rollup override milestone", "target_date": "2026-01-31" })
        )
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["status"], "off_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["automatic_status"], "off_track");
    let milestone_id = MilestoneId(Uuid::parse_str(json["id"].as_str().unwrap()).unwrap());

    // ...and linking it drags the goal's rollup to OffTrack...
    link!(&app, &token, goal_id, milestone_id);
    expect_status!(
        &app,
        &token,
        format!("/api/goals/{}", goal_id.0),
        "off_track",
        "automatic"
    );

    // ...and a manual setting sits on top of it without touching the rollup.
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal_id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let json = json_of(set).await;
    assert_eq!(json["status"], "at_risk");
    assert_eq!(json["status_source"], "manual");
    assert_eq!(json["automatic_status"], "off_track");

    milestones
        .delete(milestone_id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal_id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn create_and_update_bodies_carry_no_status_and_an_update_ignores_one() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // Creation without a status succeeds: the automatic value is computed.
    let created = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({ "title": "Status-free milestone" }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["status"], "on_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["automatic_status"], "on_track");
    let milestone_id = MilestoneId(Uuid::parse_str(json["id"].as_str().unwrap()).unwrap());

    // An update that still sends a status field is accepted — the request
    // types ignore unknown fields rather than rejecting them (roadmap 3.13,
    // D19) — and it never changes the automatic status.
    let updated = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}", milestone_id.0),
        Some(&token),
        Some(
            &serde_json::json!({ "title": "Status-free milestone (edited)", "status": "complete" })
        )
    );
    assert_eq!(updated.status(), StatusCode::OK);
    let json = json_of(updated).await;
    assert_eq!(json["title"], "Status-free milestone (edited)");
    assert_eq!(json["status"], "on_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["automatic_status"], "on_track");

    milestones
        .delete(milestone_id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}
