//! Handler tests for the goal and milestone owner endpoints (roadmap 3.11,
//! D17), against a real Postgres: every goal and milestone response carries
//! the owner as a person summary (`null` when unowned), setting shows it in
//! the PUT response, the GET and the list, clearing nulls it, setting twice
//! is a no-op that leaves `updated_at` alone, creation validates its
//! `owner_id` exactly like the set endpoint (unknown 404s, deactivated
//! 409s), an ordinary PUT and a status override never touch the owner (and
//! an owner change never touches the override), a deactivated owner still
//! renders with `active` false, the goal–milestone link lists show their
//! owners too, and a list page resolves every owner with one user query.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::assignment::AssignmentService;
use application::auth::SessionService;
use application::goal_list::GoalListService;
use application::goal_milestone_links::GoalMilestoneLinkService;
use application::goal_service::GoalService;
use application::milestone_list::MilestoneListService;
use application::milestone_service::MilestoneService;
use application::ports::{
    GoalRepository, MilestoneRepository, NoopStatusSnapshotTrigger, SessionRepository, SystemClock,
    TaskRepository, UserRepository,
};
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::status_override::StatusOverrideService;
use chrono::Utc;
use diesel::prelude::*;
use domain::{Goal, GoalId, Milestone, MilestoneId, Role, Status, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresSessionRepository, PostgresTaskRepository, PostgresUserRepository,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

use crate::auth::{COOKIE_NAME, CookieSettings};
use crate::routes;
use crate::task_assignee_tests::CountingUserRepository;

/// The `DATABASE_URL` the tests run against, or `None` to skip.
fn database_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    // In CI these tests must run: a green build that skipped them proves nothing.
    if url.is_none() && std::env::var_os("CI").is_some() {
        panic!("DATABASE_URL is not set; refusing to skip owner tests in CI");
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
/// and milestone handlers (and the link lists these tests read through) can
/// need; the other routes 500 if hit, but these tests never do. A macro
/// because `init_service`'s service type is opaque. The user repository is
/// always the counting wrapper: two tests assert the single-query batching
/// rule on its counter, the rest pass a throwaway.
macro_rules! test_app {
    ($url:expr, $summary_lookups:expr) => {{
        let pool = test_pool($url);
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let session_service = SessionService::new(
            sessions,
            Arc::new(Sha256SessionTokens),
            SessionService::DEFAULT_SESSION_TTL,
        );
        // The handlers take the trait object, like `main.rs`.
        let users: Arc<dyn UserRepository> = Arc::new(CountingUserRepository {
            inner: Arc::new(PostgresUserRepository::new(pool.clone())),
            summary_lookups: $summary_lookups,
        });
        let users_data: web::Data<dyn UserRepository> = users.clone().into();
        // The services take ports, so they get their own repository
        // instances over the shared pool (mirrors `main.rs`). The mutation
        // services recompute statuses best-effort after each write (roadmap
        // 3.13, step 4), so they share the real computation service.
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(SystemClock),
        ));
        let overrides = StatusOverrideService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(NoopStatusSnapshotTrigger),
            computation.clone(),
        );
        // The milestone handlers read task counts through the port (roadmap
        // 3.13), so a trait-object registration sits beside the concrete one,
        // like in `main.rs`.
        let task_counts: Arc<dyn TaskRepository> =
            Arc::new(PostgresTaskRepository::new(pool.clone()));
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::from(task_counts))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(overrides))
                .app_data(web::Data::new(GoalListService::new(Arc::new(
                    PostgresGoalRepository::new(pool.clone()),
                ))))
                .app_data(web::Data::new(MilestoneListService::new(Arc::new(
                    PostgresMilestoneRepository::new(pool.clone()),
                ))))
                .app_data(web::Data::new(GoalMilestoneLinkService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(GoalService::new(
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(MilestoneService::new(
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
                    computation,
                )))
                .app_data(web::Data::new(AssignmentService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(PostgresGoalRepository::new(pool.clone())),
                    Arc::new(PostgresMilestoneRepository::new(pool.clone())),
                    users,
                )))
                .app_data(web::Data::new(CookieSettings { secure: false }))
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

fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4())
}

/// A title no other test can share, safe to use as a `q=` filter value
/// (one token, no characters that need URL encoding).
fn unique_title(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
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
        display_name: "Owner test user".into(),
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

/// Deactivate a user row directly (this test app registers no admin
/// services).
fn deactivate_user(pool: &PgPool, user_id: UserId) {
    let mut conn = pool.get().expect("pool connection");
    diesel::update(infrastructure::schema::users::table.find(user_id.0))
        .set(infrastructure::schema::users::deactivated_at.eq(Some(Utc::now())))
        .execute(&mut conn)
        .expect("deactivate user");
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
    let user = create_user(pool, unique_email("owner-staff"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A `PUT /api/{goals,milestones}/{id}/owner` body for the given user.
fn owner_body(user: &User) -> serde_json::Value {
    serde_json::json!({ "owner_id": user.id.0.to_string() })
}

/// A goal with the given title, no override and no owner yet.
fn test_goal(title: &str) -> Goal {
    let now = Utc::now();
    Goal {
        id: GoalId::new(),
        title: title.to_owned(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// A milestone with the given title, no override and no owner yet.
fn test_milestone(title: &str) -> Milestone {
    let now = Utc::now();
    Milestone {
        id: MilestoneId::new(),
        title: title.to_owned(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date: None,
        owner_id: None,
        created_at: now,
        updated_at: now,
    }
}

#[actix_web::test]
async fn setting_shows_the_goal_owner_in_the_response_get_and_list() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-set"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let title = unique_title("own");
    let goal = goals.create(test_goal(&title)).await.expect("create goal");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // The response is the standard goal shape plus the owner summary.
    assert_eq!(json["id"], goal.id.0.to_string());
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["display_name"], "Owner test user");
    assert_eq!(json["owner"]["active"], true);

    // The following GET agrees...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(json_of(get).await["owner"]["id"], owner.id.0.to_string());

    // ...and the list shows it on the item (filtered to this test's goal).
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals?q={title}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_of(list).await;
    let items = page["items"].as_array().expect("page items");
    let item = items
        .iter()
        .find(|item| item["id"] == goal.id.0.to_string())
        .unwrap_or_else(|| panic!("goal missing from the list"));
    assert_eq!(item["owner"]["id"], owner.id.0.to_string());

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn clearing_the_goal_owner_returns_null() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-clear"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Clear me"))
        .await
        .expect("create goal");

    // An unowned goal answers null...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["owner"], serde_json::Value::Null);

    // ...setting shows the person...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);
    assert_eq!(json_of(set).await["owner"]["id"], owner.id.0.to_string());

    // ...and clearing nulls it again, in the response and on the next GET.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json_of(cleared).await["owner"], serde_json::Value::Null);
    let get = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["owner"], serde_json::Value::Null);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn setting_the_goal_owner_twice_is_a_noop_with_an_unchanged_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-repeat"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Steady"))
        .await
        .expect("create goal");

    let uri = format!("/api/goals/{}/owner", goal.id.0);
    let first = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_of(first).await;

    // The same request again: 200, the same goal, updated_at untouched.
    let second = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(second.status(), StatusCode::OK);
    let second = json_of(second).await;

    assert_eq!(second["updated_at"], first["updated_at"]);
    assert_eq!(second, first);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn creating_a_goal_with_an_owner_validates_the_user() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-create"), Role::Staff).await;
    let deactivated = create_user(&pool, unique_email("goal-owner-create"), Role::Staff).await;
    deactivate_user(&pool, deactivated.id);
    let goals = PostgresGoalRepository::new(pool.clone());

    // A valid owner: 201, and the response already shows them.
    let created = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        Some(&serde_json::json!({
            "title": unique_title("own-create"),
            "owner_id": owner.id.0.to_string()
        }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["active"], true);
    let created_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

    // An unknown user 404s with the distinct code...
    let unknown = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created",
            "owner_id": Uuid::new_v4().to_string()
        }))
    );
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let json = json_of(unknown).await;
    assert_eq!(json["error"]["code"], "user_not_found");

    // ...and a deactivated one 409s.
    let dead = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created either",
            "owner_id": deactivated.id.0.to_string()
        }))
    );
    assert_eq!(dead.status(), StatusCode::CONFLICT);
    let json = json_of(dead).await;
    assert_eq!(json["error"]["code"], "user_deactivated");

    goals
        .delete(GoalId(created_id))
        .await
        .expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
    delete_user(&pool, deactivated.id);
}

#[actix_web::test]
async fn an_ordinary_put_preserves_the_goal_owner() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-put"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Keep me"))
        .await
        .expect("create goal");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // An ordinary update carries no owner field and must neither change nor
    // clear it (D17).
    let updated = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed", "description": "New details" }))
    );
    assert_eq!(updated.status(), StatusCode::OK);
    let json = json_of(updated).await;
    assert_eq!(json["title"], "Renamed");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn a_status_override_set_and_clear_preserve_the_goal_owner() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-override"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Override me"))
        .await
        .expect("create goal");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // Setting an override is a different single-column write: the owner
    // survives it...
    let overridden = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(overridden.status(), StatusCode::OK);
    let json = json_of(overridden).await;
    assert_eq!(json["status"], "off_track");
    assert_eq!(json["status_source"], "manual");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    // ...and clearing it does too, with the status back to automatic.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    let json = json_of(cleared).await;
    assert_eq!(json["status"], "on_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn changing_the_goal_owner_preserves_the_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-keep"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Keep the override"))
        .await
        .expect("create goal");

    let overridden = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/status-override", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(overridden.status(), StatusCode::OK);

    // Naming an owner must not touch the sticky override (D17).
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let json = json_of(set).await;
    assert_eq!(json["status"], "at_risk");
    assert_eq!(json["status_source"], "manual");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn an_unknown_goal_is_a_404_for_owner_writes() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-404"), Role::Staff).await;

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", Uuid::new_v4()),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::NOT_FOUND);
    let json = json_of(set).await;
    assert_eq!(json["error"]["code"], "not_found");

    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/owner", Uuid::new_v4()),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::NOT_FOUND);

    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn deactivating_the_goal_owner_leaves_them_shown_as_inactive() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("goal-owner-dead"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Dead owner"))
        .await
        .expect("create goal");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    deactivate_user(&pool, owner.id);

    // The goal still names them; the summary just reports the flag.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    let json = json_of(get).await;
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["active"], false);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn a_malformed_owner_body_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals
        .create(test_goal("Bogus body"))
        .await
        .expect("create goal");

    // Both owner routes share one request type, so the goal route proves the
    // deserialization for both. A value that is not a UUID fails: the
    // standard 400...
    let bad_uuid = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({ "owner_id": "not-a-uuid" }))
    );
    assert_eq!(bad_uuid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(bad_uuid).await["error"]["code"], "bad_request");

    // ...and a missing field fails too.
    let missing = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&serde_json::json!({}))
    );
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn setting_shows_the_milestone_owner_in_the_response_get_and_list() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-set"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let title = unique_title("own");
    let milestone = milestones
        .create(test_milestone(&title))
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // The response is the standard milestone shape plus the owner summary.
    assert_eq!(json["id"], milestone.id.0.to_string());
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["display_name"], "Owner test user");
    assert_eq!(json["owner"]["active"], true);

    // The following GET agrees...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(json_of(get).await["owner"]["id"], owner.id.0.to_string());

    // ...and the list shows it on the item (filtered to this test's
    // milestone).
    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={title}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_of(list).await;
    let items = page["items"].as_array().expect("page items");
    let item = items
        .iter()
        .find(|item| item["id"] == milestone.id.0.to_string())
        .unwrap_or_else(|| panic!("milestone missing from the list"));
    assert_eq!(item["owner"]["id"], owner.id.0.to_string());

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn clearing_the_milestone_owner_returns_null() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-clear"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Clear me"))
        .await
        .expect("create milestone");

    // An unowned milestone answers null...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["owner"], serde_json::Value::Null);

    // ...setting shows the person...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);
    assert_eq!(json_of(set).await["owner"]["id"], owner.id.0.to_string());

    // ...and clearing nulls it again, in the response and on the next GET.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json_of(cleared).await["owner"], serde_json::Value::Null);
    let get = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["owner"], serde_json::Value::Null);

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn setting_the_milestone_owner_twice_is_a_noop_with_an_unchanged_milestone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-repeat"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Steady"))
        .await
        .expect("create milestone");

    let uri = format!("/api/milestones/{}/owner", milestone.id.0);
    let first = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_of(first).await;

    // The same request again: 200, the same milestone, updated_at untouched.
    let second = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(second.status(), StatusCode::OK);
    let second = json_of(second).await;

    assert_eq!(second["updated_at"], first["updated_at"]);
    assert_eq!(second, first);

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn creating_a_milestone_with_an_owner_validates_the_user() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-create"), Role::Staff).await;
    let deactivated = create_user(&pool, unique_email("milestone-owner-create"), Role::Staff).await;
    deactivate_user(&pool, deactivated.id);
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // A valid owner: 201, and the response already shows them.
    let created = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({
            "title": unique_title("own-create"),
            "owner_id": owner.id.0.to_string()
        }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["active"], true);
    let created_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

    // An unknown user 404s with the distinct code...
    let unknown = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created",
            "owner_id": Uuid::new_v4().to_string()
        }))
    );
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let json = json_of(unknown).await;
    assert_eq!(json["error"]["code"], "user_not_found");

    // ...and a deactivated one 409s.
    let dead = request!(
        &app,
        Method::POST,
        "/api/milestones",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created either",
            "owner_id": deactivated.id.0.to_string()
        }))
    );
    assert_eq!(dead.status(), StatusCode::CONFLICT);
    let json = json_of(dead).await;
    assert_eq!(json["error"]["code"], "user_deactivated");

    milestones
        .delete(MilestoneId(created_id))
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
    delete_user(&pool, deactivated.id);
}

#[actix_web::test]
async fn an_ordinary_put_preserves_the_milestone_owner() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-put"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Keep me"))
        .await
        .expect("create milestone");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // An ordinary update carries no owner field and must neither change nor
    // clear it (D17).
    let updated = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed", "description": "New details" }))
    );
    assert_eq!(updated.status(), StatusCode::OK);
    let json = json_of(updated).await;
    assert_eq!(json["title"], "Renamed");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn a_status_override_set_and_clear_preserve_the_milestone_owner() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-override"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Override me"))
        .await
        .expect("create milestone");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // Setting an override is a different single-column write: the owner
    // survives it...
    let overridden = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "off_track" }))
    );
    assert_eq!(overridden.status(), StatusCode::OK);
    let json = json_of(overridden).await;
    assert_eq!(json["status"], "off_track");
    assert_eq!(json["status_source"], "manual");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    // ...and clearing it does too, with the status back to automatic.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    let json = json_of(cleared).await;
    assert_eq!(json["status"], "on_track");
    assert_eq!(json["status_source"], "automatic");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn changing_the_milestone_owner_preserves_the_override() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-keep"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Keep the override"))
        .await
        .expect("create milestone");

    let overridden = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/status-override", milestone.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "at_risk" }))
    );
    assert_eq!(overridden.status(), StatusCode::OK);

    // Naming an owner must not touch the sticky override (D17).
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);
    let json = json_of(set).await;
    assert_eq!(json["status"], "at_risk");
    assert_eq!(json["status_source"], "manual");
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn an_unknown_milestone_is_a_404_for_owner_writes() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-404"), Role::Staff).await;

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", Uuid::new_v4()),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::NOT_FOUND);
    let json = json_of(set).await;
    assert_eq!(json["error"]["code"], "not_found");

    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}/owner", Uuid::new_v4()),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::NOT_FOUND);

    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn deactivating_the_milestone_owner_leaves_them_shown_as_inactive() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let owner = create_user(&pool, unique_email("milestone-owner-dead"), Role::Staff).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone("Dead owner"))
        .await
        .expect("create milestone");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&owner))
    );
    assert_eq!(set.status(), StatusCode::OK);

    deactivate_user(&pool, owner.id);

    // The milestone still names them; the summary just reports the flag.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    let json = json_of(get).await;
    assert_eq!(json["owner"]["id"], owner.id.0.to_string());
    assert_eq!(json["owner"]["active"], false);

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, owner.id);
}

#[actix_web::test]
async fn the_link_lists_show_the_owners() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let goal_owner = create_user(&pool, unique_email("link-goal-owner"), Role::Staff).await;
    let milestone_owner = create_user(&pool, unique_email("link-ms-owner"), Role::Staff).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals
        .create(test_goal(&unique_title("ownlink")))
        .await
        .expect("create goal");
    let milestone = milestones
        .create(test_milestone(&unique_title("ownlink")))
        .await
        .expect("create milestone");

    // Link the pair...
    let link = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(link.status(), StatusCode::NO_CONTENT);

    // ...name an owner on each side...
    let set_goal = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/owner", goal.id.0),
        Some(&token),
        Some(&owner_body(&goal_owner))
    );
    assert_eq!(set_goal.status(), StatusCode::OK);
    let set_milestone = request!(
        &app,
        Method::PUT,
        format!("/api/milestones/{}/owner", milestone.id.0),
        Some(&token),
        Some(&owner_body(&milestone_owner))
    );
    assert_eq!(set_milestone.status(), StatusCode::OK);

    // ...and both link lists show their side's owner.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_of(list).await;
    let items = page["items"].as_array().expect("page items");
    assert_eq!(items.len(), 1, "the fresh goal has exactly one link");
    assert_eq!(items[0]["owner"]["id"], milestone_owner.id.0.to_string());

    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_of(list).await;
    let items = page["items"].as_array().expect("page items");
    assert_eq!(items.len(), 1, "the fresh milestone has exactly one link");
    assert_eq!(items[0]["owner"]["id"], goal_owner.id.0.to_string());

    // Deleting either end cascade-deletes the link with it.
    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
    delete_user(&pool, goal_owner.id);
    delete_user(&pool, milestone_owner.id);
}

#[actix_web::test]
async fn a_goal_list_page_resolves_owners_with_one_user_query() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);

    // The app's user repository counts its summaries_by_ids calls into this.
    let summary_lookups = Arc::new(AtomicUsize::new(0));
    let app = test_app!(&url, summary_lookups.clone());

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    // Three distinct owners on one page: the batching rule is only
    // interesting with more than one lookup to collapse.
    let mut people = Vec::new();
    for _ in 0..3 {
        people.push(create_user(&pool, unique_email("goal-one-query"), Role::Staff).await);
    }
    let prefix = unique_title("goalonequery");
    let mut created = Vec::new();
    for person in &people {
        let mut goal = test_goal(&format!("{prefix}-{}", Uuid::new_v4().simple()));
        goal.owner_id = Some(person.id);
        created.push(goals.create(goal).await.expect("create goal"));
    }

    let res = request!(
        &app,
        Method::GET,
        format!("/api/goals?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let page = json_of(res).await;
    let items = page["items"].as_array().expect("page items");
    assert_eq!(items.len(), 3, "the page holds exactly this test's goals");

    // Every owner resolved from the single batch... (order-independent: the
    // list order is target date, then created_at, then id).
    let owners: BTreeSet<String> = items
        .iter()
        .map(|item| item["owner"]["id"].as_str().unwrap().to_owned())
        .collect();
    let expected: BTreeSet<String> = people.iter().map(|p| p.id.0.to_string()).collect();
    assert_eq!(owners, expected);

    // ...and exactly one summaries_by_ids call served the whole page (D17).
    // Session resolution uses find_by_id, not summaries_by_ids, so the count
    // is the batch resolver's own.
    assert_eq!(summary_lookups.load(Ordering::SeqCst), 1);

    for goal in &created {
        goals.delete(goal.id).await.expect("cleanup goal");
    }
    delete_user(&pool, user.id);
    for person in &people {
        delete_user(&pool, person.id);
    }
}

#[actix_web::test]
async fn a_milestone_list_page_resolves_owners_with_one_user_query() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);

    // The app's user repository counts its summaries_by_ids calls into this.
    let summary_lookups = Arc::new(AtomicUsize::new(0));
    let app = test_app!(&url, summary_lookups.clone());

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    // Three distinct owners on one page: the batching rule is only
    // interesting with more than one lookup to collapse.
    let mut people = Vec::new();
    for _ in 0..3 {
        people.push(create_user(&pool, unique_email("milestone-one-query"), Role::Staff).await);
    }
    let prefix = unique_title("msonequery");
    let mut created = Vec::new();
    for person in &people {
        let mut milestone = test_milestone(&format!("{prefix}-{}", Uuid::new_v4().simple()));
        milestone.owner_id = Some(person.id);
        created.push(
            milestones
                .create(milestone)
                .await
                .expect("create milestone"),
        );
    }

    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let page = json_of(res).await;
    let items = page["items"].as_array().expect("page items");
    assert_eq!(
        items.len(),
        3,
        "the page holds exactly this test's milestones"
    );

    // Every owner resolved from the single batch... (order-independent: the
    // list order is target date, then created_at, then id).
    let owners: BTreeSet<String> = items
        .iter()
        .map(|item| item["owner"]["id"].as_str().unwrap().to_owned())
        .collect();
    let expected: BTreeSet<String> = people.iter().map(|p| p.id.0.to_string()).collect();
    assert_eq!(owners, expected);

    // ...and exactly one summaries_by_ids call served the whole page (D17).
    // Session resolution uses find_by_id, not summaries_by_ids, so the count
    // is the batch resolver's own.
    assert_eq!(summary_lookups.load(Ordering::SeqCst), 1);

    for milestone in &created {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
    for person in &people {
        delete_user(&pool, person.id);
    }
}
