//! Handler tests for the task assignee endpoints and the people picker
//! (roadmap 3.11, D17), against a real Postgres: every task response carries
//! the assignee as a person summary (`null` when unassigned), setting shows
//! it in the PUT response, the GET and the list, clearing nulls it, setting
//! twice is a no-op that leaves `updated_at` alone, creation validates its
//! `assignee_id` exactly like the set endpoint (unknown 404s, deactivated
//! 409s), an ordinary PUT and a status PATCH never touch it, a deactivated
//! assignee still renders with `active` false (and can still be cleared),
//! `GET /api/people` lists active users only in display-name order without
//! ever exposing the email, and a list page resolves every assignee with one
//! user query.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::assignment::AssignmentService;
use application::auth::SessionService;
use application::ports::{
    AccessChange, AccessChangeError, RepositoryError, SessionRepository, TaskRepository,
    UserRepository,
};
use application::task_list::TaskListService;
use application::task_status::TaskStatusService;
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use diesel::prelude::*;
use domain::{Role, Task, TaskId, TaskStatus, User, UserId, UserSummary};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalRepository, PostgresMilestoneRepository, PostgresSessionRepository,
    PostgresTaskRepository, PostgresUserRepository,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
        panic!("DATABASE_URL is not set; refusing to skip assignee tests in CI");
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

/// Build the app with the real route table and only the `web::Data` the task
/// handlers (and the people route these tests read through) can need; the
/// other routes 500 if hit, but these tests never do. A macro because
/// `init_service`'s service type is opaque. The user repository is always
/// the counting wrapper: one test asserts the single-query batching rule on
/// its counter, the rest pass a throwaway.
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
        // instances over the shared pool (mirrors `main.rs`).
        let status = TaskStatusService::new(Arc::new(PostgresTaskRepository::new(pool.clone())));
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(status))
                .app_data(web::Data::new(TaskListService::new(Arc::new(
                    PostgresTaskRepository::new(pool.clone()),
                ))))
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

/// A task title no other test can share, safe to use as a `q=` filter value
/// (one token, no characters that need URL encoding).
fn unique_title(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

/// A user created directly in the database (no sign-in needed: sessions are
/// issued straight from the [`SessionService`]).
async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
    create_named_user(pool, email, role, "Task assignee test user").await
}

/// As [`create_user`] with a chosen display name (the people-ordering test
/// needs distinct names).
async fn create_named_user(pool: &PgPool, email: String, role: Role, display_name: &str) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: None,
        display_name: display_name.to_owned(),
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
        assignee_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("task-assignee"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A `PUT /api/tasks/{id}/assignee` body for the given user.
fn assign_body(user: &User) -> serde_json::Value {
    serde_json::json!({ "assignee_id": user.id.0.to_string() })
}

/// A user repository that counts its `summaries_by_ids` calls and delegates
/// to Postgres: proves a list page resolves every assignee or owner with one
/// query (D17). `pub` so the owner tests reuse it.
pub struct CountingUserRepository {
    pub inner: Arc<PostgresUserRepository>,
    pub summary_lookups: Arc<AtomicUsize>,
}

#[async_trait]
impl UserRepository for CountingUserRepository {
    async fn create(&self, user: User) -> Result<User, RepositoryError> {
        self.inner.create(user).await
    }
    async fn create_if_no_users(&self, user: User) -> Result<Option<User>, RepositoryError> {
        self.inner.create_if_no_users(user).await
    }
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        self.inner.find_by_id(id).await
    }
    async fn find_by_email(&self, email: String) -> Result<Option<User>, RepositoryError> {
        self.inner.find_by_email(email).await
    }
    async fn list(&self) -> Result<Vec<User>, RepositoryError> {
        self.inner.list().await
    }
    async fn summaries_by_ids(&self, ids: &[UserId]) -> Result<Vec<UserSummary>, RepositoryError> {
        self.summary_lookups.fetch_add(1, Ordering::SeqCst);
        self.inner.summaries_by_ids(ids).await
    }
    async fn list_active_summaries(&self) -> Result<Vec<UserSummary>, RepositoryError> {
        self.inner.list_active_summaries().await
    }
    async fn update(&self, user: User) -> Result<User, RepositoryError> {
        self.inner.update(user).await
    }
    async fn apply_access_change(
        &self,
        target: UserId,
        change: AccessChange,
    ) -> Result<User, AccessChangeError> {
        self.inner.apply_access_change(target, change).await
    }
}

#[actix_web::test]
async fn setting_shows_the_assignee_in_the_response_get_and_list() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-set"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let title = unique_title("asgn");
    let task = tasks.create(test_task(&title)).await.expect("create task");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // The response is the standard task shape plus the assignee summary.
    assert_eq!(json["id"], task.id.0.to_string());
    assert_eq!(json["assignee"]["id"], assignee.id.0.to_string());
    assert_eq!(json["assignee"]["display_name"], "Task assignee test user");
    assert_eq!(json["assignee"]["active"], true);

    // The following GET agrees...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        json_of(get).await["assignee"]["id"],
        assignee.id.0.to_string()
    );

    // ...and the list shows it on the item (filtered to this test's task).
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks?q={title}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let page = json_of(list).await;
    let items = page["items"].as_array().expect("page items");
    let item = items
        .iter()
        .find(|item| item["id"] == task.id.0.to_string())
        .unwrap_or_else(|| panic!("task missing from the list"));
    assert_eq!(item["assignee"]["id"], assignee.id.0.to_string());

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn clearing_returns_null() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-clear"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Clear me"))
        .await
        .expect("create task");

    // An unassigned task answers null...
    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["assignee"], serde_json::Value::Null);

    // ...setting shows the person...
    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::OK);
    assert_eq!(
        json_of(set).await["assignee"]["id"],
        assignee.id.0.to_string()
    );

    // ...and clearing nulls it again, in the response and on the next GET.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json_of(cleared).await["assignee"], serde_json::Value::Null);
    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["assignee"], serde_json::Value::Null);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn setting_twice_is_a_noop_with_an_unchanged_task() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-repeat"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Steady"))
        .await
        .expect("create task");

    let uri = format!("/api/tasks/{}/assignee", task.id.0);
    let first = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_of(first).await;

    // The same request again: 200, the same task, updated_at untouched.
    let second = request!(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(second.status(), StatusCode::OK);
    let second = json_of(second).await;

    assert_eq!(second["updated_at"], first["updated_at"]);
    assert_eq!(second, first);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn creating_with_an_assignee_validates_the_user() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-create"), Role::Staff).await;
    let deactivated = create_user(&pool, unique_email("assignee-create"), Role::Staff).await;
    deactivate_user(&pool, deactivated.id);
    let tasks = PostgresTaskRepository::new(pool.clone());

    // A valid assignee: 201, and the response already shows them.
    let created = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(&serde_json::json!({
            "title": unique_title("asgn-create"),
            "status": "backlog",
            "assignee_id": assignee.id.0.to_string()
        }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let json = json_of(created).await;
    assert_eq!(json["assignee"]["id"], assignee.id.0.to_string());
    assert_eq!(json["assignee"]["active"], true);
    let created_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

    // An unknown user 404s with the distinct code...
    let unknown = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created",
            "status": "backlog",
            "assignee_id": Uuid::new_v4().to_string()
        }))
    );
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let json = json_of(unknown).await;
    assert_eq!(json["error"]["code"], "user_not_found");

    // ...and a deactivated one 409s.
    let dead = request!(
        &app,
        Method::POST,
        "/api/tasks",
        Some(&token),
        Some(&serde_json::json!({
            "title": "Never created either",
            "status": "backlog",
            "assignee_id": deactivated.id.0.to_string()
        }))
    );
    assert_eq!(dead.status(), StatusCode::CONFLICT);
    let json = json_of(dead).await;
    assert_eq!(json["error"]["code"], "user_deactivated");

    tasks
        .delete(TaskId(created_id))
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
    delete_user(&pool, deactivated.id);
}

#[actix_web::test]
async fn an_ordinary_put_preserves_the_assignee() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-put"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Keep me"))
        .await
        .expect("create task");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::OK);

    // An ordinary update carries no assignee field and must neither change
    // nor clear it (D17).
    let updated = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "title": "Renamed", "status": "to_do" }))
    );
    assert_eq!(updated.status(), StatusCode::OK);
    let json = json_of(updated).await;
    assert_eq!(json["title"], "Renamed");
    assert_eq!(json["assignee"]["id"], assignee.id.0.to_string());

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn a_status_patch_preserves_the_assignee() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-patch"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Move me"))
        .await
        .expect("create task");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::OK);

    let moved = request!(
        &app,
        Method::PATCH,
        format!("/api/tasks/{}/status", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "status": "in_progress" }))
    );
    assert_eq!(moved.status(), StatusCode::OK);
    let json = json_of(moved).await;
    assert_eq!(json["status"], "in_progress");
    assert_eq!(json["assignee"]["id"], assignee.id.0.to_string());

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn deactivating_the_assignee_leaves_them_shown_as_inactive() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-dead"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Dead assignee"))
        .await
        .expect("create task");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::OK);

    deactivate_user(&pool, assignee.id);

    // The task still names them; the summary just reports the flag.
    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    let json = json_of(get).await;
    assert_eq!(json["assignee"]["id"], assignee.id.0.to_string());
    assert_eq!(json["assignee"]["active"], false);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn reassigning_to_an_unknown_or_deactivated_user_is_refused() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let deactivated = create_user(&pool, unique_email("assignee-reject"), Role::Staff).await;
    deactivate_user(&pool, deactivated.id);
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Refuse me"))
        .await
        .expect("create task");

    // Unknown user: 404 with the distinct code...
    let unknown = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "assignee_id": Uuid::new_v4().to_string() }))
    );
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let json = json_of(unknown).await;
    assert_eq!(json["error"]["code"], "user_not_found");

    // Deactivated user: 409 with the distinct code. The task is untouched.
    let dead = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "assignee_id": deactivated.id.0.to_string() }))
    );
    assert_eq!(dead.status(), StatusCode::CONFLICT);
    let json = json_of(dead).await;
    assert_eq!(json["error"]["code"], "user_deactivated");

    let get = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(get).await["assignee"], serde_json::Value::Null);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, deactivated.id);
}

#[actix_web::test]
async fn clearing_works_with_a_deactivated_assignee() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-clear-dead"), Role::Staff).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Clear dead"))
        .await
        .expect("create task");

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::OK);

    deactivate_user(&pool, assignee.id);

    // No user lookup on the way out: clearing succeeds anyway.
    let cleared = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json_of(cleared).await["assignee"], serde_json::Value::Null);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn an_unknown_task_is_a_404() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let assignee = create_user(&pool, unique_email("assignee-404"), Role::Staff).await;

    let set = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", Uuid::new_v4()),
        Some(&token),
        Some(&assign_body(&assignee))
    );
    assert_eq!(set.status(), StatusCode::NOT_FOUND);
    let json = json_of(set).await;
    assert_eq!(json["error"]["code"], "not_found");

    let clear = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/assignee", Uuid::new_v4()),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(clear.status(), StatusCode::NOT_FOUND);

    delete_user(&pool, user.id);
    delete_user(&pool, assignee.id);
}

#[actix_web::test]
async fn a_malformed_assignee_body_is_a_400() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    let task = tasks
        .create(test_task("Bogus body"))
        .await
        .expect("create task");

    // A value that is not a UUID fails deserialization: the standard 400.
    let bad_uuid = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "assignee_id": "not-a-uuid" }))
    );
    assert_eq!(bad_uuid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(bad_uuid).await["error"]["code"], "bad_request");

    // A missing field fails too.
    let missing = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/assignee", task.id.0),
        Some(&token),
        Some(&serde_json::json!({}))
    );
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    tasks.delete(task.id).await.expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn people_excludes_deactivated_users_is_ordered_and_never_shows_the_email() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    // A read-only session: the picker is readable by every signed-in role.
    let viewer = create_user(&pool, unique_email("people-view"), Role::ReadOnly).await;
    let token = issue_token(&pool, viewer.id).await;

    // Names chosen so only a case-insensitive order puts `lower` first:
    // case-sensitively the uppercase name would sort ahead of it.
    let upper = create_named_user(
        &pool,
        unique_email("people-order"),
        Role::Staff,
        &format!("Zzz-{}", Uuid::new_v4().simple()),
    )
    .await;
    let lower = create_named_user(
        &pool,
        unique_email("people-order"),
        Role::Staff,
        &format!("aaa-{}", Uuid::new_v4().simple()),
    )
    .await;
    let deactivated = create_user(&pool, unique_email("people-dead"), Role::Staff).await;
    deactivate_user(&pool, deactivated.id);

    let res = request!(
        &app,
        Method::GET,
        "/api/people",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    let list = json.as_array().expect("a JSON array");

    // Every entry is exactly the person summary — no email, nothing else.
    for entry in list {
        let keys: std::collections::BTreeSet<&str> = entry
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["active", "display_name", "id"].into_iter().collect());
    }

    // Both active users appear, in case-insensitive display-name order...
    let position = |id: UserId| {
        list.iter()
            .position(|entry| entry["id"] == id.0.to_string())
            .unwrap_or_else(|| panic!("{} missing from the people list", id.0))
    };
    assert!(
        position(lower.id) < position(upper.id),
        "the order must be case-insensitive: {} before {}",
        lower.display_name,
        upper.display_name
    );

    // ...and the deactivated account is absent.
    assert!(
        !list
            .iter()
            .any(|entry| entry["id"] == deactivated.id.0.to_string()),
        "deactivated accounts must not be listed"
    );

    delete_user(&pool, viewer.id);
    delete_user(&pool, upper.id);
    delete_user(&pool, lower.id);
    delete_user(&pool, deactivated.id);
}

#[actix_web::test]
async fn a_list_page_resolves_summaries_with_one_user_query() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);

    // The app's user repository counts its summaries_by_ids calls into this.
    let summary_lookups = Arc::new(AtomicUsize::new(0));
    let app = test_app!(&url, summary_lookups.clone());

    let (user, token) = staff(&pool).await;
    let tasks = PostgresTaskRepository::new(pool.clone());
    // Three distinct assignees on one page: the batching rule is only
    // interesting with more than one lookup to collapse.
    let mut people = Vec::new();
    for _ in 0..3 {
        people.push(create_user(&pool, unique_email("one-query"), Role::Staff).await);
    }
    let prefix = unique_title("onequery");
    let mut created = Vec::new();
    for person in &people {
        let mut task = test_task(&format!("{prefix}-{}", Uuid::new_v4().simple()));
        task.assignee_id = Some(person.id);
        created.push(tasks.create(task).await.expect("create task"));
    }

    let res = request!(
        &app,
        Method::GET,
        format!("/api/tasks?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let page = json_of(res).await;
    let items = page["items"].as_array().expect("page items");
    assert_eq!(items.len(), 3, "the page holds exactly this test's tasks");

    // Every assignee resolved from the single batch...
    for (item, person) in items.iter().zip(&people) {
        assert_eq!(item["assignee"]["id"], person.id.0.to_string());
    }

    // ...and exactly one summaries_by_ids call served the whole page (D17).
    // Session resolution uses find_by_id, not summaries_by_ids, so the count
    // is the batch resolver's own.
    assert_eq!(summary_lookups.load(Ordering::SeqCst), 1);

    for task in &created {
        tasks.delete(task.id).await.expect("cleanup task");
    }
    delete_user(&pool, user.id);
    for person in &people {
        delete_user(&pool, person.id);
    }
}
