//! Task-comment API tests against Postgres (roadmap 3.12, D20): the create /
//! read / edit / delete handlers end to end — the author summary in every
//! response, oldest-first paging, the ownership rules (author edits,
//! author-or-Admin deletes), the no-op edit, and the rule that commenting
//! touches nothing but the comment row.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::ports::{
    NoopStatusSnapshotTrigger, SessionRepository, SystemClock, TaskCommentRepository,
    TaskRepository, UserRepository,
};
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::task_comments::TaskCommentService;
use application::task_service::TaskService;
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use domain::{Role, Task, TaskComment, TaskCommentId, TaskId, TaskStatus, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresSessionRepository, PostgresTaskCommentRepository, PostgresTaskRepository,
    PostgresUserRepository,
};
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
        panic!("DATABASE_URL is not set; refusing to skip task-comment tests in CI");
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
/// comment handlers (and the task routes these tests clean up through) can
/// need; the other routes 500 if hit, but these tests never do. A macro
/// because `init_service`'s service is opaque. The user repository is always
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
        // The mutation services recompute statuses best-effort after each
        // write (roadmap 3.13), so they share the real computation service.
        // The comment service takes it too — and never calls it (D20).
        let computation: Arc<dyn StatusRecomputation> = Arc::new(StatusComputationService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            Arc::new(SystemClock),
        ));
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(TaskService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    computation.clone(),
                )))
                .app_data(web::Data::new(TaskCommentService::new(
                    Arc::new(PostgresTaskRepository::new(pool.clone())),
                    Arc::new(PostgresTaskCommentRepository::new(pool.clone())),
                    Arc::new(NoopStatusSnapshotTrigger),
                    computation,
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
        display_name: "Task comment test user".into(),
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

/// A user of the given role with a live session token.
async fn seeded_user(pool: &PgPool, role: Role) -> (User, String) {
    let user = create_user(pool, unique_email("task-comment"), role).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A task with a unique title, created directly in the database.
async fn create_task(pool: &PgPool, title: &str) -> Task {
    let now = Utc::now();
    let task = Task {
        id: TaskId::new(),
        milestone_id: None,
        title: title.to_owned(),
        description: None,
        status: TaskStatus::Backlog,
        target_date: None,
        assignee_id: None,
        created_at: now,
        updated_at: now,
    };
    PostgresTaskRepository::new(pool.clone())
        .create(task)
        .await
        .expect("create task")
}

/// A comment inserted directly with a controlled timestamp (the API sets
/// `created_at` to now, which the ordering tests cannot rely on).
async fn seed_comment(
    pool: &PgPool,
    task_id: TaskId,
    author: Option<UserId>,
    body: &str,
    created_at: DateTime<Utc>,
) -> TaskComment {
    let comment = TaskComment {
        id: TaskCommentId::new(),
        task_id,
        author_id: author,
        body: body.to_owned(),
        created_at,
        updated_at: created_at,
    };
    PostgresTaskCommentRepository::new(pool.clone())
        .create(comment)
        .await
        .expect("seed comment")
}

/// The `created_at` of a fixed day in 2026, exact to the second.
fn at(day: u32) -> DateTime<Utc> {
    chrono::NaiveDate::from_ymd_opt(2026, 1, day)
        .expect("valid date")
        .and_hms_opt(9, 0, 0)
        .expect("valid time")
        .and_utc()
}

/// The comment ids of a page's items, in the order the API returned them.
fn item_ids(json: &serde_json::Value) -> Vec<Uuid> {
    json["items"]
        .as_array()
        .expect("an items array")
        .iter()
        .map(|item| Uuid::parse_str(item["id"].as_str().expect("an id string")).expect("a uuid"))
        .collect()
}

/// A fresh, unique task title so every assertion can filter to this test's
/// rows (the test database is shared across parallel runs).
fn unique_title(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

#[actix_web::test]
async fn create_shows_the_author_summary_equal_timestamps_and_appears_in_the_list() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (author, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-create")).await;

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "  First note  " }))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let json = json_of(res).await;

    // The body is trimmed, the author is a person summary (never the email),
    // and created_at equals updated_at until edited.
    assert_eq!(json["body"], "First note");
    assert_eq!(json["author"]["id"], author.id.0.to_string());
    assert_eq!(json["author"]["display_name"], "Task comment test user");
    assert_eq!(json["author"]["active"], true);
    assert!(
        json.get("email").is_none(),
        "the email is never part of the summary"
    );
    assert_eq!(json["created_at"], json["updated_at"]);
    assert_eq!(json["task_id"], task.id.0.to_string());

    // The new comment appears in the list, oldest first (it is the only one).
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = json_of(list).await;
    assert_eq!(list_json["total"], 1);
    assert_eq!(
        item_ids(&list_json),
        vec![Uuid::parse_str(json["id"].as_str().expect("an id")).expect("a uuid")]
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, author.id);
}

#[actix_web::test]
async fn the_list_is_oldest_first_and_a_paging_walk_reassembles_the_thread_once() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-order")).await;

    // Seed five comments out of order: the stored order is not the thread
    // order, so oldest-first ordering is what the API must supply.
    let bodies = ["e", "a", "c", "b", "d"];
    let days = [5u32, 1, 3, 2, 4];
    let mut by_day = Vec::new();
    for (body, day) in bodies.iter().zip(days.iter()) {
        by_day.push((
            seed_comment(&pool, task.id, Some(user.id), body, at(*day)).await,
            *day,
        ));
    }
    // The expected thread order: oldest (day 1) to newest (day 5).
    let mut pairs: Vec<(Uuid, u32)> = by_day.iter().map(|(c, day)| (c.id.0, *day)).collect();
    pairs.sort_by_key(|(_, day)| *day);
    let expected: Vec<Uuid> = pairs.into_iter().map(|(id, _)| id).collect();

    // Walk the thread in pages of two and reassemble it.
    let mut seen = Vec::new();
    for offset in [0u64, 2, 4] {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/tasks/{}/comments?limit=2&offset={offset}", task.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK);
        let json = json_of(res).await;
        assert_eq!(json["total"], 5, "the total ignores paging");
        seen.extend(item_ids(&json));
    }
    assert_eq!(seen, expected, "every comment exactly once, oldest first");

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_total_counts_only_that_tasks_comments() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task_a = create_task(&pool, &unique_title("tc-total-a")).await;
    let task_b = create_task(&pool, &unique_title("tc-total-b")).await;

    for day in 1..=3 {
        seed_comment(&pool, task_a.id, Some(user.id), "a", at(day)).await;
    }
    for day in 1..=2 {
        seed_comment(&pool, task_b.id, Some(user.id), "b", at(day)).await;
    }

    let list_a = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task_a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(list_a).await["total"], 3);

    let list_b = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task_b.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(list_b).await["total"], 2);

    for task in [task_a, task_b] {
        PostgresTaskRepository::new(pool.clone())
            .delete(task.id)
            .await
            .expect("cleanup task");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_default_limit_is_50_and_invalid_values_are_400s_naming_the_parameter() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-limit")).await;

    // No params: the defaults apply.
    let res = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["limit"], 50);
    assert_eq!(json["offset"], 0);

    // Every invalid value is a 400 that names its parameter.
    let bad_queries = [
        "limit=0",
        "limit=201",
        "limit=-1",
        "limit=abc",
        "offset=-1",
        "offset=abc",
    ];
    for query in bad_queries {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/tasks/{}/comments?{query}", task.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{query}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "invalid_query", "{query}");
        let param = if query.starts_with("limit") {
            "limit"
        } else {
            "offset"
        };
        assert!(
            json["error"]["message"].as_str().unwrap().contains(param),
            "{query}: the message must name {param}"
        );
    }

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_offset_past_the_end_is_an_empty_page_with_the_total_intact() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-pastend")).await;
    seed_comment(&pool, task.id, Some(user.id), "one", at(1)).await;
    seed_comment(&pool, task.id, Some(user.id), "two", at(2)).await;

    let res = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments?offset=10", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert!(json["items"].as_array().unwrap().is_empty());
    assert_eq!(json["total"], 2);
    assert_eq!(json["offset"], 10);

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn editing_by_the_author_replaces_the_body_and_bumps_updated_at() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (author, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-edit")).await;
    // Seed with an old timestamp so the bump is observable.
    let seeded = seed_comment(&pool, task.id, Some(author.id), "before", at(1)).await;

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/comments/{}", task.id.0, seeded.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "after" }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["body"], "after");
    // Compare through serde so the string format is the API's own.
    let seeded_updated = serde_json::to_value(seeded.updated_at).expect("a datetime");
    assert_ne!(
        json["updated_at"], seeded_updated,
        "a real edit bumps updated_at"
    );
    assert_eq!(
        json["created_at"],
        serde_json::to_value(seeded.created_at).expect("a datetime"),
        "editing never touches created_at"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, author.id);
}

#[actix_web::test]
async fn editing_with_the_same_body_is_a_noop_leaving_updated_at_alone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (author, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-noop")).await;
    let seeded = seed_comment(&pool, task.id, Some(author.id), "same", at(1)).await;

    // Re-submit the stored body (with padding that trims to it): nothing is
    // written, so updated_at is untouched.
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/comments/{}", task.id.0, seeded.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "  same  " }))
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["body"], "same");
    assert_eq!(
        json["updated_at"],
        serde_json::to_value(seeded.updated_at).expect("a datetime"),
        "a no-op edit leaves updated_at alone"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, author.id);
}

#[actix_web::test]
async fn editing_by_another_staff_or_an_admin_is_forbidden_and_leaves_the_body_alone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (author, _author_token) = seeded_user(&pool, Role::Staff).await;
    let (stranger, stranger_token) = seeded_user(&pool, Role::Staff).await;
    let (admin, admin_token) = seeded_user(&pool, Role::Admin).await;
    let task = create_task(&pool, &unique_title("tc-forbidden")).await;
    let seeded = seed_comment(&pool, task.id, Some(author.id), "untouched", at(1)).await;

    // Neither a Staff nor an Admin who is not the author may edit.
    for (token, who) in [
        (stranger_token.clone(), "staff"),
        (admin_token.clone(), "admin"),
    ] {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/tasks/{}/comments/{}", task.id.0, seeded.id.0),
            Some(&token),
            Some(&serde_json::json!({ "body": "hijacked" }))
        );
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{who}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "forbidden", "{who}");
    }

    // The body is untouched.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&stranger_token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(list).await["items"][0]["body"], "untouched");

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    for user in [author, stranger, admin] {
        delete_user(&pool, user.id);
    }
}

#[actix_web::test]
async fn deleting_by_the_author_or_an_admin_works_and_another_staff_cannot() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (author, author_token) = seeded_user(&pool, Role::Staff).await;
    let (stranger, stranger_token) = seeded_user(&pool, Role::Staff).await;
    let (admin, admin_token) = seeded_user(&pool, Role::Admin).await;
    let task = create_task(&pool, &unique_title("tc-delete")).await;
    let by_author = seed_comment(&pool, task.id, Some(author.id), "mine", at(1)).await;
    let by_admin_target = seed_comment(&pool, task.id, Some(author.id), "admin takes", at(2)).await;
    let guarded = seed_comment(&pool, task.id, Some(author.id), "guarded", at(3)).await;

    // Another Staff cannot delete.
    let denied = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/comments/{}", task.id.0, guarded.id.0),
        Some(&stranger_token),
        None::<&serde_json::Value>
    );
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_of(denied).await["error"]["code"], "forbidden");

    // The author deletes their own...
    let by_author_res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/comments/{}", task.id.0, by_author.id.0),
        Some(&author_token),
        None::<&serde_json::Value>
    );
    assert_eq!(by_author_res.status(), StatusCode::NO_CONTENT);

    // ...and an Admin deletes anyone's (a hard delete: it is gone).
    let admin_res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/comments/{}", task.id.0, by_admin_target.id.0),
        Some(&admin_token),
        None::<&serde_json::Value>
    );
    assert_eq!(admin_res.status(), StatusCode::NO_CONTENT);

    // Only the guarded comment remains.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&author_token),
        None::<&serde_json::Value>
    );
    let json = json_of(list).await;
    assert_eq!(json["total"], 1);
    assert_eq!(item_ids(&json), vec![guarded.id.0]);

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    for user in [author, stranger, admin] {
        delete_user(&pool, user.id);
    }
}

#[actix_web::test]
async fn a_comment_under_the_wrong_task_is_a_404_comment_not_found() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task_a = create_task(&pool, &unique_title("tc-wrong-a")).await;
    let task_b = create_task(&pool, &unique_title("tc-wrong-b")).await;
    let comment = seed_comment(&pool, task_a.id, Some(user.id), "on a", at(1)).await;

    // PUT and DELETE the comment under the other task: 404 comment_not_found,
    // so its existence is not leaked.
    for method in [Method::PUT, Method::DELETE] {
        let body = if method == Method::PUT {
            Some(serde_json::json!({ "body": "moved" }))
        } else {
            None
        };
        let res = request!(
            &app,
            method.clone(),
            format!("/api/tasks/{}/comments/{}", task_b.id.0, comment.id.0),
            Some(&token),
            body.as_ref()
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "comment_not_found", "{method}");
    }

    // The comment is still on task A.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task_a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(json_of(list).await["total"], 1);

    for task in [task_a, task_b] {
        PostgresTaskRepository::new(pool.clone())
            .delete(task.id)
            .await
            .expect("cleanup task");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_task_is_a_404_on_every_route() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let missing_task = Uuid::new_v4();
    let missing_comment = Uuid::new_v4();

    // GET and POST the unknown task...
    for (method, uri, body) in [
        (
            Method::GET,
            format!("/api/tasks/{missing_task}/comments"),
            None::<&serde_json::Value>,
        ),
        (
            Method::POST,
            format!("/api/tasks/{missing_task}/comments"),
            Some(&serde_json::json!({ "body": "hello" })),
        ),
    ] {
        let res = request!(&app, method, uri, Some(&token), body);
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(json_of(res).await["error"]["code"], "not_found", "{method}");
    }

    // ...and PUT and DELETE a comment under it.
    for method in [Method::PUT, Method::DELETE] {
        let body = if method == Method::PUT {
            Some(serde_json::json!({ "body": "hello" }))
        } else {
            None
        };
        let res = request!(
            &app,
            method.clone(),
            format!("/api/tasks/{missing_task}/comments/{missing_comment}"),
            Some(&token),
            body.as_ref()
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "not_found", "{method}");
    }

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn blank_and_overlong_bodies_are_400s() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-blank")).await;
    let existing = seed_comment(&pool, task.id, Some(user.id), "here", at(1)).await;

    // Blank and over-long bodies are rejected on create...
    for body in ["", "   ", &"a".repeat(4001)] {
        let res = request!(
            &app,
            Method::POST,
            format!("/api/tasks/{}/comments", task.id.0),
            Some(&token),
            Some(&serde_json::json!({ "body": body }))
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body:?}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "bad_request", "{body:?}");
    }

    // ...and on update.
    for body in ["   ", &"a".repeat(4001)] {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/tasks/{}/comments/{}", task.id.0, existing.id.0),
            Some(&token),
            Some(&serde_json::json!({ "body": body }))
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body:?}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "bad_request", "{body:?}");
    }

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_body_is_trimmed_and_interior_newlines_are_kept() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-trim")).await;

    let res = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "  line one\nline two  \n" }))
    );
    assert_eq!(res.status(), StatusCode::CREATED);
    let json = json_of(res).await;
    assert_eq!(
        json["body"], "line one\nline two",
        "surrounding whitespace is trimmed, interior newlines kept"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_deactivated_author_still_renders_with_active_false() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (reader, reader_token) = seeded_user(&pool, Role::Staff).await;
    // An author who will be deactivated after commenting.
    let author = create_user(&pool, unique_email("task-comment-deact"), Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-deact")).await;
    seed_comment(
        &pool,
        task.id,
        Some(author.id),
        "before deactivation",
        at(1),
    )
    .await;

    // Deactivate the author directly in the database.
    {
        let mut conn = pool.get().expect("pool connection");
        diesel::update(infrastructure::schema::users::table.find(author.id.0))
            .set(infrastructure::schema::users::deactivated_at.eq(Some(Utc::now())))
            .execute(&mut conn)
            .expect("deactivate author");
    }

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&reader_token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    assert_eq!(json["items"][0]["author"]["id"], author.id.0.to_string());
    assert_eq!(
        json["items"][0]["author"]["active"], false,
        "a deactivated author still renders, flagged inactive"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, author.id);
    delete_user(&pool, reader.id);
}

#[actix_web::test]
async fn deleting_the_task_removes_its_comments() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-cascade")).await;
    seed_comment(&pool, task.id, Some(user.id), "one", at(1)).await;
    seed_comment(&pool, task.id, Some(user.id), "two", at(2)).await;

    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // The task is gone, so its comments are too: the list 404s.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::NOT_FOUND);

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn commenting_leaves_the_task_untouched() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = seeded_user(&pool, Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-untouched")).await;

    // The task before any commenting.
    let before = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    let before = json_of(before).await;

    // Comment, edit the comment, delete it: none of it touches the task row.
    let created = request!(
        &app,
        Method::POST,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "a note" }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let comment_id = json_of(created).await["id"].as_str().unwrap().to_owned();

    let edited = request!(
        &app,
        Method::PUT,
        format!("/api/tasks/{}/comments/{comment_id}", task.id.0),
        Some(&token),
        Some(&serde_json::json!({ "body": "an edited note" }))
    );
    assert_eq!(edited.status(), StatusCode::OK);
    let _ = json_of(edited).await;

    let deleted = request!(
        &app,
        Method::DELETE,
        format!("/api/tasks/{}/comments/{comment_id}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let _ = read_body(deleted).await;

    // The task is byte-for-byte the same on the fields commenting must not
    // touch (D20): its updated_at and its status.
    let after = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}", task.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    let after = json_of(after).await;
    assert_eq!(
        after["updated_at"], before["updated_at"],
        "commenting must not touch the task's updated_at"
    );
    assert_eq!(
        after["status"], before["status"],
        "commenting must not touch the task's status"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_page_of_comments_by_three_authors_issues_one_summary_lookup() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let lookups = Arc::new(AtomicUsize::new(0));
    let app = test_app!(&url, lookups.clone());

    let (reader, reader_token) = seeded_user(&pool, Role::Staff).await;
    let author_a = create_user(&pool, unique_email("tc-author-a"), Role::Staff).await;
    let author_b = create_user(&pool, unique_email("tc-author-b"), Role::Staff).await;
    let author_c = create_user(&pool, unique_email("tc-author-c"), Role::Staff).await;
    let task = create_task(&pool, &unique_title("tc-batch")).await;

    // Three comments by three distinct authors on one page.
    seed_comment(&pool, task.id, Some(author_a.id), "a", at(1)).await;
    seed_comment(&pool, task.id, Some(author_b.id), "b", at(2)).await;
    seed_comment(&pool, task.id, Some(author_c.id), "c", at(3)).await;

    let list = request!(
        &app,
        Method::GET,
        format!("/api/tasks/{}/comments", task.id.0),
        Some(&reader_token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    assert_eq!(json["total"], 3);

    // The whole page's authors resolve with ONE summaries_by_ids call (D17),
    // not one per row.
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        1,
        "a page of comments must batch its author lookups into one query"
    );

    PostgresTaskRepository::new(pool.clone())
        .delete(task.id)
        .await
        .expect("cleanup task");
    for user in [reader, author_a, author_b, author_c] {
        delete_user(&pool, user.id);
    }
}
