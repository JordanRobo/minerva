//! Users HTTP API: the admin-only `/api/users` handlers and their JSON DTOs.
//!
//! The business rules (no self-modification, at least one active admin) live
//! in [`UserAdminService`]; these handlers only translate requests and
//! errors to and from HTTP.

use actix_web::{HttpResponse, web};
use application::account_links::AccountLinkService;
use application::rate_limit::{ADMIN_ISSUE_ACTOR, RateLimitDecision, RateLimitService};
use application::user_admin::{UserAdminError, UserAdminService};
use chrono::{DateTime, Utc};
use domain::{Role, User, UserId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::AdminAccess;
use crate::error::{ApiError, account_link_error_response, repo_error_response};
use crate::openapi::RoleDoc;

/// JSON shape of a user in the admin list and mutation responses. The
/// password hash is never part of the wire format.
#[derive(Serialize, ToSchema)]
pub struct UserAdminResponse {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    #[schema(value_type = RoleDoc)]
    pub role: Role,
    /// Whether the account may sign in (that is, has not been deactivated).
    pub active: bool,
    /// When an admin deactivated this account; `null` while it is active.
    pub deactivated_at: Option<DateTime<Utc>>,
    /// Whether the role is currently locked against hand edits by SSO group
    /// rules (the user's `role_managed_by_sso` flag while any rule exists).
    pub role_managed_by_sso: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<(&User, bool)> for UserAdminResponse {
    /// The second value is the effective D15 lock (flag AND any rule
    /// exists), computed once per request by the handler.
    fn from((user, role_managed_by_sso): (&User, bool)) -> Self {
        Self {
            id: user.id.0,
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            role: user.role,
            active: user.is_active(),
            deactivated_at: user.deactivated_at,
            role_managed_by_sso,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

/// Body for `PUT /api/users/{id}/role`. A string that is not a role fails
/// JSON deserialization, so the standard 400 handler answers.
#[derive(Deserialize, ToSchema)]
pub struct ChangeRoleRequest {
    #[schema(value_type = RoleDoc)]
    pub role: Role,
}

/// Translate a [`UserAdminError`] into the [`ApiError`] it renders as:
/// `NotFound` -> 404, repository failures like any other, and both policy
/// rejections (`CannotModifySelf`, `LastAdmin`) as 409 with the service's
/// plain-language message.
fn admin_error_response(error: UserAdminError) -> ApiError {
    match error {
        UserAdminError::NotFound => ApiError::not_found(),
        UserAdminError::RoleManagedBySso => ApiError::role_managed_by_sso(error.to_string()),
        UserAdminError::Repository(err) => repo_error_response(err),
        other => ApiError::conflict(other.to_string()),
    }
}

/// List Users
///
/// List every user account, in creation order. Requires the Admin role.
#[utoipa::path(
    get,
    path = "/api/users",
    tags = ["users"],
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "All users", body = Vec<UserAdminResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError)
    )
)]
pub async fn list_users(
    admin: web::Data<UserAdminService>,
    _access: AdminAccess,
) -> Result<HttpResponse, ApiError> {
    let users = admin.list().await.map_err(admin_error_response)?;
    // One query for the D15 condition shared by every row.
    let rules_exist = admin
        .sso_rules_exist()
        .await
        .map_err(admin_error_response)?;
    Ok(HttpResponse::Ok().json(
        users
            .iter()
            .map(|user| UserAdminResponse::from((user, user.role_managed_by_sso && rules_exist)))
            .collect::<Vec<_>>(),
    ))
}

/// Change a User's Role
///
/// Change another user's role. An admin cannot change their own role, the last active admin can never be demoted, and a role set by SSO groups is locked while any group rule exists. Requires the Admin role.
#[utoipa::path(
    put,
    path = "/api/users/{id}/role",
    tags = ["users"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "User identifier")),
    request_body = ChangeRoleRequest,
    responses(
        (status = 200, description = "The updated user", body = UserAdminResponse),
        (status = 400, description = "Unknown role string", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No user with this id", body = ApiError),
        (status = 409, description = "Changing your own role, demoting the last active admin, or changing a role managed by SSO groups, is not allowed", body = ApiError)
    )
)]
pub async fn change_user_role(
    admin: web::Data<UserAdminService>,
    access: AdminAccess,
    path: web::Path<Uuid>,
    body: web::Json<ChangeRoleRequest>,
) -> Result<HttpResponse, ApiError> {
    match admin
        .change_role(&access.user, UserId(*path), body.role)
        .await
    {
        Ok(user) => {
            let managed = user.role_managed_by_sso
                && admin
                    .sso_rules_exist()
                    .await
                    .map_err(admin_error_response)?;
            Ok(HttpResponse::Ok().json(UserAdminResponse::from((&user, managed))))
        }
        Err(error) => Err(admin_error_response(error)),
    }
}

/// Deactivate a User
///
/// Deactivate another user's account: it can no longer sign in and every session of it stops working immediately. An admin cannot deactivate themselves, and the last active admin can never be deactivated. Requires the Admin role.
#[utoipa::path(
    post,
    path = "/api/users/{id}/deactivate",
    tags = ["users"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "User identifier")),
    responses(
        (status = 200, description = "The updated user", body = UserAdminResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No user with this id", body = ApiError),
        (status = 409, description = "Deactivating yourself, or deactivating the last active admin, is not allowed", body = ApiError)
    )
)]
pub async fn deactivate_user(
    admin: web::Data<UserAdminService>,
    access: AdminAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match admin.deactivate(&access.user, UserId(*path)).await {
        Ok(user) => {
            let managed = user.role_managed_by_sso
                && admin
                    .sso_rules_exist()
                    .await
                    .map_err(admin_error_response)?;
            Ok(HttpResponse::Ok().json(UserAdminResponse::from((&user, managed))))
        }
        Err(error) => Err(admin_error_response(error)),
    }
}

/// Reactivate a User
///
/// Reactivate a previously deactivated account so it can sign in again. Requires the Admin role.
#[utoipa::path(
    post,
    path = "/api/users/{id}/reactivate",
    tags = ["users"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "User identifier")),
    responses(
        (status = 200, description = "The updated user", body = UserAdminResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No user with this id", body = ApiError)
    )
)]
pub async fn reactivate_user(
    admin: web::Data<UserAdminService>,
    _access: AdminAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match admin.reactivate(UserId(*path)).await {
        Ok(user) => {
            let managed = user.role_managed_by_sso
                && admin
                    .sso_rules_exist()
                    .await
                    .map_err(admin_error_response)?;
            Ok(HttpResponse::Ok().json(UserAdminResponse::from((&user, managed))))
        }
        Err(error) => Err(admin_error_response(error)),
    }
}

/// A freshly issued password-reset link. The raw token appears in `link`
/// exactly once and is not recoverable afterwards; issuing again replaces it.
#[derive(Serialize, ToSchema)]
pub struct PasswordResetCreatedResponse {
    /// The one-time reset link; shown only in this response.
    pub link: String,
    pub expires_at: DateTime<Utc>,
    /// Whether the reset email went out; when false, share `link` by hand.
    pub emailed: bool,
}

/// Create a Password Reset Link
///
/// Issue a one-time password-reset link for a user's account; any live link the user already has stops working. The link is returned only in this response (and emailed when delivery is configured). Refused for accounts that sign in through an external provider (no password to reset) and for deactivated accounts. Requires the Admin role. Rate limited per acting admin.
#[utoipa::path(
    post,
    path = "/api/users/{id}/password-reset",
    tags = ["users"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "User identifier")),
    responses(
        (status = 201, description = "The one-time reset link", body = PasswordResetCreatedResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No user with this id", body = ApiError),
        (status = 409, description = "The account has no password to reset (external provider only) or is deactivated", body = ApiError),
        (status = 429, description = "Too many attempts from this admin; wait the time in the Retry-After header before trying again", body = ApiError)
    )
)]
pub async fn create_password_reset(
    links: web::Data<AccountLinkService>,
    access: AdminAccess,
    rate_limits: web::Data<RateLimitService>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    if let RateLimitDecision::Limited { retry_after } = rate_limits
        .hit(
            &ADMIN_ISSUE_ACTOR,
            &access.user.id.0.to_string(),
            Utc::now(),
        )
        .await
    {
        return Err(ApiError::rate_limited(retry_after));
    }
    match links
        .issue_password_reset(&access.user, UserId(*path))
        .await
    {
        Ok(issued) => Ok(HttpResponse::Created().json(PasswordResetCreatedResponse {
            link: issued.link,
            expires_at: issued.item.expires_at,
            emailed: issued.emailed,
        })),
        Err(error) => Err(account_link_error_response(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::Service;
    use actix_web::http::{Method, StatusCode, header};
    use actix_web::test::{TestRequest, init_service, read_body};
    use application::account_links::AccountLinkService;
    use application::auth::SessionService;
    use application::auth::password::PasswordAuthProvider;
    use application::auth::provider::AuthProviders;
    use application::goal_list::GoalListService;
    use application::milestone_list::MilestoneListService;
    use application::ports::{
        GoalRepository, PasswordHasher, SessionRepository, SessionTokens, SsoGroupRuleRepository,
        UserRepository,
    };
    use chrono::Utc;
    use diesel::prelude::*;
    use domain::{GoalId, SsoGroupRule, SsoGroupRuleId};
    use infrastructure::db::PgPool;
    use infrastructure::repositories::{
        PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
        PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresSessionRepository,
        PostgresSsoGroupRuleRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
        PostgresUserRepository,
    };
    use infrastructure::{Argon2PasswordHasher, NoEmailSender, Sha256SessionTokens};
    use std::sync::Arc;

    use crate::auth::{COOKIE_NAME, CookieSettings};
    use crate::config::RateLimitConfig;
    use crate::routes;

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip user API tests in CI");
        }
        url
    }

    /// A one-connection pool (mirrors the access tests: the default pool size
    /// times the parallel test pools would exceed local Postgres's
    /// `max_connections`). Pending migrations are applied once per process.
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

    /// Build the app with the real route table and every `web::Data` a handler
    /// in it can need, mirroring `main.rs`. A macro because `init_service`'s
    /// service type is opaque.
    macro_rules! test_app {
        ($url:expr) => {{
            let pool = test_pool($url);
            let sessions: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool.clone()));
            let users: Arc<dyn UserRepository> =
                Arc::new(PostgresUserRepository::new(pool.clone()));
            let hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
            let users_data: web::Data<dyn UserRepository> = users.clone().into();
            // The login route resolves providers by id, mirroring `main.rs`.
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
            // Mirrors `main.rs`: the invite and password-reset routes go
            // through the account-link service (no email delivery in tests,
            // so links come back site-relative).
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
                    .app_data(web::Data::new(GoalListService::new(Arc::new(
                        PostgresGoalRepository::new(pool.clone()),
                    ))))
                    .app_data(web::Data::new(MilestoneListService::new(Arc::new(
                        PostgresMilestoneRepository::new(pool.clone()),
                    ))))
                    .app_data(web::Data::new(UserAdminService::new(
                        users,
                        Arc::new(PostgresSsoGroupRuleRepository::new(pool.clone())),
                        session_service,
                    )))
                    .app_data(providers)
                    .app_data(web::Data::new(CookieSettings { secure: false }))
                    // Rate limiting is disabled in these tests (they assert
                    // user-admin behaviour, not throttling — the rate-limit
                    // tests run it enabled with a small limit), but the
                    // handlers extract the service, so it must be registered.
                    .app_data(web::Data::new(RateLimitService::new(
                        crate::rate_limit_tests::TestRateLimiter::new(&[]),
                        false,
                    )))
                    .app_data(web::Data::new(RateLimitConfig {
                        enabled: false,
                        client_ip_header: String::new(),
                    }))
                    .configure(routes::configure),
            )
            .await
        }};
    }

    /// One request against the test app: `token` is the session cookie value
    /// (or none), `body` a JSON body for POST/PUT.
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

    /// A user created directly in the database (no sign-in needed: sessions
    /// are issued straight from the [`SessionService`]).
    async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            email,
            password_hash: None,
            display_name: "Users API test user".into(),
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        };
        users.create(user).await.expect("create user")
    }

    /// A password account (known hash) so the login route can be exercised.
    async fn create_password_user(pool: &PgPool, email: String, role: Role) -> User {
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
            display_name: "Users API test user".into(),
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        };
        users.create(user).await.expect("create user")
    }

    /// An admin row that is already deactivated: the role column still says
    /// admin, but the account cannot sign in and must not count as an active
    /// admin for the last-admin guard.
    async fn create_deactivated_admin(pool: &PgPool, email: String) -> User {
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            email,
            password_hash: None,
            display_name: "Users API test user".into(),
            role: Role::Admin,
            deactivated_at: Some(now),
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

    /// Set a user's role directly in the database (the race test resets its
    /// round this way).
    fn set_role(pool: &PgPool, user_id: UserId, role: &str) {
        let mut conn = pool.get().expect("pool connection");
        diesel::update(infrastructure::schema::users::table.find(user_id.0))
            .set(infrastructure::schema::users::role.eq(role))
            .execute(&mut conn)
            .expect("set role");
    }

    /// Flip the D15 flag directly in the database: sign-in recomputation,
    /// which sets it for real, lands in a later step.
    fn set_sso_managed(pool: &PgPool, user_id: UserId) {
        let mut conn = pool.get().expect("pool connection");
        diesel::update(infrastructure::schema::users::table.find(user_id.0))
            .set(infrastructure::schema::users::role_managed_by_sso.eq(true))
            .execute(&mut conn)
            .expect("set sso flag");
    }

    /// Insert one group rule (the D15 lock condition) under a unique name and
    /// return its id for cleanup.
    async fn create_rule(pool: &PgPool) -> SsoGroupRuleId {
        let rules = PostgresSsoGroupRuleRepository::new(pool.clone());
        let now = Utc::now();
        let rule = SsoGroupRule {
            id: SsoGroupRuleId::new(),
            group_name: format!("users-api-{}", Uuid::new_v4()),
            role: Role::Admin,
            created_at: now,
            updated_at: now,
        };
        rules.create(rule).await.expect("create rule").id
    }

    /// Delete a group rule row directly (the repository has no bulk delete).
    fn delete_rule(pool: &PgPool, id: SsoGroupRuleId) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::sso_group_role_rules::table.find(id.0))
            .execute(&mut conn)
            .expect("delete rule");
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

    /// A `PUT /api/users/{id}/role` request demoting to read-only, carrying the
    /// given session cookie. Built (not sent) so the race test can fire two of
    /// them concurrently: `tokio::join!` needs the un-awaited call futures.
    fn demote_request(uri: String, token: &str) -> TestRequest {
        let mut req = TestRequest::with_uri(&uri).method(Method::PUT);
        req = req.set_json(serde_json::json!({ "role": "read_only" }));
        req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={token}")))
    }

    #[actix_web::test]
    async fn an_admin_can_list_users() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-list"), Role::Admin).await;
        let staff = create_user(&pool, unique_email("users-list"), Role::Staff).await;
        let viewer = create_user(&pool, unique_email("users-list"), Role::ReadOnly).await;
        let token = issue_token(&pool, admin.id).await;

        let res = request!(
            &app,
            Method::GET,
            "/api/users",
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
        let list = json.as_array().expect("a JSON array");

        for (user, role) in [(&admin, "admin"), (&staff, "staff"), (&viewer, "read_only")] {
            let entry = list
                .iter()
                .find(|entry| entry["id"] == user.id.0.to_string())
                .unwrap_or_else(|| panic!("{} missing from the list", user.email));
            assert_eq!(entry["role"], role);
            assert_eq!(entry["active"], true);
            assert!(
                entry.get("password_hash").is_none(),
                "the hash must never be sent"
            );
        }

        // Stable order: users created one after another appear in creation order.
        let position = |id: UserId| {
            list.iter()
                .position(|entry| entry["id"] == id.0.to_string())
                .unwrap()
        };
        assert!(position(admin.id) < position(staff.id));
        assert!(position(staff.id) < position(viewer.id));

        for user in [admin, staff, viewer] {
            delete_user(&pool, user.id);
        }
    }

    #[actix_web::test]
    async fn a_role_change_applies_on_the_users_next_request() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-role"), Role::Admin).await;
        let staff = create_user(&pool, unique_email("users-role"), Role::Staff).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let staff_token = issue_token(&pool, staff.id).await;

        // Staff can edit...
        let goal_body = Some(serde_json::json!({ "title": "Users API test" }));
        let created = request!(
            &app,
            Method::POST,
            "/api/goals",
            Some(&staff_token),
            goal_body.as_ref()
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let goal_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

        // ...until an admin demotes them; the same cookie is read-only next.
        let demoted = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", staff.id.0),
            Some(&admin_token),
            Some(&serde_json::json!({ "role": "read_only" }))
        );
        assert_eq!(demoted.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(demoted).await).unwrap();
        assert_eq!(json["role"], "read_only");

        let denied = request!(
            &app,
            Method::POST,
            "/api/goals",
            Some(&staff_token),
            goal_body.as_ref()
        );
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        PostgresGoalRepository::new(pool.clone())
            .delete(GoalId(goal_id))
            .await
            .expect("cleanup goal");
        delete_user(&pool, admin.id);
        delete_user(&pool, staff.id);
    }

    #[actix_web::test]
    async fn deactivating_kills_the_session_and_login_reactivating_restores_both() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-deactivate"), Role::Admin).await;
        let staff =
            create_password_user(&pool, unique_email("users-deactivate"), Role::Staff).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let staff_token = issue_token(&pool, staff.id).await;

        // The account signs in and its session works.
        let login_body = serde_json::json!({ "email": staff.email, "password": "password123" });
        let logged_in = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&login_body)
        );
        assert_eq!(logged_in.status(), StatusCode::OK);
        let working = request!(
            &app,
            Method::GET,
            "/api/goals",
            Some(&staff_token),
            None::<&serde_json::Value>
        );
        assert_eq!(working.status(), StatusCode::OK);

        // Deactivation: the already-issued session stops working immediately
        // and new logins fail.
        let deactivated = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/deactivate", staff.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(deactivated.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&read_body(deactivated).await).unwrap();
        assert_eq!(json["active"], false);
        assert!(json["deactivated_at"].is_string());

        let dead_session = request!(
            &app,
            Method::GET,
            "/api/goals",
            Some(&staff_token),
            None::<&serde_json::Value>
        );
        assert_eq!(dead_session.status(), StatusCode::UNAUTHORIZED);
        let login_refused = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&login_body)
        );
        assert_eq!(login_refused.status(), StatusCode::UNAUTHORIZED);

        // Reactivation: login works again.
        let reactivated = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/reactivate", staff.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(reactivated.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&read_body(reactivated).await).unwrap();
        assert_eq!(json["active"], true);
        let login_again = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&login_body)
        );
        assert_eq!(login_again.status(), StatusCode::OK);

        delete_user(&pool, admin.id);
        delete_user(&pool, staff.id);
    }

    #[actix_web::test]
    async fn an_admin_cannot_modify_themselves() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-self"), Role::Admin).await;
        let token = issue_token(&pool, admin.id).await;

        let self_role = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", admin.id.0),
            Some(&token),
            Some(&serde_json::json!({ "role": "staff" }))
        );
        assert_eq!(self_role.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(self_role).await).unwrap();
        assert_eq!(json["error"]["code"], "conflict");

        let self_deactivate = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/deactivate", admin.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(self_deactivate.status(), StatusCode::CONFLICT);

        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn demoting_or_deactivating_one_of_two_admins_succeeds() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin_a = create_user(&pool, unique_email("users-last-admin"), Role::Admin).await;
        let admin_b = create_user(&pool, unique_email("users-last-admin"), Role::Admin).await;
        let token_a = issue_token(&pool, admin_a.id).await;
        let token_b = issue_token(&pool, admin_b.id).await;

        // Demoting one of two admins goes through: the other is still active.
        let demoted = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", admin_b.id.0),
            Some(&token_a),
            Some(&serde_json::json!({ "role": "staff" }))
        );
        assert_eq!(demoted.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(demoted).await).unwrap();
        assert_eq!(json["role"], "staff");

        // The change applies on B's very next request: they can no longer
        // manage users.
        let denied = request!(
            &app,
            Method::GET,
            "/api/users",
            Some(&token_b),
            None::<&serde_json::Value>
        );
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        // Deactivating the (now staff) account goes through too...
        let deactivated = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/deactivate", admin_b.id.0),
            Some(&token_a),
            None::<&serde_json::Value>
        );
        assert_eq!(deactivated.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&read_body(deactivated).await).unwrap();
        assert_eq!(json["active"], false);

        // ...and A is still there and active: neither operation left the
        // system without an administrator. The refusal side of this rule (the
        // last-admin guard) only fires under concurrency over HTTP — see
        // concurrent_demotions_never_leave_zero_active_admins.
        let users = PostgresUserRepository::new(pool.clone());
        let still_admin = users.find_by_id(admin_a.id).await.unwrap().unwrap();
        assert_eq!(still_admin.role, Role::Admin);
        assert!(still_admin.is_active());

        delete_user(&pool, admin_a.id);
        delete_user(&pool, admin_b.id);
    }

    #[actix_web::test]
    async fn a_deactivated_admin_does_not_count_as_active() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        // Two separate apps, one pool each, same database (as in the race test).
        let app_a = test_app!(&url);
        let app_b = test_app!(&url);

        let admin_a = create_user(&pool, unique_email("users-dead-admin"), Role::Admin).await;
        let admin_b = create_user(&pool, unique_email("users-dead-admin"), Role::Admin).await;
        // A third account whose role column says admin but which is deactivated.
        // If the guard counted it as an active admin, both demotions below could
        // succeed and leave the system without one.
        let dead_admin = create_deactivated_admin(&pool, unique_email("users-dead-admin")).await;
        let token_a = issue_token(&pool, admin_a.id).await;
        let token_b = issue_token(&pool, admin_b.id).await;

        for _ in 0..3 {
            set_role(&pool, admin_a.id, "admin");
            set_role(&pool, admin_b.id, "admin");

            let (res_a, res_b) = tokio::join!(
                app_a.call(
                    demote_request(format!("/api/users/{}/role", admin_b.id.0), &token_a)
                        .to_request()
                ),
                app_b.call(
                    demote_request(format!("/api/users/{}/role", admin_a.id.0), &token_b)
                        .to_request()
                )
            );
            let res_a = res_a.unwrap();
            let res_b = res_b.unwrap();

            for res in [&res_a, &res_b] {
                assert!(
                    matches!(res.status(), StatusCode::OK | StatusCode::CONFLICT),
                    "unexpected status {}",
                    res.status()
                );
            }
            assert!(
                res_a.status() == StatusCode::OK || res_b.status() == StatusCode::OK,
                "one demotion must succeed"
            );

            // The invariant the guard exists for: an active admin remains.
            // (Other tests share this database, so count all users, not just
            // the two racers; when no outsider is present, a guard that
            // counted the deactivated admin would let both demotions through
            // and leave zero.)
            let users = PostgresUserRepository::new(pool.clone());
            let active_admins = users
                .list()
                .await
                .expect("list users")
                .iter()
                .filter(|user| user.role == Role::Admin && user.is_active())
                .count();
            assert!(active_admins >= 1, "the round left no active admin");
        }

        delete_user(&pool, admin_a.id);
        delete_user(&pool, admin_b.id);
        delete_user(&pool, dead_admin.id);
    }

    #[actix_web::test]
    async fn concurrent_demotions_never_leave_zero_active_admins() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        // Two separate apps (the test service type is opaque, and sharing one
        // across concurrent calls is not guaranteed), each with its own
        // one-connection pool, against the same database.
        let app_a = test_app!(&url);
        let app_b = test_app!(&url);

        let admin_a = create_user(&pool, unique_email("users-race"), Role::Admin).await;
        let admin_b = create_user(&pool, unique_email("users-race"), Role::Admin).await;
        let token_a = issue_token(&pool, admin_a.id).await;
        let token_b = issue_token(&pool, admin_b.id).await;

        for _ in 0..5 {
            // A fresh round: both admins again.
            set_role(&pool, admin_a.id, "admin");
            set_role(&pool, admin_b.id, "admin");

            // Each admin tries to demote the other at the same instant.
            let (res_a, res_b) = tokio::join!(
                app_a.call(
                    demote_request(format!("/api/users/{}/role", admin_b.id.0), &token_a)
                        .to_request()
                ),
                app_b.call(
                    demote_request(format!("/api/users/{}/role", admin_a.id.0), &token_b)
                        .to_request()
                )
            );
            let res_a = res_a.unwrap();
            let res_b = res_b.unwrap();

            // One demotion wins; the loser hits the last-admin guard. A late
            // loser can also get 403: if the winner commits before the
            // loser's session check runs, that check sees the fresh role and
            // refuses — which is correct for a just-demoted user.
            for res in [&res_a, &res_b] {
                assert!(
                    matches!(
                        res.status(),
                        StatusCode::OK | StatusCode::CONFLICT | StatusCode::FORBIDDEN
                    ),
                    "unexpected status {}",
                    res.status()
                );
            }
            assert!(
                res_a.status() == StatusCode::OK || res_b.status() == StatusCode::OK,
                "one demotion must succeed"
            );
            // When a loser exists (no other test's admin was around), its 409
            // must be the last-admin refusal, not some other conflict.
            let loser_is_a = res_a.status() == StatusCode::CONFLICT;
            let loser_is_b = res_b.status() == StatusCode::CONFLICT;
            if loser_is_a || loser_is_b {
                let json: serde_json::Value = if loser_is_a {
                    serde_json::from_slice(&read_body(res_a).await).unwrap()
                } else {
                    serde_json::from_slice(&read_body(res_b).await).unwrap()
                };
                assert_eq!(json["error"]["code"], "conflict");
                assert!(
                    json["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("at least one active administrator")
                );
            }

            // The invariant the advisory lock exists for: the system still has
            // an active admin. (Other tests share this database, so count all
            // users, not just the two racers.)
            let users = PostgresUserRepository::new(pool.clone());
            let active_admins = users
                .list()
                .await
                .expect("list users")
                .iter()
                .filter(|user| user.role == Role::Admin && user.is_active())
                .count();
            assert!(active_admins >= 1, "the round left no active admin");
        }

        delete_user(&pool, admin_a.id);
        delete_user(&pool, admin_b.id);
    }

    #[actix_web::test]
    async fn an_unknown_user_id_is_not_found() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-404"), Role::Admin).await;
        let token = issue_token(&pool, admin.id).await;
        let missing = Uuid::new_v4().to_string();

        let role = request!(
            &app,
            Method::PUT,
            format!("/api/users/{missing}/role"),
            Some(&token),
            Some(&serde_json::json!({ "role": "staff" }))
        );
        assert_eq!(role.status(), StatusCode::NOT_FOUND);
        let deactivate = request!(
            &app,
            Method::POST,
            format!("/api/users/{missing}/deactivate"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(deactivate.status(), StatusCode::NOT_FOUND);
        let reactivate = request!(
            &app,
            Method::POST,
            format!("/api/users/{missing}/reactivate"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(reactivate.status(), StatusCode::NOT_FOUND);

        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn an_unknown_role_string_is_a_bad_request() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-400"), Role::Admin).await;
        let staff = create_user(&pool, unique_email("users-400"), Role::Staff).await;
        let token = issue_token(&pool, admin.id).await;

        let res = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", staff.id.0),
            Some(&token),
            Some(&serde_json::json!({ "role": "superadmin" }))
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
        assert_eq!(json["error"]["code"], "bad_request");

        delete_user(&pool, admin.id);
        delete_user(&pool, staff.id);
    }

    #[actix_web::test]
    async fn an_sso_managed_role_is_locked_while_a_rule_exists() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-sso-lock"), Role::Admin).await;
        let managed = create_user(&pool, unique_email("users-sso-lock"), Role::Staff).await;
        set_sso_managed(&pool, managed.id);
        let rule_id = create_rule(&pool).await;
        let token = issue_token(&pool, admin.id).await;

        // While any rule exists, the hand edit is refused with the D15 code.
        let locked = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", managed.id.0),
            Some(&token),
            Some(&serde_json::json!({ "role": "admin" }))
        );
        assert_eq!(locked.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(locked).await).unwrap();
        assert_eq!(json["error"]["code"], "role_managed_by_sso");

        // Deleting our rule unlocks the role for hand edits. Other tests
        // share this database and may hold their own rules briefly, so retry
        // through any transient lock from a foreign rule.
        delete_rule(&pool, rule_id);
        let mut unlocked = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", managed.id.0),
            Some(&token),
            Some(&serde_json::json!({ "role": "admin" }))
        );
        for _ in 0..100 {
            if unlocked.status() == StatusCode::OK {
                break;
            }
            let body: serde_json::Value =
                serde_json::from_slice(&read_body(unlocked).await).unwrap();
            assert_eq!(body["error"]["code"], "role_managed_by_sso");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            unlocked = request!(
                &app,
                Method::PUT,
                format!("/api/users/{}/role", managed.id.0),
                Some(&token),
                Some(&serde_json::json!({ "role": "admin" }))
            );
        }
        assert_eq!(unlocked.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(unlocked).await).unwrap();
        assert_eq!(json["role"], "admin");

        delete_user(&pool, admin.id);
        delete_user(&pool, managed.id);
    }

    #[actix_web::test]
    async fn the_response_reports_the_effective_sso_lock() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-sso-field"), Role::Admin).await;
        let managed = create_user(&pool, unique_email("users-sso-field"), Role::Staff).await;
        let plain = create_user(&pool, unique_email("users-sso-field"), Role::Staff).await;
        set_sso_managed(&pool, managed.id);
        let token = issue_token(&pool, admin.id).await;

        // The field is the effective value: flag AND any rule exists. Users
        // SSO never recomputed report false with or without rules (other
        // tests share this database, so a foreign rule may be present).
        let list = request!(
            &app,
            Method::GET,
            "/api/users",
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(list).await).unwrap();
        for id in [admin.id, plain.id] {
            let entry = json
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == id.0.to_string())
                .unwrap();
            assert_eq!(entry["role_managed_by_sso"], false);
        }

        // A rule flips the field for the flagged user only.
        let rule_id = create_rule(&pool).await;
        let list = request!(
            &app,
            Method::GET,
            "/api/users",
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(list).await).unwrap();
        let entry = |id: UserId| {
            json.as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == id.0.to_string())
                .unwrap()
        };
        assert_eq!(entry(managed.id)["role_managed_by_sso"], true);
        assert_eq!(entry(admin.id)["role_managed_by_sso"], false);
        assert_eq!(entry(plain.id)["role_managed_by_sso"], false);

        delete_rule(&pool, rule_id);
        delete_user(&pool, admin.id);
        delete_user(&pool, managed.id);
        delete_user(&pool, plain.id);
    }

    #[actix_web::test]
    async fn an_unmanaged_role_changes_fine_while_a_rule_exists() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-sso-unmanaged"), Role::Admin).await;
        let staff = create_user(&pool, unique_email("users-sso-unmanaged"), Role::Staff).await;
        let rule_id = create_rule(&pool).await;
        let token = issue_token(&pool, admin.id).await;

        // The lock follows the flag: SSO never recomputed this user's role.
        let changed = request!(
            &app,
            Method::PUT,
            format!("/api/users/{}/role", staff.id.0),
            Some(&token),
            Some(&serde_json::json!({ "role": "admin" }))
        );
        assert_eq!(changed.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(changed).await).unwrap();
        assert_eq!(json["role"], "admin");

        delete_rule(&pool, rule_id);
        delete_user(&pool, admin.id);
        delete_user(&pool, staff.id);
    }

    #[actix_web::test]
    async fn deactivating_an_sso_managed_user_is_unaffected_by_the_lock() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("users-sso-deactivate"), Role::Admin).await;
        let managed = create_user(&pool, unique_email("users-sso-deactivate"), Role::Staff).await;
        set_sso_managed(&pool, managed.id);
        let rule_id = create_rule(&pool).await;
        let token = issue_token(&pool, admin.id).await;

        // The lock covers role changes only: deactivation still goes through.
        let deactivated = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/deactivate", managed.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(deactivated.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&read_body(deactivated).await).unwrap();
        assert_eq!(json["active"], false);
        // Flag set and a rule present: the response reports the lock on.
        assert_eq!(json["role_managed_by_sso"], true);

        let reactivated = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/reactivate", managed.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(reactivated.status(), StatusCode::OK);

        delete_rule(&pool, rule_id);
        delete_user(&pool, admin.id);
        delete_user(&pool, managed.id);
    }
}
