//! Route-level authorization tests against the real route table.
//!
//! The app is built through `routes::configure` — the same function `main`
//! uses — with one user per role, and asserts:
//!
//! - every protected route 401s without a session cookie;
//! - each role gets exactly what its permissions allow (403 otherwise);
//! - the public routes stay reachable without a session (an explicit
//!   allowlist, so adding a route forces a conscious choice);
//! - a role change takes effect on the next request with the same cookie.

use actix_web::App;
use actix_web::dev::Service;
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::account_links::AccountLinkService;
use application::auth::SessionService;
use application::auth::password::PasswordAuthProvider;
use application::auth::provider::AuthProviders;
use application::ports::{
    GoalRepository, MilestoneRepository, PasswordHasher, SessionRepository, SessionTokens,
    TaskRepository, UserRepository,
};
use application::user_admin::UserAdminService;
use chrono::Utc;
use diesel::prelude::*;
use domain::{GoalId, MilestoneId, Permission, Role, TaskId, User, UserId};
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
    PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresSessionRepository,
    PostgresTaskRelationRepository, PostgresTaskRepository, PostgresUserRepository,
};
use infrastructure::{Argon2PasswordHasher, NoEmailSender, Sha256SessionTokens};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{COOKIE_NAME, CookieSettings};
use crate::public_routes::PUBLIC_OPERATIONS;
use crate::routes;

/// The `DATABASE_URL` the tests run against, or `None` to skip.
fn database_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    // In CI these tests must run: a green build that skipped them proves nothing.
    if url.is_none() && std::env::var_os("CI").is_some() {
        panic!("DATABASE_URL is not set; refusing to skip access tests in CI");
    }
    url
}

/// A one-connection pool (mirrors the auth handler tests: the default pool
/// size times the parallel test pools would exceed local Postgres's
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

/// Build the app with the real route table and every `web::Data` a handler in
/// it can need, mirroring `main.rs`: a missing one is a 500 that would mask a
/// 401/403. A macro because `init_service`'s service type is opaque.
macro_rules! test_app {
    ($url:expr) => {{
        let pool = test_pool($url);
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
        let hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
        let users_data: web::Data<dyn UserRepository> = users.clone().into();
        // The login route resolves providers by id, mirroring `main.rs`. No
        // redirect provider is registered: the flow routes must 404.
        let providers = web::Data::new(
            AuthProviders::new(
                vec![Arc::new(PasswordAuthProvider::new(
                    users.clone(),
                    hasher.clone(),
                ))],
                Vec::new(),
            )
            .expect("static provider ids are valid and unique"),
        );
        let session_tokens: Arc<dyn SessionTokens> = Arc::new(Sha256SessionTokens);
        let session_service = SessionService::new(
            sessions,
            session_tokens.clone(),
            SessionService::DEFAULT_SESSION_TTL,
        );
        // Mirrors `main.rs`: the invite and password-reset routes go through
        // the account-link service (no email delivery in tests).
        let account_link_service = AccountLinkService::new(
            Arc::new(PostgresAccountTokenRepository::new(pool.clone())),
            users.clone(),
            hasher.clone(),
            session_tokens,
            session_service.clone(),
            Arc::new(NoEmailSender),
            String::new(),
        );
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::new(PostgresGoalMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresTaskRelationRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::new(PostgresProgressSnapshotRepository::new(
                    pool.clone(),
                )))
                .app_data(users_data)
                .app_data(web::Data::new(session_service.clone()))
                .app_data(web::Data::new(account_link_service))
                .app_data(web::Data::new(UserAdminService::new(
                    users,
                    session_service,
                )))
                .app_data(providers)
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
        display_name: "Access test user".into(),
        role,
        deactivated_at: None,
        created_at: now,
        updated_at: now,
    };
    users.create(user).await.expect("create user")
}

/// A password account (known hash) so the login allowlist check can sign in
/// for real.
async fn create_password_user(pool: &PgPool, email: String) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let password_hash = Argon2PasswordHasher
        .hash("password123")
        .await
        .expect("hash password");
    let user = User {
        id: UserId::new(),
        email,
        password_hash: Some(password_hash),
        display_name: "Access test user".into(),
        role: Role::Admin,
        deactivated_at: None,
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

/// Set a user's role directly in the database: until the Users API exists
/// (roadmap 2.4), this is how roles change.
fn set_role(pool: &PgPool, user_id: UserId, role: &str) {
    let mut conn = pool.get().expect("pool connection");
    diesel::update(infrastructure::schema::users::table.find(user_id.0))
        .set(infrastructure::schema::users::role.eq(role))
        .execute(&mut conn)
        .expect("set role");
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

/// The `id` field of a creation response, as a UUID.
fn parse_id(value: &serde_json::Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

/// Delete an account-token row directly (the repository has no delete).
fn delete_token(pool: &PgPool, id: Uuid) {
    let mut conn = pool.get().expect("pool connection");
    diesel::delete(infrastructure::schema::account_tokens::table.find(id))
        .execute(&mut conn)
        .expect("delete token");
}

/// Every protected route: method, URI (placeholder UUIDs), the permission it
/// requires, and a body valid enough that a 400 cannot mask a 403.
fn protected_routes() -> Vec<(Method, String, Permission, Option<serde_json::Value>)> {
    let goal = Uuid::new_v4().to_string();
    let milestone = Uuid::new_v4().to_string();
    let task = Uuid::new_v4().to_string();
    let debug_id = Uuid::new_v4().to_string();
    let goal_body = serde_json::json!({ "title": "Access test" });
    let milestone_body = serde_json::json!({ "title": "Access test" });
    let task_body = serde_json::json!({ "title": "Access test", "status": "backlog" });
    vec![
        (
            Method::GET,
            "/api/goals".into(),
            Permission::ViewContent,
            None,
        ),
        (
            Method::POST,
            "/api/goals".into(),
            Permission::EditContent,
            Some(goal_body.clone()),
        ),
        (
            Method::GET,
            format!("/api/goals/{goal}"),
            Permission::ViewContent,
            None,
        ),
        (
            Method::PUT,
            format!("/api/goals/{goal}"),
            Permission::EditContent,
            Some(goal_body.clone()),
        ),
        (
            Method::DELETE,
            format!("/api/goals/{goal}"),
            Permission::EditContent,
            None,
        ),
        (
            Method::GET,
            "/api/milestones".into(),
            Permission::ViewContent,
            None,
        ),
        (
            Method::POST,
            "/api/milestones".into(),
            Permission::EditContent,
            Some(milestone_body.clone()),
        ),
        (
            Method::GET,
            format!("/api/milestones/{milestone}"),
            Permission::ViewContent,
            None,
        ),
        (
            Method::PUT,
            format!("/api/milestones/{milestone}"),
            Permission::EditContent,
            Some(milestone_body.clone()),
        ),
        (
            Method::DELETE,
            format!("/api/milestones/{milestone}"),
            Permission::EditContent,
            None,
        ),
        // `GET /api/tasks` 400s without a filter, before any auth answer.
        (
            Method::GET,
            "/api/tasks?unassigned=true".into(),
            Permission::ViewContent,
            None,
        ),
        (
            Method::POST,
            "/api/tasks".into(),
            Permission::EditContent,
            Some(task_body.clone()),
        ),
        (
            Method::GET,
            format!("/api/tasks/{task}"),
            Permission::ViewContent,
            None,
        ),
        (
            Method::PUT,
            format!("/api/tasks/{task}"),
            Permission::EditContent,
            Some(task_body),
        ),
        (
            Method::DELETE,
            format!("/api/tasks/{task}"),
            Permission::EditContent,
            None,
        ),
        // The admin-only user management routes.
        (
            Method::GET,
            "/api/users".into(),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::PUT,
            format!("/api/users/{debug_id}/role"),
            Permission::ManageUsers,
            Some(serde_json::json!({ "role": "staff" })),
        ),
        (
            Method::POST,
            format!("/api/users/{debug_id}/deactivate"),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::POST,
            format!("/api/users/{debug_id}/reactivate"),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::POST,
            format!("/api/users/{debug_id}/password-reset"),
            Permission::ManageUsers,
            None,
        ),
        // The admin-only invite routes.
        (
            Method::POST,
            "/api/invites".into(),
            Permission::ManageUsers,
            Some(serde_json::json!({
                "email": "invite-access-test@example.com",
                "role": "staff"
            })),
        ),
        (
            Method::GET,
            "/api/invites".into(),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::POST,
            format!("/api/invites/{debug_id}/revoke"),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::POST,
            format!("/api/invites/{debug_id}/reissue"),
            Permission::ManageUsers,
            None,
        ),
        // The temporary debug routes are locked down to Admin.
        (
            Method::GET,
            format!("/debug/task-relations?task_id={debug_id}"),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::GET,
            format!("/debug/progress-snapshots?goal_id={debug_id}"),
            Permission::ManageUsers,
            None,
        ),
        (
            Method::GET,
            format!("/debug/goal-milestones?goal_id={debug_id}"),
            Permission::ManageUsers,
            None,
        ),
    ]
}

#[actix_web::test]
async fn every_protected_route_requires_a_session() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let app = test_app!(&url);
    for (method, uri, _permission, body) in protected_routes() {
        let res = request!(&app, method, uri, None, body.as_ref());
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
        assert_eq!(json["error"]["code"], "unauthorized", "{method} {uri}");
    }
}

#[actix_web::test]
async fn each_role_gets_exactly_what_its_permissions_allow() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    // Rows created by the allowed POSTs, deleted at the end so the test
    // leaves nothing behind.
    let mut created_goals = Vec::new();
    let mut created_milestones = Vec::new();
    let mut created_tasks = Vec::new();
    let mut created_invites = Vec::new();

    for role in [Role::Admin, Role::Staff, Role::ReadOnly] {
        let user = create_user(&pool, unique_email("access"), role).await;
        let token = issue_token(&pool, user.id).await;
        for (method, uri, permission, body) in protected_routes() {
            let res = request!(&app, method, uri, Some(&token), body.as_ref());
            if role.allows(permission) {
                assert!(
                    res.status() != StatusCode::UNAUTHORIZED
                        && res.status() != StatusCode::FORBIDDEN,
                    "{role:?} {method} {uri} -> {} (allowed by {permission:?})",
                    res.status()
                );
                if method == Method::POST && res.status() == StatusCode::CREATED {
                    let json: serde_json::Value =
                        serde_json::from_slice(&read_body(res).await).unwrap();
                    // Invites nest their id under `invite`; the other
                    // creations carry a top-level one.
                    match uri.as_str() {
                        "/api/goals" => created_goals.push(parse_id(&json["id"])),
                        "/api/milestones" => created_milestones.push(parse_id(&json["id"])),
                        "/api/invites" => created_invites.push(parse_id(&json["invite"]["id"])),
                        _ => created_tasks.push(parse_id(&json["id"])),
                    }
                } else {
                    // Consume the body so the response is dropped cleanly.
                    let _ = read_body(res).await;
                }
            } else {
                assert_eq!(
                    res.status(),
                    StatusCode::FORBIDDEN,
                    "{role:?} {method} {uri}"
                );
                let json: serde_json::Value =
                    serde_json::from_slice(&read_body(res).await).unwrap();
                assert_eq!(
                    json["error"]["code"], "forbidden",
                    "{role:?} {method} {uri}"
                );
            }
        }
        delete_user(&pool, user.id);
    }

    let goals = PostgresGoalRepository::new(pool.clone());
    for id in created_goals {
        goals.delete(GoalId(id)).await.expect("cleanup goal");
    }
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    for id in created_milestones {
        milestones
            .delete(MilestoneId(id))
            .await
            .expect("cleanup milestone");
    }
    let tasks = PostgresTaskRepository::new(pool.clone());
    for id in created_tasks {
        tasks.delete(TaskId(id)).await.expect("cleanup task");
    }
    for id in created_invites {
        delete_token(&pool, id);
    }
}

/// Routes that must stay reachable without a session. Adding a route to the
/// server means either adding it here (if public) or giving it an access
/// extractor — the matrix test above fails otherwise.
#[actix_web::test]
async fn public_routes_do_not_require_a_session() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    // Login needs real credentials: a 401 for bad credentials is correct
    // behaviour, so reachability is proven by signing in successfully.
    let login_user = create_password_user(&pool, unique_email("public-login")).await;

    // The public API operations come from the shared allowlist (the OpenAPI
    // document test checks the same list); `{provider}` is exercised with
    // `oidc`. No redirect provider is registered in the test app: the flow
    // routes must answer 404, not 401.
    let mut cases: Vec<(Method, String, Option<serde_json::Value>, StatusCode)> = PUBLIC_OPERATIONS
        .iter()
        .map(|(method, path)| {
            let (body, expected) = match *path {
                "/api/auth/login" => (
                    Some(serde_json::json!({
                        "email": login_user.email.clone(),
                        "password": "password123",
                    })),
                    StatusCode::OK,
                ),
                "/api/auth/logout" => (None, StatusCode::NO_CONTENT),
                "/api/auth/providers" => (None, StatusCode::OK),
                // A bogus token must reach the handler and answer 400: that
                // proves the route needs no session.
                "/api/auth/tokens/inspect" => (
                    Some(serde_json::json!({ "token": "bogus" })),
                    StatusCode::BAD_REQUEST,
                ),
                "/api/auth/accept-invite" => (
                    Some(serde_json::json!({
                        "token": "bogus",
                        "password": "long-enough-pass"
                    })),
                    StatusCode::BAD_REQUEST,
                ),
                "/api/auth/reset-password" => (
                    Some(serde_json::json!({
                        "token": "bogus",
                        "password": "long-enough-pass"
                    })),
                    StatusCode::BAD_REQUEST,
                ),
                "/api/auth/{provider}/login" | "/api/auth/{provider}/callback" => {
                    (None, StatusCode::NOT_FOUND)
                }
                // The allowlist only holds the routes above.
                other => panic!("public route {other} has no request details"),
            };
            (
                method.clone(),
                path.replace("{provider}", "oidc"),
                body,
                expected,
            )
        })
        .collect();

    // Public routes that are not OpenAPI operations: no allowlist entry.
    cases.push((Method::GET, "/health".into(), None, StatusCode::OK));
    cases.push((
        Method::GET,
        "/api-docs/openapi.json".into(),
        None,
        StatusCode::OK,
    ));
    cases.push((
        Method::GET,
        "/api-docs/swagger-ui/".into(),
        None,
        StatusCode::OK,
    ));

    // Open signup is gone (roadmap 2.5): no route matches the path at all, so
    // the router answers 404 — a 405 would mean some method still serves it.
    cases.push((
        Method::POST,
        "/api/auth/signup".into(),
        None,
        StatusCode::NOT_FOUND,
    ));

    for (method, uri, body, expected) in cases {
        let res = request!(&app, method, uri, None, body.as_ref());
        assert_eq!(res.status(), expected, "{method} {uri}");
    }

    // The login check created a user; clean it up.
    delete_user(&pool, login_user.id);
}

#[actix_web::test]
async fn a_role_change_takes_effect_on_the_next_request() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let user = create_user(&pool, unique_email("role-change"), Role::ReadOnly).await;
    let token = issue_token(&pool, user.id).await;
    let body = Some(serde_json::json!({ "title": "Role change" }));

    // Read-only can read but not write.
    let get = request!(
        &app,
        Method::GET,
        "/api/goals",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(get.status(), StatusCode::OK);
    let denied = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        body.as_ref()
    );
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // Promote directly in the database: the same cookie now writes.
    set_role(&pool, user.id, "admin");
    let allowed = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        body.as_ref()
    );
    assert_eq!(allowed.status(), StatusCode::CREATED);
    let json: serde_json::Value = serde_json::from_slice(&read_body(allowed).await).unwrap();
    let goal_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

    // Demote again: the same cookie is read-only once more.
    set_role(&pool, user.id, "read_only");
    let denied_again = request!(
        &app,
        Method::POST,
        "/api/goals",
        Some(&token),
        body.as_ref()
    );
    assert_eq!(denied_again.status(), StatusCode::FORBIDDEN);

    PostgresGoalRepository::new(pool.clone())
        .delete(GoalId(goal_id))
        .await
        .expect("cleanup goal");
    delete_user(&pool, user.id);
}
