//! Handler tests for the board-state transition endpoint (roadmap 3.7),
//! against a real Postgres: `PATCH /api/tasks/{id}/status` moves a card
//! between columns and answers with the standard task shape, any column may
//! move to any other (including Done back to Backlog), repeating the same
//! request is a harmless no-op that leaves `updated_at` alone, a blocked task
//! moves just like any other, the task's other fields are untouched, an
//! unknown task is a 404 and an unknown status string a 400.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::ports::{MilestoneRepository, SessionRepository, TaskRepository, UserRepository};
use application::task_status::TaskStatusService;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{MilestoneId, Role, Task, TaskId, TaskStatus, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresMilestoneRepository, PostgresSessionRepository, PostgresTaskRepository,
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
        panic!("DATABASE_URL is not set; refusing to skip task-status tests in CI");
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

/// Build the app with the real route table and only the `web::Data` the status
/// handler (and the existing task routes these tests read through) can need;
/// the other routes 500 if hit, but these tests never do. A macro because
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
        // The status service takes ports, so it gets its own repository
        // instance over the shared pool (mirrors `main.rs`).
        let status = TaskStatusService::new(Arc::new(PostgresTaskRepository::new(pool.clone())));
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(status))
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

/// A user created directly in the database (no sign-in needed: sessions are
/// issued straight from the [`SessionService`]).
async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: None,
        display_name: "Task status test user".into(),
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

/// A task with the given title and no milestone.
fn test_task(title: &str) -> Task {
    let now = Utc::now();
    Task {
        id: TaskId::new(),
        milestone_id: None,
        title: title.to_owned(),
        description: Some("Some details".to_owned()),
        status: TaskStatus::Backlog,
        target_date: Some(NaiveDate::from_ymd_opt(2026, 6, 1).expect("valid date")),
        created_at: now,
        updated_at: now,
    }
}

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("task-status"), Role::Staff).await;
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

#[actix_web::test]
async fn patch_moves_a_task_and_the_get_agrees() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Move me"))
        .await
        .expect("create task");

    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "in_progress" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // The response is the standard task shape, with the new column.
    assert_eq!(json["id"], task.id.0.to_string());
    assert!(json["milestone_id"].is_null());
    assert_eq!(json["title"], "Move me");
    assert_eq!(json["status"], "in_progress");

    // The following GET agrees.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(json_of(get).await["status"], "in_progress");

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_response_keeps_the_standard_task_shape() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks.create(test_task("Shape")).await.expect("create task");

    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "to_do" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // Exactly the fields of the existing task response — no new ones.
    let keys: std::collections::BTreeSet<&str> = json
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "created_at",
            "description",
            "id",
            "milestone_id",
            "status",
            "target_date",
            "title",
            "updated_at"
        ]
        .into_iter()
        .collect()
    );

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn any_column_may_move_to_any_other_including_done_back_to_backlog() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Anywhere"))
        .await
        .expect("create task");

    // Backlog -> in progress -> done, then Done back to the start of the board.
    for status in ["in_progress", "done", "backlog"] {
        let res = request!(
            &app,
            Method::PATCH,
            format!("/api/tasks/{}/status", task.id.0),
            Some(&token),
            Some(&serde_json::json!({ "status": status }))
        );
        assert_eq!(res.status(), StatusCode::OK, "move to {status}");
        assert_eq!(json_of(res).await["status"], status);
    }

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn repeating_the_same_patch_is_a_noop_with_an_unchanged_task() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Steady"))
        .await
        .expect("create task");

    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "to_do" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let first = json_of(res).await;

    // The same request again: 200, the same task, updated_at untouched.
    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "to_do" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let second = json_of(res).await;

    assert_eq!(second["status"], "to_do");
    assert_eq!(second["updated_at"], first["updated_at"]);
    assert_eq!(second, first);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_blocked_task_moves_fine() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let blocker = tasks
        .create(test_task("Blocker"))
        .await
        .expect("create task");
    let blocked = tasks
        .create(test_task("Blocked"))
        .await
        .expect("create task");

    // The blocker is not Done, so the second task is blocked...
    assert!(blocked.is_blocked(std::slice::from_ref(&blocker)));

    // ...but the column move goes through anyway (D6: a warning, not a stop).
    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", blocked.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "done" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_of(res).await["status"], "done");

    tasks.delete(blocker.id).await.expect("cleanup task");
    tasks.delete(blocked.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_task_is_a_404() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;

    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", Uuid::new_v4()),
        Some(&token),
        Some(&serde_json::json!({ "status": "done" }))
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let json = json_of(res).await;
    assert_eq!(json["error"]["code"], "not_found");

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_status_string_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks.create(test_task("Bogus")).await.expect("create task");

    // The body fails to parse before the handler runs: the standard 400.
    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "finished" }))
    );
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(res).await["error"]["code"], "bad_request");

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_other_fields_are_untouched_by_a_column_move() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let milestones_repo = PostgresMilestoneRepository::new(pool.clone());
    let milestone = domain::Milestone {
        id: MilestoneId::new(),
        title: "Status test milestone".into(),
        description: None,
        status: domain::Status::OnTrack,
        status_override: None,
        target_date: Some(NaiveDate::from_ymd_opt(2026, 9, 1).expect("valid date")),
        created_at: at(1, 1),
        updated_at: at(1, 1),
    };
    milestones_repo
        .create(milestone.clone())
        .await
        .expect("create milestone");

    let tasks = PostgresTaskRepository::new(pool.clone());
    let mut task = test_task("Untouched");
    task.milestone_id = Some(milestone.id);
    // Fixed timestamps: Postgres keeps microseconds, so a Utc::now() value
    // would not round-trip exactly into the JSON assertion below.
    task.created_at = at(1, 1);
    task.updated_at = at(1, 1);
    tasks.create(task.clone()).await.expect("create task");

    let res = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "in_progress" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    assert_eq!(json["status"], "in_progress");
    assert_eq!(json["title"], "Untouched");
    assert_eq!(json["description"], "Some details");
    assert_eq!(json["target_date"], "2026-06-01");
    assert_eq!(json["milestone_id"], milestone.id.0.to_string());
    assert_eq!(json["created_at"], "2026-01-01T09:00:00Z");

    tasks.delete(task.id).await.expect("cleanup task");
    milestones_repo
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}
