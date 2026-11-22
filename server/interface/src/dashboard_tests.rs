//! Handler tests for the dashboard endpoints (roadmap 3.15, D22), against a
//! real Postgres: the response shapes, counts that follow an override set and
//! cleared through the real override endpoints, a stale-free read after a
//! task completes its milestone, trend points that pick up a manual-change
//! snapshot written by the real override flow, the timeline's statuses,
//! owners and links following the real endpoints, invalid query windows, and
//! no personal data in the body. The access matrix rows live in
//! `access_tests`.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::assignment::AssignmentService;
use application::auth::SessionService;
use application::dashboard::DashboardService;
use application::goal_milestone_links::GoalMilestoneLinkService;
use application::ports::{
    GoalRepository, MilestoneRepository, PasswordHasher, SessionRepository, StatusSnapshotTrigger,
    SystemClock, TaskRepository, UserRepository,
};
use application::progress_snapshots::{ProgressSnapshotService, RecordingStatusSnapshotTrigger};
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::status_override::StatusOverrideService;
use application::task_status::TaskStatusService;
use chrono::{NaiveDate, Utc};
use diesel::prelude::*;
use domain::{Goal, GoalId, Milestone, MilestoneId, Role, Status, Task, TaskId, User, UserId};
use infrastructure::Argon2PasswordHasher;
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresDashboardRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
    PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresSessionRepository,
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
        panic!("DATABASE_URL is not set; refusing to skip dashboard tests in CI");
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

/// Build the app with the real route table and only the `web::Data` the
/// dashboard, status-override and task-status handlers can need (the other
/// routes 500 if hit, but these tests never do). A macro because
/// `init_service`'s service type is opaque.
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
        // The override and task-status services recompute best-effort after
        // each change (roadmap 3.13, step 4), so they share the real
        // computation service over the shared pool (mirrors `main.rs`).
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(SystemClock),
        ));
        // The override endpoint records a manual-change snapshot through the
        // real trigger (roadmap 3.14, step 2), so the trend tests can read it
        // back; built over their own repositories like `main.rs`.
        let snapshots_service = Arc::new(ProgressSnapshotService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(PostgresProgressSnapshotRepository::new(pool.clone())),
            computation.clone(),
            Arc::new(SystemClock),
        ));
        let snapshots: Arc<dyn StatusSnapshotTrigger> =
            Arc::new(RecordingStatusSnapshotTrigger::new(snapshots_service));
        let overrides = StatusOverrideService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            snapshots,
            computation.clone(),
        );
        // The milestone handlers read task counts through the port (roadmap
        // 3.13), so a trait-object registration sits beside the concrete one,
        // like in `main.rs`.
        let task_counts: Arc<dyn TaskRepository> =
            Arc::new(PostgresTaskRepository::new(pool.clone()));
        init_service(
            App::new()
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::from(task_counts))
                .app_data(web::Data::new(overrides))
                .app_data(web::Data::new(TaskStatusService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    computation.clone(),
                )))
                // The timeline tests link and unlink through the real
                // endpoints (roadmap 3.4), so the link service is wired in
                // like `main.rs`.
                .app_data(web::Data::new(GoalMilestoneLinkService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
                    computation,
                )))
                // The owner test sets an owner through the real endpoint
                // (roadmap 3.11), so the assignment service is wired in like
                // `main.rs`.
                .app_data(web::Data::new(AssignmentService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    users.clone(),
                )))
                // Dashboard (roadmap 3.15, D22): the summary, trends and
                // timeline read through the service over its own repository
                // instances; the timeline resolves owners and percents
                // through the user and task ports.
                .app_data(web::Data::new(DashboardService::new(
                    Arc::new(PostgresDashboardRepository::new(pool.clone())),
                    users.clone(),
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(SystemClock),
                )))
                .app_data(web::Data::new(CookieSettings { secure: false }))
                .configure(routes::configure),
        )
        .await
    }};
}

/// One request against the test app: `token` is the session cookie value (or
/// none), `body` a JSON body for POST/PUT.
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

/// The raw body text of a response, for the no-leakage check.
async fn text_of(res: ServiceResponse) -> String {
    String::from_utf8(read_body(res).await.to_vec()).expect("a UTF-8 body")
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
        display_name: "Dashboard test user".into(),
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

/// A goal with a stored automatic status and no override yet.
fn test_goal(status: Status) -> Goal {
    let now = Utc::now();
    Goal {
        id: GoalId::new(),
        title: "Dashboard test goal".into(),
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
        title: "Dashboard test milestone".into(),
        description: None,
        status,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// A task on the given milestone.
fn test_task(milestone_id: MilestoneId) -> Task {
    let now = Utc::now();
    Task {
        id: TaskId::new(),
        milestone_id: Some(milestone_id),
        title: "Dashboard test task".into(),
        description: None,
        status: domain::TaskStatus::Backlog,
        target_date: None,
        assignee_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// Assert the summary body has the documented shape (roadmap 3.15, D22).
fn assert_summary_shape(json: &serde_json::Value) {
    assert!(json.get("as_of").is_some(), "as_of missing");
    for kind in ["goals", "milestones"] {
        let summary = &json[kind];
        assert!(summary["total"].is_u64(), "{kind} total missing");
        assert!(summary["manual"].is_u64(), "{kind} manual missing");
        // All four statuses are always present, each with count and manual.
        for status in ["on_track", "at_risk", "off_track", "complete"] {
            let entry = &summary["by_status"][status];
            assert!(entry["count"].is_u64(), "{kind} {status} count missing");
            assert!(entry["manual"].is_u64(), "{kind} {status} manual missing");
        }
    }
    // The totals add up across the four statuses.
    for kind in ["goals", "milestones"] {
        let summary = &json[kind];
        let by_status: u64 = ["on_track", "at_risk", "off_track", "complete"]
            .iter()
            .map(|status| summary["by_status"][*status]["count"].as_u64().unwrap())
            .sum();
        assert_eq!(summary["total"], by_status, "{kind} total");
    }
    let items = json["manually_set"]
        .as_array()
        .expect("manually_set is an array");
    for item in items {
        assert!(item["kind"].is_string(), "manually_set kind missing");
        assert!(item["id"].is_string(), "manually_set id missing");
        assert!(item["title"].is_string(), "manually_set title missing");
        assert!(item["status"].is_string(), "manually_set status missing");
        assert!(
            item["automatic_status"].is_string(),
            "manually_set automatic_status missing"
        );
        assert!(
            item.get("target_date").is_some(),
            "manually_set target_date missing"
        );
        assert!(
            item["target_date"].is_null() || item["target_date"].is_string(),
            "manually_set target_date malformed"
        );
    }
}

#[actix_web::test]
async fn the_summary_answers_with_the_documented_shape() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("shape"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;

    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/summary",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    assert_summary_shape(&json_of(res).await);

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_summary_follows_an_override_set_and_cleared() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("override"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // Setting the override through the real endpoint moves the count to the
    // override and flags the item as manual...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "complete" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/summary",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    // Lower bounds: the shared test database may hold other rows, but this
    // goal's contribution is exactly one complete, manually set item.
    assert!(
        json["goals"]["by_status"]["complete"]["count"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(
        json["goals"]["by_status"]["complete"]["manual"]
            .as_u64()
            .unwrap()
            >= 1
    );
    let entry = json["manually_set"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == goal.id.0.to_string())
        .expect("the goal in manually_set");
    assert_eq!(entry["kind"], "goal");
    assert_eq!(entry["status"], "complete");
    assert_eq!(entry["automatic_status"], "on_track");

    // ...and clearing it takes the item out of the list again.
    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::OK);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/summary",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    assert!(
        !json["manually_set"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == goal.id.0.to_string()),
        "the cleared goal is still listed"
    );

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_summary_is_stale_free_after_a_task_completes_its_milestone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("stale"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    // No target date and one open task: the milestone computes OnTrack.
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task(milestone.id))
        .await
        .expect("create task");

    // Completing the task through the board endpoint recomputes the
    // milestone (roadmap 3.13); the summary reads the stored status, so it
    // reports the change with no separate recompute call.
    let moved = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "done" }))
    );
    assert_eq!(moved.status(), StatusCode::OK);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/summary",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    assert!(
        json["milestones"]["by_status"]["complete"]["count"]
            .as_u64()
            .unwrap()
            >= 1
    );

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_summary_never_leaks_an_email_or_password_hash() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    // A user with a real password hash and a distinctive email, so the
    // leakage check has something concrete to look for.
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let password_hash = Argon2PasswordHasher
        .hash("dashboard-test-password")
        .await
        .expect("hash password");
    let email = unique_email("leak");
    let user = User {
        id: UserId::new(),
        email: email.clone(),
        password_hash: Some(password_hash.clone()),
        display_name: "Dashboard leak check".into(),
        role: Role::ReadOnly,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    let user = users.create(user).await.expect("create user");
    let token = issue_token(&pool, user.id).await;

    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/summary",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let body = text_of(res).await;
    assert!(!body.contains(&email), "the email leaked into the summary");
    assert!(
        !body.contains(&password_hash),
        "the password hash leaked into the summary"
    );

    delete_user(&pool, user.id);
}

/// Assert the trends body has the documented shape (roadmap 3.15, D22): the
/// resolved window echoed, both series present, every point carrying all
/// four counts, dates ascending, and an average on milestone points only.
fn assert_trends_shape(json: &serde_json::Value, from: &str, to: &str) {
    assert_eq!(json["from"], from);
    assert_eq!(json["to"], to);
    for (kind, has_average) in [("goals", false), ("milestones", true)] {
        let points = json[kind]
            .as_array()
            .unwrap_or_else(|| panic!("{kind} is an array"));
        let mut previous: Option<NaiveDate> = None;
        for point in points {
            let date = NaiveDate::parse_from_str(
                point["date"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{kind} point has a date")),
                "%Y-%m-%d",
            )
            .expect("a YYYY-MM-DD date");
            if let Some(previous) = previous {
                assert!(date > previous, "{kind} dates ascending");
            }
            previous = Some(date);
            let counts = &point["counts"];
            for status in ["on_track", "at_risk", "off_track", "complete"] {
                assert!(counts[status].is_u64(), "{kind} point missing {status}");
            }
            let total: u64 = ["on_track", "at_risk", "off_track", "complete"]
                .iter()
                .map(|status| counts[*status].as_u64().unwrap())
                .sum();
            assert_eq!(point["total"], total, "{kind} total");
            assert!(point["manual"].is_u64(), "{kind} manual missing");
            if has_average {
                let average = &point["average_percent_complete"];
                assert!(
                    average.is_null() || average.is_f64(),
                    "{kind} average malformed"
                );
            } else {
                assert!(
                    point.get("average_percent_complete").is_none(),
                    "goal points carry no average"
                );
            }
        }
    }
}

#[actix_web::test]
async fn the_trends_answer_with_the_documented_shape() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    // A read-only user: the trends are readable by any signed-in role.
    let user = create_user(&pool, unique_email("trend-shape"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;

    // An explicit window is echoed back resolved...
    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/trends?from=2026-11-14&to=2026-11-20",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    assert_trends_shape(&json_of(res).await, "2026-11-14", "2026-11-20");

    // ...and with no parameters the window defaults to 90 days ending today.
    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/trends",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let today = Utc::now().date_naive();
    assert_trends_shape(
        &json_of(res).await,
        (today - chrono::Duration::days(89)).to_string().as_str(),
        today.to_string().as_str(),
    );

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_trends_pick_up_a_manual_change_snapshot_from_the_override_flow() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("trend-flow"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task(milestone.id))
        .await
        .expect("create task");

    // Setting the overrides through the real endpoints records a manual-change
    // snapshot for today (roadmap 3.14)...
    let set_goal = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "complete" }))
    );
    assert_eq!(set_goal.status(), StatusCode::OK);
    let set_milestone = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(set_milestone.status(), StatusCode::OK);

    let today = Utc::now().date_naive();
    let res = request!(
        &app,
        Method::GET,
        format!("/api/dashboard/trends?from={today}&to={today}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // ...and today's point counts each under the override status, flagged as
    // manual. Lower bounds: the shared test database may hold other rows for
    // today, but each entity's contribution is exactly one.
    let goal_point = json["goals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["date"] == today.to_string())
        .expect("a goal point for today");
    assert!(goal_point["counts"]["complete"].as_u64().unwrap() >= 1);
    assert!(goal_point["manual"].as_u64().unwrap() >= 1);
    let milestone_point = json["milestones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["date"] == today.to_string())
        .expect("a milestone point for today");
    assert!(milestone_point["counts"]["at_risk"].as_u64().unwrap() >= 1);
    assert!(milestone_point["manual"].as_u64().unwrap() >= 1);
    // The milestone's snapshot carries its percent, so the day has an average.
    assert!(milestone_point["average_percent_complete"].is_f64());

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn invalid_trend_queries_are_400s_naming_the_parameter() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("trend-invalid"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;
    let cases = [
        ("?from=banana", "from"),
        ("?to=2026-13-40", "to"),
        // `from` after `to`: the explicit `from` is the offender.
        ("?from=2026-11-20&to=2026-11-19", "from"),
        // A window longer than 366 days, with `to` defaulting to today.
        ("?from=2020-01-01", "from"),
        // Duplicated parameters are unparseable by the extractor itself; the
        // query error handler must still answer with the standard envelope.
        ("?from=2026-11-20&from=2026-11-19", "from"),
        ("?to=2026-11-20&to=2026-11-19", "to"),
    ];
    for (query, param) in cases {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/dashboard/trends{query}"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{query}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "invalid_query", "{query}");
        assert!(
            json["error"]["message"]
                .as_str()
                .expect("a message")
                .contains(param),
            "message names {param}: {}",
            json["error"]["message"]
        );
    }

    delete_user(&pool, user.id);
}

/// Assert one timeline row carries its documented keys with well-formed
/// values (roadmap 3.15, D22). `kind` is only for the panic messages.
fn assert_timeline_row(row: &serde_json::Value, kind: &str) {
    assert!(row["id"].is_string(), "{kind} id missing");
    assert!(row["title"].is_string(), "{kind} title missing");
    assert!(
        NaiveDate::parse_from_str(
            row["start_date"]
                .as_str()
                .unwrap_or_else(|| panic!("{kind} start_date malformed")),
            "%Y-%m-%d"
        )
        .is_ok(),
        "{kind} start_date malformed"
    );
    assert!(
        row["target_date"].is_null() || row["target_date"].is_string(),
        "{kind} target_date malformed"
    );
    for key in ["status", "status_source", "automatic_status"] {
        assert!(row[key].is_string(), "{kind} {key} missing");
    }
    if !row["owner"].is_null() {
        assert!(row["owner"]["id"].is_string(), "{kind} owner malformed");
        assert!(
            row["owner"]["display_name"].is_string(),
            "{kind} owner display_name missing"
        );
        assert!(
            row["owner"]["active"].is_boolean(),
            "{kind} owner active missing"
        );
    }
}

/// Assert the timeline body has the documented shape (roadmap 3.15, D22):
/// both arrays present, every row whole, goal rows carrying their linked
/// milestone ids and milestone rows their percent complete.
fn assert_timeline_shape(json: &serde_json::Value) {
    for goal in json["goals"].as_array().expect("goals is an array") {
        assert_timeline_row(goal, "goal");
        let ids = goal["milestone_ids"]
            .as_array()
            .unwrap_or_else(|| panic!("goal milestone_ids missing"));
        assert!(
            ids.iter().all(|id| id.is_string()),
            "goal milestone_ids malformed"
        );
    }
    for milestone in json["milestones"]
        .as_array()
        .expect("milestones is an array")
    {
        assert_timeline_row(milestone, "milestone");
        let percent = milestone["percent_complete"]
            .as_u64()
            .unwrap_or_else(|| panic!("milestone percent_complete missing"));
        assert!(percent <= 100, "milestone percent_complete in range");
    }
}

/// The row of one kind of array with the given id, if present.
fn timeline_row<'a>(
    json: &'a serde_json::Value,
    kind: &str,
    id: Uuid,
) -> Option<&'a serde_json::Value> {
    json[kind]
        .as_array()?
        .iter()
        .find(|row| row["id"] == id.to_string())
}

#[actix_web::test]
async fn the_timeline_answers_with_the_documented_shape() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    // A read-only user: the timeline is readable by any signed-in role.
    let user = create_user(&pool, unique_email("timeline-shape"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task(milestone.id))
        .await
        .expect("create task");

    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_timeline_shape(&json);

    // The rows just created are present, unowned and undated...
    let goal_row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert!(goal_row["owner"].is_null(), "unowned goal has a null owner");
    assert!(
        goal_row["target_date"].is_null(),
        "undated goal has a null target date"
    );
    // ...and the start date is today (or yesterday, if the test straddled
    // UTC midnight).
    let today = Utc::now().date_naive();
    let start = NaiveDate::parse_from_str(
        goal_row["start_date"].as_str().expect("a start date"),
        "%Y-%m-%d",
    )
    .expect("a YYYY-MM-DD start date");
    assert!(
        start == today || start == today - chrono::Duration::days(1),
        "start date is the UTC creation date, got {start}"
    );
    let milestone_row =
        timeline_row(&json, "milestones", milestone.id.0).expect("the milestone in the timeline");
    assert!(
        milestone_row["owner"].is_null(),
        "unowned milestone has a null owner"
    );
    // One open task: nothing is done yet.
    assert_eq!(milestone_row["percent_complete"], 0);

    tasks.delete(task.id).await.expect("cleanup task");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_timeline_reports_an_override_as_manual_and_clears_back_to_automatic() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("timeline-override"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // Setting the override through the real endpoint moves the row to the
    // override status, flagged manual, with the automatic value alongside...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "complete" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert_eq!(row["status"], "complete");
    assert_eq!(row["status_source"], "manual");
    assert_eq!(row["automatic_status"], "on_track");

    // ...and clearing it takes the row back to the automatic value.
    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::OK);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert_eq!(row["status"], "on_track");
    assert_eq!(row["status_source"], "automatic");
    assert_eq!(row["automatic_status"], "on_track");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn linking_and_unlinking_a_milestone_changes_the_goals_milestone_ids() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("timeline-links"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    // Unlinked: the milestone is not among the goal's ids...
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert!(
        !row["milestone_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &serde_json::json!(milestone.id.0.to_string())),
        "unlinked milestone listed"
    );

    // Linking through the real endpoint adds it...
    let link = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(link.status(), StatusCode::NO_CONTENT);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert!(
        row["milestone_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &serde_json::json!(milestone.id.0.to_string())),
        "linked milestone missing"
    );

    // ...and unlinking removes it again.
    let unlink = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(unlink.status(), StatusCode::NO_CONTENT);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert!(
        !row["milestone_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &serde_json::json!(milestone.id.0.to_string())),
        "unlinked milestone still listed"
    );

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_goal_in_the_window_lists_a_linked_milestone_outside_it() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("timeline-window"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let mut goal = test_goal(Status::OnTrack);
    goal.target_date = Some(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
    let goal = goals.create(goal).await.expect("create goal");
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let mut milestone = test_milestone(Status::OnTrack);
    milestone.target_date = Some(NaiveDate::from_ymd_opt(2027, 6, 1).unwrap());
    let milestone = milestones
        .create(milestone)
        .await
        .expect("create milestone");
    let link = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(link.status(), StatusCode::NO_CONTENT);

    // A window that holds the goal but not its linked milestone: the goal
    // still lists the milestone id (D22), and the milestone itself is absent.
    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline?target_after=2026-11-01&target_before=2026-12-31",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the window");
    assert!(
        row["milestone_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &serde_json::json!(milestone.id.0.to_string())),
        "out-of-window linked milestone missing from the goal's ids"
    );
    assert!(
        timeline_row(&json, "milestones", milestone.id.0).is_none(),
        "the out-of-window milestone is listed"
    );

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

/// Deactivate a user row directly (the admin endpoint is not wired into the
/// test app): the owner summary must still resolve, flagged inactive.
fn deactivate_user(pool: &PgPool, user_id: UserId) {
    let mut conn = pool.get().expect("pool connection");
    diesel::update(infrastructure::schema::users::table.find(user_id.0))
        .set(infrastructure::schema::users::deactivated_at.eq(Some(Utc::now())))
        .execute(&mut conn)
        .expect("deactivate user");
}

#[actix_web::test]
async fn the_timeline_owner_is_a_person_summary_and_never_an_email() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("timeline-owner"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let owner = create_user(&pool, unique_email("timeline-owned-by"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // Setting the owner through the real endpoint...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "owner_id": owner.id.0.to_string() }))
    );
    assert_eq!(set.status(), StatusCode::OK);

    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let body = text_of(res).await;
    let json: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
    // The email never appears anywhere in the body (D17).
    assert!(
        !body.contains(&owner.email),
        "the owner's email leaked into the timeline"
    );
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert_eq!(row["owner"]["id"], owner.id.0.to_string());
    assert_eq!(row["owner"]["active"], true);

    // A deactivated owner still resolves, flagged inactive.
    deactivate_user(&pool, owner.id);
    let json = json_of(request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline",
        Some(&token),
        None::<&serde_json::Value>
    ))
    .await;
    let row = timeline_row(&json, "goals", goal.id.0).expect("the goal in the timeline");
    assert_eq!(row["owner"]["id"], owner.id.0.to_string());
    assert_eq!(row["owner"]["active"], false);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, owner.id);
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn invalid_timeline_queries_are_400s_naming_the_parameter() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("timeline-invalid"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;
    let cases = [
        ("?target_after=banana", "target_after"),
        ("?target_before=2026-13-40", "target_before"),
        // Duplicated parameters are unparseable by the extractor itself; the
        // query error handler must still answer with the standard envelope.
        (
            "?target_after=2026-11-20&target_after=2026-11-19",
            "target_after",
        ),
        (
            "?target_before=2026-11-20&target_before=2026-11-19",
            "target_before",
        ),
    ];
    for (query, param) in cases {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/dashboard/timeline{query}"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{query}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "invalid_query", "{query}");
        assert!(
            json["error"]["message"]
                .as_str()
                .expect("a message")
                .contains(param),
            "message names {param}: {}",
            json["error"]["message"]
        );
    }

    // An inverted range matches nothing rather than erroring: no row can sit
    // on or after 2999-01-02 and on or before 2999-01-01.
    let res = request!(
        &app,
        Method::GET,
        "/api/dashboard/timeline?target_after=2999-01-02&target_before=2999-01-01",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert!(json["goals"].as_array().unwrap().is_empty());
    assert!(json["milestones"].as_array().unwrap().is_empty());

    delete_user(&pool, user.id);
}
