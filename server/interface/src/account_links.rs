//! Public account-link endpoints: inspecting a one-time link and acting on it
//! (accepting an invite, resetting a password). No session is required — the
//! token in the request body is the credential. Tokens go in bodies, never in
//! URLs, so they stay out of access logs.
//!
//! The link rules live in [`AccountLinkService`]; these handlers only
//! translate requests and errors to and from HTTP.

use actix_web::{HttpResponse, web};
use application::account_links::{AccountLinkService, LinkInfo, LinkPurpose};
use application::auth::SessionService;
use chrono::{DateTime, Utc};
use domain::Role;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{CookieSettings, UserResponse, issue_session};
use crate::error::{ApiError, account_link_error_response};
use crate::openapi::{LinkPurposeDoc, RoleDoc};

/// Body for `POST /api/auth/tokens/inspect`.
#[derive(Deserialize, ToSchema)]
pub struct InspectTokenRequest {
    /// The raw token out of the link's `token` query parameter.
    pub token: String,
}

/// What a link is for and when it stops working, shown to its holder before
/// they act on it.
#[derive(Serialize, ToSchema)]
pub struct LinkInfoResponse {
    /// `invite` or `password_reset`.
    #[schema(value_type = LinkPurposeDoc)]
    pub purpose: String,
    /// Who the link is for: the invited address, or the reset target's email.
    pub email: String,
    /// The role an invite grants; `null` for password resets.
    #[schema(value_type = Option<RoleDoc>, nullable = true)]
    pub role: Option<Role>,
    pub expires_at: DateTime<Utc>,
}

impl From<&LinkInfo> for LinkInfoResponse {
    fn from(info: &LinkInfo) -> Self {
        Self {
            purpose: purpose_string(info.purpose).to_owned(),
            email: info.email.clone(),
            role: info.role,
            expires_at: info.expires_at,
        }
    }
}

fn purpose_string(purpose: LinkPurpose) -> &'static str {
    match purpose {
        LinkPurpose::Invite => "invite",
        LinkPurpose::PasswordReset => "password_reset",
    }
}

/// Body for `POST /api/auth/accept-invite`.
#[derive(Deserialize, ToSchema)]
pub struct AcceptInviteRequest {
    /// The raw token out of the link's `token` query parameter.
    pub token: String,
    pub password: String,
    /// Shown on the account; when blank, the email's local part is used.
    #[schema(nullable = true)]
    pub display_name: Option<String>,
}

/// Body for `POST /api/auth/reset-password`.
#[derive(Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    /// The raw token out of the link's `token` query parameter.
    pub token: String,
    pub password: String,
}

/// Inspect an Account Link
///
/// What a link is for (an invite, with its role, or a password reset) and when it stops working, shown to the holder before they act on it. Unknown, expired, already used and revoked links all get the same 400 — the answer never hints which.
#[utoipa::path(
    post,
    path = "/api/auth/tokens/inspect",
    tags = ["auth"],
    request_body = InspectTokenRequest,
    responses(
        (status = 200, description = "What the link is for", body = LinkInfoResponse),
        (status = 400, description = "The link is unknown, expired, already used or revoked", body = ApiError)
    )
)]
pub async fn inspect_token(
    links: web::Data<AccountLinkService>,
    body: web::Json<InspectTokenRequest>,
) -> Result<HttpResponse, ApiError> {
    match links.inspect(&body.token).await {
        Ok(info) => Ok(HttpResponse::Ok().json(LinkInfoResponse::from(&info))),
        Err(error) => Err(account_link_error_response(error)),
    }
}

/// Accept an Invite
///
/// Create the account an invite grants and sign it in: the response carries a session cookie. The link is single-use — accepting burns the token, while a too-short password leaves it usable for another attempt.
#[utoipa::path(
    post,
    path = "/api/auth/accept-invite",
    tags = ["auth"],
    request_body = AcceptInviteRequest,
    responses(
        (status = 201, description = "The created user; session cookie set", body = UserResponse),
        (status = 400, description = "The link is not valid, or the password is too short", body = ApiError),
        (status = 409, description = "An account with that email already exists", body = ApiError)
    )
)]
pub async fn accept_invite(
    links: web::Data<AccountLinkService>,
    session_service: web::Data<SessionService>,
    cookies: web::Data<CookieSettings>,
    body: web::Json<AcceptInviteRequest>,
) -> Result<HttpResponse, ApiError> {
    let AcceptInviteRequest {
        token,
        password,
        display_name,
    } = body.into_inner();
    let user = links
        .accept_invite(&token, &password, display_name)
        .await
        .map_err(account_link_error_response)?;
    let cookie = issue_session(session_service.get_ref(), user.id, &cookies).await?;
    Ok(HttpResponse::Created()
        .cookie(cookie)
        .json(UserResponse::from(&user)))
}

/// Reset a Password
///
/// Set a new password with a reset link. The link is single-use, and every session of the account stops working — sign in again with the new password. No session cookie is set here.
#[utoipa::path(
    post,
    path = "/api/auth/reset-password",
    tags = ["auth"],
    request_body = ResetPasswordRequest,
    responses(
        (status = 204, description = "Password changed; all previous sessions revoked"),
        (status = 400, description = "The link is not valid, or the password is too short", body = ApiError)
    )
)]
pub async fn reset_password(
    links: web::Data<AccountLinkService>,
    body: web::Json<ResetPasswordRequest>,
) -> Result<HttpResponse, ApiError> {
    links
        .reset_password(&body.token, &body.password)
        .await
        .map_err(account_link_error_response)?;
    Ok(HttpResponse::NoContent().finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::{Service, ServiceResponse};
    use actix_web::http::{Method, StatusCode, header};
    use actix_web::test::{TestRequest, init_service, read_body};
    use application::auth::password::PasswordAuthProvider;
    use application::auth::provider::AuthProviders;
    use application::ports::{PasswordHasher, SessionRepository, SessionTokens, UserRepository};
    use application::user_admin::UserAdminService;
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
    use uuid::Uuid;

    use crate::auth::COOKIE_NAME;
    use crate::routes;

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip account-link API tests in CI");
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

    /// A user created directly in the database (no password: SSO-style).
    async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            email,
            password_hash: None,
            display_name: "Account links API test user".into(),
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
            display_name: "Account links API test user".into(),
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        };
        users.create(user).await.expect("create user")
    }

    /// A deactivated password account.
    async fn create_deactivated_user(pool: &PgPool, email: String) -> User {
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
            display_name: "Account links API test user".into(),
            role: Role::Staff,
            deactivated_at: Some(now),
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
    async fn a_password_reset_link_works_once_and_revokes_sessions() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("reset-admin"), Role::Admin).await;
        let target = create_password_user(&pool, unique_email("reset-target"), Role::Staff).await;
        let admin_token = issue_token(&pool, admin.id).await;

        // The target signs in with the old password.
        let logged_in = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&serde_json::json!({ "email": target.email, "password": "password123" }))
        );
        assert_eq!(logged_in.status(), StatusCode::OK);
        let old_session = session_token(&logged_in);

        // The admin issues a reset link: 201; no SMTP in tests, so the link
        // comes back site-relative.
        let issued = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/password-reset", target.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(issued.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(issued).await).unwrap();
        assert_eq!(json["emailed"], false, "no SMTP in tests");
        assert!(json["expires_at"].is_string());
        let link = json["link"].as_str().unwrap();
        assert!(link.starts_with("/reset-password?token="), "{link}");
        let first_raw = raw_token(link);

        // A second issue replaces the live one, so the first link is dead.
        let again = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/password-reset", target.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(again.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(again).await).unwrap();
        let second_raw = raw_token(json["link"].as_str().unwrap());
        assert_ne!(second_raw, first_raw);

        let stale = request!(
            &app,
            Method::POST,
            "/api/auth/reset-password",
            None::<&str>,
            Some(&serde_json::json!({ "token": first_raw, "password": "brand-new-pass" }))
        );
        assert_eq!(stale.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(stale).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        // The second link sets the new password.
        let reset = request!(
            &app,
            Method::POST,
            "/api/auth/reset-password",
            None::<&str>,
            Some(&serde_json::json!({ "token": second_raw, "password": "brand-new-pass" }))
        );
        assert_eq!(reset.status(), StatusCode::NO_CONTENT);

        // Every old session is gone...
        let me = request!(
            &app,
            Method::GET,
            "/api/auth/me",
            Some(&old_session),
            None::<&serde_json::Value>
        );
        assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
        // ...the old password no longer works...
        let old_login = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&serde_json::json!({ "email": target.email, "password": "password123" }))
        );
        assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED);
        // ...and the new one does.
        let new_login = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            None::<&str>,
            Some(&serde_json::json!({ "email": target.email, "password": "brand-new-pass" }))
        );
        assert_eq!(new_login.status(), StatusCode::OK);
        let new_session = session_token(&new_login);
        let me = request!(
            &app,
            Method::GET,
            "/api/auth/me",
            Some(&new_session),
            None::<&serde_json::Value>
        );
        assert_eq!(me.status(), StatusCode::OK);

        // The link is single-use.
        let reused = request!(
            &app,
            Method::POST,
            "/api/auth/reset-password",
            None::<&str>,
            Some(&serde_json::json!({ "token": second_raw, "password": "brand-new-pass" }))
        );
        assert_eq!(reused.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(reused).await).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");

        delete_user(&pool, target.id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn a_reset_is_refused_for_sso_only_and_deactivated_users() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("reset-refuse-admin"), Role::Admin).await;
        let sso_only = create_user(&pool, unique_email("reset-sso-only"), Role::Staff).await;
        let deactivated = create_deactivated_user(&pool, unique_email("reset-deactivated")).await;
        let admin_token = issue_token(&pool, admin.id).await;

        // No password on the account: nothing to reset.
        let sso = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/password-reset", sso_only.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(sso.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(sso).await).unwrap();
        assert_eq!(json["error"]["code"], "conflict");
        assert!(
            json["error"]["message"]
                .to_string()
                .contains("external provider"),
            "{}",
            json["error"]["message"]
        );

        // A deactivated account cannot be reset either.
        let off = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/password-reset", deactivated.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(off.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(off).await).unwrap();
        assert_eq!(json["error"]["code"], "conflict");
        assert!(
            json["error"]["message"].to_string().contains("deactivated"),
            "{}",
            json["error"]["message"]
        );

        delete_user(&pool, sso_only.id);
        delete_user(&pool, deactivated.id);
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn a_short_password_is_rejected_and_the_link_stays_usable() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);

        let admin = create_user(&pool, unique_email("reset-short-admin"), Role::Admin).await;
        let target = create_password_user(&pool, unique_email("reset-short"), Role::Staff).await;
        let admin_token = issue_token(&pool, admin.id).await;

        let issued = request!(
            &app,
            Method::POST,
            format!("/api/users/{}/password-reset", target.id.0),
            Some(&admin_token),
            None::<&serde_json::Value>
        );
        assert_eq!(issued.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(issued).await).unwrap();
        let raw = raw_token(json["link"].as_str().unwrap());

        // Too short: 400, and the link survives the failed attempt.
        let short = request!(
            &app,
            Method::POST,
            "/api/auth/reset-password",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw, "password": "short" }))
        );
        assert_eq!(short.status(), StatusCode::BAD_REQUEST);
        let json: serde_json::Value = serde_json::from_slice(&read_body(short).await).unwrap();
        assert_eq!(json["error"]["code"], "bad_request");

        let ok = request!(
            &app,
            Method::POST,
            "/api/auth/reset-password",
            None::<&str>,
            Some(&serde_json::json!({ "token": raw, "password": "brand-new-pass" }))
        );
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);

        delete_user(&pool, target.id);
        delete_user(&pool, admin.id);
    }
}
