//! Handler tests for the task-relation endpoints (roadmap 3.6), against a
//! real Postgres: a `blocks` relation shows up as `blocked_by` on the other
//! side, duplicates and reverse-blocking pairs are rejected with distinct
//! 409 codes, self-relations and unknown types are 400s, unknown tasks are
//! 404s that name which task was missing, deleting removes the relation from
//! both sides (and deleting a task drops its relations), the listed related
//! task carries its current status, and the lists are deterministically
//! ordered.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::ports::{
    SessionRepository, TaskRelationRepository, TaskRepository, UserRepository,
};
use application::task_relations::TaskRelationService;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{Role, Task, TaskId, TaskRelation, TaskRelationType, TaskStatus, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresSessionRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
    PostgresUserRepository,
};
use std::collections::BTreeSet;
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
        panic!("DATABASE_URL is not set; refusing to skip task-relation tests in CI");
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
/// relation handlers (and the existing task routes these tests use to change
/// a status or drop an end of a relation) can need; the other routes 500 if
/// hit, but these tests never do. A macro because `init_service`'s service
/// type is opaque.
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
        // The relation service takes ports, so it gets its own repository
        // instances over the shared pool (mirrors `main.rs`).
        let relations = TaskRelationService::new(
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(PostgresTaskRelationRepository::new(pool.clone())),
        );
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(relations))
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
        display_name: "Relation test user".into(),
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
        description: None,
        status: TaskStatus::ToDo,
        target_date: None,
        created_at: now,
        updated_at: now,
    }
}

/// A relation request body: the type from the path task's perspective.
fn relation_body(relation_type: &str, related_task_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "relation_type": relation_type,
        "related_task_id": related_task_id.to_string()
    })
}

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("relations"), Role::Staff).await;
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
async fn blocks_a_to_b_shows_blocks_in_a_and_blocked_by_in_b() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let json = json_of(res).await;
    assert_eq!(json["relation_type"], "blocks");
    assert_eq!(json["related_task"]["id"], b.id.0.to_string());
    assert_eq!(json["related_task"]["title"], "B");
    assert_eq!(json["related_task"]["status"], "to_do");

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["relation_type"], "blocks");
    assert_eq!(entries[0]["related_task"]["id"], b.id.0.to_string());

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", b.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["relation_type"], "blocked_by");
    assert_eq!(entries[0]["related_task"]["id"], a.id.0.to_string());

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn blocked_by_and_reverse_blocks_are_rejected_when_the_other_side_blocks() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    // A blocks B exists...
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);

    // ...so "B is blocked by A" is the same relationship: 409 relation_exists.
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", b.id.0),
        Some(&token),
        Some(&relation_body("blocked_by", a.id.0))
    );
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(res).await["error"]["code"], "relation_exists");

    // And "B blocks A" is the reverse: 409 reverse_relation_exists.
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", b.id.0),
        Some(&token),
        Some(&relation_body("blocks", a.id.0))
    );
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let json = json_of(res).await;
    assert_eq!(json["error"]["code"], "reverse_relation_exists");

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_same_blocks_twice_is_a_conflict() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    for attempt in 0..2 {
        let res = request!(
            &app,
            Method::POST,
            format!("/api/tasks/{}/relations", a.id.0),
            Some(&token),
            Some(&relation_body("blocks", b.id.0))
        );
        if attempt == 0 {
            assert_eq!(res.status(), StatusCode::CREATED);
        } else {
            assert_eq!(res.status(), StatusCode::CONFLICT);
            assert_eq!(json_of(res).await["error"]["code"], "relation_exists");
        }
    }

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn relates_to_appears_on_both_sides_and_the_second_create_conflicts() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("relates_to", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(json_of(res).await["relation_type"], "relates_to");

    // Both ends read the symmetric relation, pointing at the other task.
    for (task, other) in [(a.id, b.id), (b.id, a.id)] {
        let list = request!(
            &app,
            Method::GET,
            format!("/api/tasks/{}/relations", task.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json = json_of(list).await;
        let entries = json.as_array().expect("a JSON array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["relation_type"], "relates_to");
        assert_eq!(entries[0]["related_task"]["id"], other.0.to_string());
    }

    // The second submission of the same relationship conflicts.
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", b.id.0),
        Some(&token),
        Some(&relation_body("relates_to", a.id.0))
    );
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json_of(res).await["error"]["code"], "relation_exists");

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_self_relation_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", a.id.0))
    );
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(res).await["error"]["code"], "self_relation");

    tasks.delete(a.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn unknown_tasks_are_404s_that_name_which_task() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");
    let missing = Uuid::new_v4();

    // Unknown path task (a real related task on the other side).
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{missing}/relations"),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let json = json_of(res).await;
    assert_eq!(json["error"]["code"], "not_found");
    assert_eq!(json["error"]["message"], "task was not found");

    // Known path task, unknown related task.
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", missing))
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let json = json_of(res).await;
    assert_eq!(json["error"]["code"], "not_found");
    assert_eq!(json["error"]["message"], "related task was not found");

    // GET and DELETE with an unknown path task.
    let res = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{missing}/relations"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let _ = read_body(res).await;
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{missing}/relations/{}", Uuid::new_v4()),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let _ = read_body(res).await;

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_relation_type_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    // The body fails to parse before the handler runs: the standard 400.
    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("follows", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(res).await["error"]["code"], "bad_request");

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_removes_the_relation_from_both_lists() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let relation_id = json_of(res).await["id"]
        .as_str()
        .expect("an id string")
        .to_owned();

    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/relations/{relation_id}", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    for task in [a.id, b.id] {
        let list = request!(
            &app,
            Method::GET,
            format!("/api/tasks/{}/relations", task.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json = json_of(list).await;
        let entries = json.as_array().expect("a JSON array");
        assert!(entries.is_empty());
    }

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_a_relation_from_the_wrong_task_or_an_unknown_id_is_404() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");
    let c = tasks.create(test_task("C")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let relation_id = json_of(res).await["id"]
        .as_str()
        .expect("an id string")
        .to_owned();

    // A relation that exists but does not involve the path task.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/relations/{relation_id}", c.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(res).await["error"]["code"], "relation_not_found");

    // An unknown relation id under an existing task.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/relations/{}", a.id.0, Uuid::new_v4()),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(res).await["error"]["code"], "relation_not_found");

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    tasks.delete(c.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn blocks_and_relates_to_between_the_same_tasks_coexist() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    for relation_type in ["blocks", "relates_to"] {
        let res = request!(
            &app,
            Method::POST,
            format!("/api/tasks/{}/relations", a.id.0),
            Some(&token),
            Some(&relation_body(relation_type, b.id.0))
        );
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["related_task"]["id"], b.id.0.to_string());
    }
    let types: BTreeSet<&str> = entries
        .iter()
        .map(|entry| entry["relation_type"].as_str().expect("a type string"))
        .collect();
    assert_eq!(types, ["blocks", "relates_to"].into_iter().collect());

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_a_task_drops_its_relations_from_the_other_side() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);

    // Delete the task through the existing route: its relations cascade away.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", b.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    assert!(entries.is_empty());

    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn lists_order_by_creation_time_then_id() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");
    let c = tasks.create(test_task("C")).await.expect("create task");

    // Seed directly with controlled timestamps: a blocks b (a is source),
    // c blocks a (a is target), a relates to c.
    let relations = PostgresTaskRelationRepository::new(pool.clone());
    relations
        .create(TaskRelation::new(
            a.id,
            b.id,
            TaskRelationType::Blocks,
            at(1, 1),
        ))
        .await
        .expect("seed relation");
    relations
        .create(TaskRelation::new(
            c.id,
            a.id,
            TaskRelationType::Blocks,
            at(1, 2),
        ))
        .await
        .expect("seed relation");
    relations
        .create(TaskRelation::new(
            a.id,
            c.id,
            TaskRelationType::RelatesTo,
            at(1, 3),
        ))
        .await
        .expect("seed relation");

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    // The three seeded rows in creation order, each from a's perspective.
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["relation_type"], "blocks");
    assert_eq!(entries[0]["related_task"]["id"], b.id.0.to_string());
    assert_eq!(entries[1]["relation_type"], "blocked_by");
    assert_eq!(entries[1]["related_task"]["id"], c.id.0.to_string());
    assert_eq!(entries[2]["relation_type"], "relates_to");
    assert_eq!(entries[2]["related_task"]["id"], c.id.0.to_string());

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    tasks.delete(c.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_related_task_summary_reports_its_current_status() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let a = tasks.create(test_task("A")).await.expect("create task");
    let b = tasks.create(test_task("B")).await.expect("create task");

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        Some(&relation_body("blocks", b.id.0))
    );
    assert_eq!(res.status(), StatusCode::CREATED);

    // Change the related task's status through the existing task route...
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}", b.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "B", "status": "done" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let _ = read_body(res).await;

    // ...and the list entry reports the current one.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/relations", a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entries = json.as_array().expect("a JSON array");
    assert_eq!(entries[0]["related_task"]["status"], "done");

    tasks.delete(a.id).await.expect("cleanup task");
    tasks.delete(b.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}
