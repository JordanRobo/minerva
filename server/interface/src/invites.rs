//! Invites HTTP API: the admin-only `/api/invites` handlers and their JSON
//! DTOs.
//!
//! The link rules (who may be invited, when a link stops working) live in
//! [`AccountLinkService`]; these handlers only translate requests and errors
//! to and from HTTP.

use actix_web::{HttpResponse, web};
use application::account_links::{AccountLinkService, IssuedLink};
use application::rate_limit::{ADMIN_ISSUE_ACTOR, RateLimitDecision, RateLimitService};
use chrono::{DateTime, Utc};
use domain::{AccountToken, AccountTokenId, AccountTokenKind, AccountTokenStatus, Role};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::AdminAccess;
use crate::error::{ApiError, account_link_error_response};
use crate::openapi::{InviteStatusDoc, RoleDoc};

/// JSON shape of an invite. The token hash is never part of the wire format,
/// and neither is the raw token — it was shown exactly once, when the invite
/// was created or re-issued, and cannot be recovered afterwards.
#[derive(Serialize, ToSchema)]
pub struct InviteResponse {
    pub id: Uuid,
    pub email: String,
    #[schema(value_type = RoleDoc)]
    pub role: Role,
    /// The invite's state now: `pending`, `accepted`, `revoked` or `expired`.
    #[schema(value_type = InviteStatusDoc)]
    pub status: String,
    /// The admin who issued the invite.
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// When the invite was accepted; `null` while it is not.
    pub consumed_at: Option<DateTime<Utc>>,
    /// When an admin cancelled the invite; `null` while it has not been.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<&AccountToken> for InviteResponse {
    fn from(token: &AccountToken) -> Self {
        // Only invites are ever rendered this way: the list returns invite
        // tokens only, and revoke/reissue refuse non-invite ids.
        let AccountTokenKind::Invite { email, role } = &token.kind else {
            unreachable!("non-invite token rendered as an invite")
        };
        Self {
            id: token.id.0,
            email: email.clone(),
            role: *role,
            status: status_string(token.status(Utc::now())).to_owned(),
            created_by: token.created_by.map(|id| id.0),
            created_at: token.created_at,
            expires_at: token.expires_at,
            consumed_at: token.consumed_at,
            revoked_at: token.revoked_at,
        }
    }
}

fn status_string(status: AccountTokenStatus) -> &'static str {
    match status {
        AccountTokenStatus::Pending => "pending",
        AccountTokenStatus::Accepted => "accepted",
        AccountTokenStatus::Revoked => "revoked",
        AccountTokenStatus::Expired => "expired",
    }
}

/// Body for `POST /api/invites`. A string that is not a role fails JSON
/// deserialization, so the standard 400 handler answers.
#[derive(Deserialize, ToSchema)]
pub struct CreateInviteRequest {
    pub email: String,
    #[schema(value_type = RoleDoc)]
    pub role: Role,
}

/// A freshly issued invite and its one-time accept link. The raw token
/// appears in `link` exactly once — it is not recoverable afterwards (re-issue
/// the invite for a new link).
#[derive(Serialize, ToSchema)]
pub struct InviteCreatedResponse {
    pub invite: InviteResponse,
    /// The one-time accept link; shown only in this response.
    pub link: String,
    /// Whether the invite email went out; when false, share `link` by hand.
    pub emailed: bool,
}

fn created_response(issued: IssuedLink<AccountToken>) -> InviteCreatedResponse {
    InviteCreatedResponse {
        invite: InviteResponse::from(&issued.item),
        link: issued.link,
        emailed: issued.emailed,
    }
}

/// Create an Invite
///
/// Invite an email address to create an account with the given role. The one-time accept link is returned only in this response (and emailed when delivery is configured); the raw token cannot be recovered afterwards — re-issue the invite for a new link. Requires the Admin role. Rate limited per acting admin.
#[utoipa::path(
    post,
    path = "/api/invites",
    tags = ["invites"],
    security(("session_cookie" = [])),
    request_body = CreateInviteRequest,
    responses(
        (status = 201, description = "The invite and its one-time accept link", body = InviteCreatedResponse),
        (status = 400, description = "Not a valid email address", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 409, description = "An account with that email already exists, or an unexpired invite for it is already pending", body = ApiError),
        (status = 429, description = "Too many attempts from this admin; wait the time in the Retry-After header before trying again", body = ApiError)
    )
)]
pub async fn create_invite(
    links: web::Data<AccountLinkService>,
    access: AdminAccess,
    rate_limits: web::Data<RateLimitService>,
    body: web::Json<CreateInviteRequest>,
) -> Result<HttpResponse, ApiError> {
    // The AdminAccess extractor has already answered 401/403; the limiter
    // counts only authenticated admins, per acting user (roadmap 2.8).
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
        .create_invite(&access.user, &body.email, body.role)
        .await
    {
        Ok(issued) => Ok(HttpResponse::Created().json(created_response(issued))),
        Err(error) => Err(account_link_error_response(error)),
    }
}

/// List Invites
///
/// Every invite, newest first, in whatever state it is in. Requires the Admin role.
#[utoipa::path(
    get,
    path = "/api/invites",
    tags = ["invites"],
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "All invites, newest first", body = Vec<InviteResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError)
    )
)]
pub async fn list_invites(
    links: web::Data<AccountLinkService>,
    _access: AdminAccess,
) -> Result<HttpResponse, ApiError> {
    match links.list_invites().await {
        Ok(invites) => {
            Ok(HttpResponse::Ok()
                .json(invites.iter().map(InviteResponse::from).collect::<Vec<_>>()))
        }
        Err(error) => Err(account_link_error_response(error)),
    }
}

/// Revoke an Invite
///
/// Cancel an invite before it is used: its link stops working immediately. Revoking an already-revoked invite succeeds again; an accepted invite cannot be revoked. Requires the Admin role.
#[utoipa::path(
    post,
    path = "/api/invites/{id}/revoke",
    tags = ["invites"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Invite identifier")),
    responses(
        (status = 200, description = "The revoked invite", body = InviteResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No invite with this id", body = ApiError),
        (status = 409, description = "The invite has already been accepted", body = ApiError)
    )
)]
pub async fn revoke_invite(
    links: web::Data<AccountLinkService>,
    _access: AdminAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match links.revoke_invite(AccountTokenId(*path)).await {
        Ok(token) => Ok(HttpResponse::Ok().json(InviteResponse::from(&token))),
        Err(error) => Err(account_link_error_response(error)),
    }
}

/// Re-issue an Invite
///
/// Replace an unused invite with a fresh token and expiry: the old link stops working. The new one-time accept link is returned only in this response; its raw token cannot be recovered afterwards. Refused once the invite has been accepted, or if a user with the invited email has registered in the meantime. Requires the Admin role. Rate limited per acting admin.
#[utoipa::path(
    post,
    path = "/api/invites/{id}/reissue",
    tags = ["invites"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Invite identifier")),
    responses(
        (status = 201, description = "The re-issued invite and its new one-time accept link", body = InviteCreatedResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No invite with this id", body = ApiError),
        (status = 409, description = "The invite has already been accepted, or an account with that email now exists", body = ApiError),
        (status = 429, description = "Too many attempts from this admin; wait the time in the Retry-After header before trying again", body = ApiError)
    )
)]
pub async fn reissue_invite(
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
        .reissue_invite(&access.user, AccountTokenId(*path))
        .await
    {
        Ok(issued) => Ok(HttpResponse::Created().json(created_response(issued))),
        Err(error) => Err(account_link_error_response(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::{Service, ServiceResponse};
    use actix_web::http::{Method, StatusCode, header};
    use actix_web::test::{TestRequest, init_service, read_body};
    use application::auth::SessionService;
    use application::auth::password::PasswordAuthProvider;
    use application::auth::provider::AuthProviders;
    use application::ports::{PasswordHasher, SessionRepository, SessionTokens, UserRepository};
    use application::user_admin::UserAdminService;
    use chrono::{Duration, Utc};
    use diesel::prelude::*;
    use domain::{User, UserId};
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
            panic!("DATABASE_URL is not set; refusing to skip invite API tests in CI");
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
                    .app_data(web::Data::new(UserAdminService::new(
                        users,
                        Arc::new(PostgresSsoGroupRuleRepository::new(pool.clone())),
                        session_service,
                    )))
                    .app_data(providers)
                    .app_data(web::Data::new(CookieSettings { secure: false }))
                    // Rate limiting is disabled in these tests (they assert
                    // invite behaviour, not throttling — the rate-limit tests
                    // run it enabled with a small limit), but the handlers
                    // extract the service, so it must be registered.
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
            display_name: "Invites API test user".into(),
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
    /// and password-reset tokens cascade-delete with it.
    fn delete_user(pool: &PgPool, user_id: UserId) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::users::table.find(user_id.0))
            .execute(&mut conn)
            .expect("delete user");
    }

    /// Delete an account-token row directly (the repository has no delete).
    fn delete_token(pool: &PgPool, id: Uuid) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::account_tokens::table.find(id))
            .execute(&mut conn)
            .expect("delete token");
    }

    /// Push a token's expiry into the past directly in the database (the
    /// service never creates one this old).
    fn expire_token(pool: &PgPool, id: Uuid) {
        let mut conn = pool.get().expect("pool connection");
        diesel::update(infrastructure::schema::account_tokens::table.find(id))
            .set(
                infrastructure::schema::account_tokens::expires_at
                    .eq(Utc::now() - Duration::hours(1)),
            )
            .execute(&mut conn)
            .expect("expire token");
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

    /// The raw token out of an issued link (`...?token=<uuid>`).
    fn raw_token(link: &str) -> String {
        link.rsplit_once('=').unwrap().1.to_owned()
    }

    /// The raw session token out of the `minerva_session` Set-Cookie.
    fn session_token(res: &ServiceResponse) -> String {
        res.headers()
            .get_all(header::SET_COOKIE)
            .filter_map(|value| value.to_str().ok())
            .find(|set| set.starts_with(&format!("{COOKIE_NAME}=")))
            .expect("session cookie set")
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{COOKIE_NAME}="))
            .to_owned()
    }

    #[actix_web::test]
    async fn an_admin_creates_an_invite_and_the_invitee_accepts_it() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-accept"), Role::Admin).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let email = unique_email("invited");

        // The invite: 201 with the one-time link; no SMTP in tests, so
        // `emailed` is false and the link is site-relative.
        let created = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let invite_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();
        assert_eq!(json["invite"]["email"], email);
        assert_eq!(json["invite"]["role"], "staff");
        assert_eq!(json["invite"]["status"], "pending");
        assert!(
            json["invite"].get("token_hash").is_none(),
            "no hash on the wire"
        );
        assert_eq!(json["emailed"], false, "no SMTP in tests");
        let link = json["link"].as_str().unwrap();
        assert!(link.starts_with("/accept-invite?token="), "{link}");
        let raw = raw_token(link);

        // The list shows it.
        let listed = request!(
            &app,
            Method::GET,
            "/api/invites",
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(listed.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(listed).await).unwrap();
        assert!(
            json.as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == invite_id.to_string()),
            "the new invite is in the list"
        );

        // Inspect (public, no session): what the link is for.
        let inspected = request!(
            &app,
            Method::POST,
            "/api/auth/tokens/inspect",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw }))
        );
        assert_eq!(inspected.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(inspected).await).unwrap();
        assert_eq!(json["purpose"], "invite");
        assert_eq!(json["email"], email);
        assert_eq!(json["role"], "staff");

        // Accept (public): creates the account with the invited role and
        // signs it in.
        let accepted = request!(
            &app,
            Method::POST,
            "/api/auth/accept-invite",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw, "password": "long-enough-pass" }))
        );
        assert_eq!(accepted.status(), StatusCode::CREATED);
        let new_cookie = session_token(&accepted);
        let json: serde_json::Value = serde_json::from_slice(&read_body(accepted).await).unwrap();
        assert_eq!(json["email"], email);
        assert_eq!(json["role"], "staff");
        let invited_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

        // The session cookie works.
        let me = request!(
            &app,
            Method::GET,
            "/api/auth/me",
            Some(&new_cookie),
            None::<&serde_json::Value>
        );
        assert_eq!(me.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(me).await).unwrap();
        assert_eq!(json["id"], invited_id.to_string());

        // The link is single-use.
        let reused = request!(
            &app,
            Method::POST,
            "/api/auth/accept-invite",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw, "password": "long-enough-pass" }))
        );
        assert_eq!(reused.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(reused).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        delete_user(&pool, UserId(invited_id));
        delete_token(&pool, invite_id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn reissuing_an_invite_invalidates_the_old_link() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-reissue"), Role::Admin).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let email = unique_email("invited");

        let created = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "read_only" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let first_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();
        let first_raw = raw_token(json["link"].as_str().unwrap());

        // The re-issue returns a fresh link for the same invite email.
        let reissued = request!(
            &app,
            Method::POST,
            format!("/api/invites/{first_id}/reissue"),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(reissued.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(reissued).await).unwrap();
        let second_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();
        assert_ne!(second_id, first_id, "a re-issue is a fresh token row");
        assert_eq!(json["invite"]["email"], email);
        let second_raw = raw_token(json["link"].as_str().unwrap());
        assert_ne!(second_raw, first_raw);

        // The old link is dead...
        let inspected = request!(
            &app,
            Method::POST,
            "/api/auth/tokens/inspect",
            None::<&str>,
            Some(&serde_json::json!({ "token": first_raw }))
        );
        assert_eq!(inspected.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(inspected).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        // ...and the new one works.
        let accepted = request!(
            &app,
            Method::POST,
            "/api/auth/accept-invite",
            None::<&str>,
            Some(&serde_json::json!({ "token": second_raw, "password": "long-enough-pass" }))
        );
        assert_eq!(accepted.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(accepted).await).unwrap();
        let invited_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();

        delete_user(&pool, UserId(invited_id));
        delete_token(&pool, first_id);
        delete_token(&pool, second_id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn revoking_an_invite_kills_its_link() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-revoke"), Role::Admin).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let email = unique_email("invited");

        let created = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let invite_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();
        let raw = raw_token(json["link"].as_str().unwrap());

        // The revoke reports the updated state...
        let revoked = request!(
            &app,
            Method::POST,
            format!("/api/invites/{invite_id}/revoke"),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(revoked.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(revoked).await).unwrap();
        assert_eq!(json["status"], "revoked");

        // ...and the link is dead.
        let inspected = request!(
            &app,
            Method::POST,
            "/api/auth/tokens/inspect",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw }))
        );
        assert_eq!(inspected.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(inspected).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        // Revoking again is a successful no-op.
        let again = request!(
            &app,
            Method::POST,
            format!("/api/invites/{invite_id}/revoke"),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(again.status(), StatusCode::OK);

        delete_token(&pool, invite_id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn an_expired_invite_is_dead_and_listed_as_expired() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-expired"), Role::Admin).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let email = unique_email("invited");

        let created = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let invite_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();
        let raw = raw_token(json["link"].as_str().unwrap());

        // Age the token past its expiry directly in the database.
        expire_token(&pool, invite_id);

        let inspected = request!(
            &app,
            Method::POST,
            "/api/auth/tokens/inspect",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw }))
        );
        assert_eq!(inspected.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(inspected).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        // The list still shows it, as expired.
        let listed = request!(
            &app,
            Method::GET,
            "/api/invites",
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(listed.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(listed).await).unwrap();
        let entry = json
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == invite_id.to_string())
            .unwrap_or_else(|| panic!("invite {invite_id} missing from the list"));
        assert_eq!(entry["status"], "expired");

        delete_token(&pool, invite_id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn creating_an_invite_for_a_registered_email_conflicts() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-registered"), Role::Admin).await;
        let existing = create_user(&pool, unique_email("invited-registered"), Role::Staff).await;
        let admin_token = issue_token(&pool, admin.id).await;

        let res = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": existing.email, "role": "staff" }))
        );
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
        assert_eq!(json["error"]["code"], "account_exists");

        delete_user(&pool, existing.id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn a_duplicate_pending_invite_conflicts() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("invites-dup"), Role::Admin).await;
        let admin_token = issue_token(&pool, admin.id).await;
        let email = unique_email("invited");

        let created = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let invite_id: Uuid = json["invite"]["id"].as_str().unwrap().parse().unwrap();

        // An unexpired invite for the same email is already out there.
        let duplicate = request!(
            &app,
            Method::POST,
            "/api/invites",
            Some(&admin_token),
            Some(&serde_json::json!({ "email": email, "role": "staff" }))
        );
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(duplicate).await).unwrap();
        assert_eq!(json["error"]["code"], "conflict");

        delete_token(&pool, invite_id);
        delete_user(&pool, admin.id);
    }
}
