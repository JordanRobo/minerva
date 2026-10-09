//! Handler tests for the manual status-override endpoints (roadmap 3.2),
//! against a real Postgres: setting an override reports the new effective
//! status as "manual" on the response, the GET and list agree with it,
//! clearing returns to the stored automatic status, and an ordinary goal or
//! milestone update never touches an active override.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::goal_list::GoalListService;
use application::milestone_list::MilestoneListService;
use application::ports::{
    GoalRepository, MilestoneRepository, NoopStatusSnapshotTrigger, SessionRepository,
    UserRepository,
};
use application::status_override::StatusOverrideService;
use chrono::Utc;
use diesel::prelude::*;
use domain::{Goal, GoalId, Milestone, MilestoneId, Role, Status, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalRepository, PostgresMilestoneRepository, PostgresSessionRepository,
    PostgresUserRepository,
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
        panic!("DATABASE_URL is not set; refusing to skip status-override tests in CI");
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

/// Build the app with the real route table and only the `web::Data` the goal
/// and milestone handlers can need (the other routes 500 if hit, but these
/// tests never do). A macro because `init_service`'s service type is opaque.
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
        // Snapshots are not built yet (roadmap 3.14): the no-op trigger keeps
        // the service wired exactly as `main.rs` wires it today.
        let overrides = StatusOverrideService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(NoopStatusSnapshotTrigger),
        );
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(overrides))
                .app_data(web::Data::new(GoalListService::new(Arc::new(
                    PostgresGoalRepository::new(pool.clone()),
                ))))
                .app_data(web::Data::new(MilestoneListService::new(Arc::new(
                    PostgresMilestoneRepository::new(pool.clone()),
                ))))
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

/// Assert the two status fields of a goal or milestone response body.
fn assert_status(json: &serde_json::Value, status: &str, source: &str) {
    assert_eq!(json["status"], status);
    assert_eq!(json["status_source"], source);
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
        display_name: "Override test user".into(),
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
        title: "Override test goal".into(),
        description: None,
        status,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

/// A milestone with a stored automatic status and no override yet.
fn test_milestone(status: Status) -> Milestone {
    let now = Utc::now();
    Milestone {
        id: MilestoneId::new(),
        title: "Override test milestone".into(),
        description: None,
        status,
        status_override: None,
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

#[actix_web::test]
async fn setting_a_goal_override_reports_the_manual_status() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-set"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_status(&json, "at_risk", "manual");

    // A subsequent GET and the list report the same effective status.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_status(&json_of(get).await, "at_risk", "manual");
    // The list is paginated, so narrow it to this test's own goals by title
    // before looking the row up.
    let list = request!(
        &app,
        Method::GET,
        "/api/goals?q=Override",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_of(list).await;
    let entry = list_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == goal.id.0.to_string())
        .expect("the goal in the list");
    assert_status(entry, "at_risk", "manual");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn clearing_a_goal_override_returns_to_the_stored_status() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-clear"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
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
        Some(&serde_json::json!({ "status": "complete" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    assert_status(&json_of(set).await, "complete", "manual");

    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_status(&json, "at_risk", "automatic");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_ordinary_goal_update_does_not_touch_an_active_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-update"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // An ordinary update before any override behaves as it always did...
    let before = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed early" }))
    );
    assert_eq!(before.status(), StatusCode::OK);
    assert_status(&json_of(before).await, "on_track", "automatic");

    // ...and one after the override neither clears it nor changes its value.
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let after = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed late" }))
    );
    assert_eq!(after.status(), StatusCode::OK);
    let json = json_of(after).await;
    assert_status(&json, "off_track", "manual");
    assert_eq!(json["title"], "Renamed late");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_goal_override_on_an_unknown_goal_is_a_404() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-404"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let missing = Uuid::new_v4();

    for (method, body) in [
        (
            Method::PUT,
            Some(serde_json::json!({ "status": "at_risk" })),
        ),
        (Method::DELETE, None),
    ] {
        let res = request!(
            &app,
            method.clone(),
            format!("/api/goals/{missing}/status-override"),
            Some(&token),
            body.as_ref()
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(json_of(res).await["error"]["code"], "not_found", "{method}");
    }

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_goal_status_value_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-400"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goal = PostgresGoalRepository::new(pool.clone())
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    // Rejected by JSON parsing before the handler runs: the standard 400.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "sideways" }))
    );
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(res).await["error"]["code"], "bad_request");

    PostgresGoalRepository::new(pool.clone())
        .delete(goal.id)
        .await
        .expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn setting_the_same_goal_override_twice_is_idempotent() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("goal-idem"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(Status::OnTrack))
        .await
        .expect("create goal");

    for _ in 0..2 {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/goals/{}/status-override", goal.id.0),
            Some(&token),
            Some(&serde_json::json!({ "status": "at_risk" }))
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_status(&json_of(res).await, "at_risk", "manual");
    }

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn setting_a_milestone_override_reports_the_manual_status() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-set"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_status(&json, "off_track", "manual");

    // A subsequent GET and the list report the same effective status.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_status(&json_of(get).await, "off_track", "manual");
    // The list is paginated, so narrow it to this test's own milestones by
    // title before looking the row up.
    let list = request!(
        &app,
        Method::GET,
        "/api/milestones?q=Override",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_of(list).await;
    let entry = list_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == milestone.id.0.to_string())
        .expect("the milestone in the list");
    assert_status(entry, "off_track", "manual");

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn clearing_a_milestone_override_returns_to_the_stored_status() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-clear"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    // The stored automatic status differs from the override, so the clear
    // has something visible to return to.
    let milestone = milestones
        .create(test_milestone(Status::OffTrack))
        .await
        .expect("create milestone");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "on_track" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    assert_status(&json_of(set).await, "on_track", "manual");

    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_status(&json, "off_track", "automatic");

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_ordinary_milestone_update_does_not_touch_an_active_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-update"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    // An ordinary update before any override behaves as it always did...
    let before = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed early" }))
    );
    assert_eq!(before.status(), StatusCode::OK);
    assert_status(&json_of(before).await, "on_track", "automatic");

    // ...and one after the override neither clears it nor changes its value.
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let after = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed late" }))
    );
    assert_eq!(after.status(), StatusCode::OK);
    let json = json_of(after).await;
    assert_status(&json, "at_risk", "manual");
    assert_eq!(json["title"], "Renamed late");

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_milestone_override_on_an_unknown_milestone_is_a_404() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-404"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let missing = Uuid::new_v4();

    for (method, body) in [
        (
            Method::PUT,
            Some(serde_json::json!({ "status": "at_risk" })),
        ),
        (Method::DELETE, None),
    ] {
        let res = request!(
            &app,
            method.clone(),
            format!("/api/milestones/{missing}/status-override"),
            Some(&token),
            body.as_ref()
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(json_of(res).await["error"]["code"], "not_found", "{method}");
    }

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_milestone_status_value_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-400"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestone = PostgresMilestoneRepository::new(pool.clone())
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    // Rejected by JSON parsing before the handler runs: the standard 400.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "sideways" }))
    );
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(res).await["error"]["code"], "bad_request");

    PostgresMilestoneRepository::new(pool.clone())
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn setting_the_same_milestone_override_twice_is_idempotent() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("ms-idem"), Role::Staff).await;
    let token = issue_token(&pool, user.id).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(Status::OnTrack))
        .await
        .expect("create milestone");

    for _ in 0..2 {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/milestones/{}/status-override", milestone.id.0),
            Some(&token),
            Some(&serde_json::json!({ "status": "off_track" }))
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_status(&json_of(res).await, "off_track", "manual");
    }

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}
