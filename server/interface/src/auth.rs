//! Auth HTTP API: the `/api/auth/*` handlers (signup, login, logout, me),
//! their JSON DTOs, and the [`AuthenticatedUser`] extractor that resolves a
//! request's session cookie to a logged-in user.
//!
//! Sessions are cookie-based: the client stores a raw token in an HttpOnly
//! cookie; the server stores only its SHA-256 hash, so a leaked database
//! cannot be turned into live sessions.

use actix_web::cookie::{Cookie, SameSite, time::OffsetDateTime};
use actix_web::dev::Payload;
use actix_web::{FromRequest, HttpRequest, HttpResponse, web};
use application::auth::password::PASSWORD_PROVIDER_ID;
use application::auth::provider::{AuthError, AuthProviders, Credentials};
use application::auth::{SessionService, normalize_email};
use application::ports::{PasswordHasher, UserRepository};
use chrono::{DateTime, Utc};
use domain::{User, UserId};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, repo_error_response};

/// Cookie attributes shared by the session and OIDC-state cookies. Built from
/// `server.cookie_secure` in the configuration and registered as `web::Data`.
#[derive(Debug, Clone, Copy)]
pub struct CookieSettings {
    /// Set the `Secure` flag: true once the deployment is served over https;
    /// false for local http:// development, where browsers would drop a
    /// Secure cookie.
    pub secure: bool,
}

/// Name of the session cookie.
const COOKIE_NAME: &str = "minerva_session";

/// JSON shape of a user in auth responses. Deliberately omits
/// `password_hash` — it is server-side only and must never cross the wire.
#[derive(Serialize, ToSchema)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&User> for UserResponse {
    fn from(user: &User) -> Self {
        Self {
            id: user.id.0,
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

/// Body for `POST /api/auth/signup`.
#[derive(Deserialize, ToSchema)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub display_name: String,
}

/// Body for `POST /api/auth/login`.
#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// The user authenticated by the request's session cookie. Use as a handler
/// argument for endpoints that require a valid session; requests without one
/// get a 401 before the handler runs.
pub struct AuthenticatedUser {
    pub user: User,
}

impl FromRequest for AuthenticatedUser {
    type Error = ApiError;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        // The future must be 'static, so work on an owned clone of the request
        // (cheap: headers + extensions).
        let req = req.clone();
        Box::pin(async move {
            let service = req
                .app_data::<web::Data<SessionService>>()
                .ok_or_else(ApiError::internal_error)?;
            let users = req
                .app_data::<web::Data<dyn UserRepository>>()
                .ok_or_else(ApiError::internal_error)?;

            // Every failure mode (no cookie, unknown token, expired session,
            // vanished user) is the same 401: the response must not hint which.
            let Some(cookie) = req.cookie(COOKIE_NAME) else {
                return Err(ApiError::unauthorized());
            };
            let session = service
                .resolve(cookie.value())
                .await
                .map_err(repo_error_response)?;
            let Some(session) = session else {
                return Err(ApiError::unauthorized());
            };
            let user = users
                .find_by_id(session.user_id)
                .await
                .map_err(repo_error_response)?;
            let Some(user) = user else {
                return Err(ApiError::unauthorized());
            };
            Ok(Self { user })
        })
    }
}

/// Create a session for `user_id` and return the cookie carrying its raw
/// token. Only the token's hash is stored; from here on the raw token exists
/// only in the response cookie. Shared with the OIDC callback so sessions are
/// indistinguishable regardless of how the user signed in.
pub(crate) async fn issue_session(
    service: &SessionService,
    user_id: UserId,
    cookies: &CookieSettings,
) -> Result<Cookie<'static>, ApiError> {
    let issued = service.issue(user_id).await.map_err(repo_error_response)?;
    Ok(session_cookie(
        &issued.token,
        issued.session.expires_at,
        cookies,
    ))
}

/// Build the session cookie: HttpOnly so JavaScript cannot read it,
/// SameSite=Lax as a CSRF baseline, scoped to the whole site, and expiring
/// with the session. `Secure` comes from the configuration (server.cookie_secure)
/// because local dev talks plain HTTP (bun run dev -> localhost API), where
/// browsers would drop a Secure cookie; production must set it.
fn session_cookie(
    token: &str,
    expires_at: DateTime<Utc>,
    cookies: &CookieSettings,
) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE_NAME, "");
    cookie.set_value(token.to_owned());
    apply_session_attributes(&mut cookie, cookies);
    // Second precision is all a cookie expiry needs; the unix-timestamp
    // constructor avoids time's chrono feature (not enabled in our tree).
    cookie.set_expires(
        OffsetDateTime::from_unix_timestamp(expires_at.timestamp()).expect("valid session expiry"),
    );
    cookie
}

/// A cookie that makes the browser drop the session (empty value, past
/// expiry), carrying the same attributes so it matches the original.
fn clear_cookie(cookies: &CookieSettings) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE_NAME, "");
    apply_session_attributes(&mut cookie, cookies);
    cookie.make_removal();
    cookie
}

fn apply_session_attributes(cookie: &mut Cookie<'_>, cookies: &CookieSettings) {
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    if cookies.secure {
        cookie.set_secure(true);
    }
}

/// Sign Up
///
/// Create an account and start a session: the user is created with a hashed
/// password and a session cookie is set on the response.
#[utoipa::path(
    post,
    path = "/api/auth/signup",
    tags = ["auth"],
    request_body = SignupRequest,
    responses(
        (status = 201, description = "Account created; session cookie set", body = UserResponse),
        (status = 400, description = "Email or password is blank, or the password is under 8 characters", body = ApiError),
        (status = 409, description = "A user with this email already exists", body = ApiError)
    )
)]
pub async fn signup(
    users: web::Data<dyn UserRepository>,
    session_service: web::Data<SessionService>,
    password_hasher: web::Data<dyn PasswordHasher>,
    cookies: web::Data<CookieSettings>,
    body: web::Json<SignupRequest>,
) -> Result<HttpResponse, ApiError> {
    // Normalize once at the boundary: lookups are case-insensitive, so the
    // stored email must be too or duplicate detection would miss "Foo@x.com".
    let email = normalize_email(&body.email);
    if email.is_empty() {
        return Err(ApiError::bad_request("email must not be empty"));
    }
    if body.password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters long",
        ));
    }
    let password_hash = password_hasher
        .hash(&body.password)
        .await
        .map_err(|_| ApiError::internal_error())?;
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: Some(password_hash),
        display_name: body.display_name.clone(),
        created_at: now,
        updated_at: now,
    };
    let user = users.create(user).await.map_err(repo_error_response)?;
    let cookie = issue_session(session_service.get_ref(), user.id, &cookies).await?;
    Ok(HttpResponse::Created()
        .cookie(cookie)
        .json(UserResponse::from(&user)))
}

/// Log In
///
/// Exchange credentials for a session cookie. Failures return one generic
/// 401 whether the email is unknown or the password is wrong.
#[utoipa::path(
    post,
    path = "/api/auth/login",
    tags = ["auth"],
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Credentials valid; session cookie set", body = UserResponse),
        (status = 401, description = "Invalid credentials", body = ApiError)
    )
)]
pub async fn login(
    providers: web::Data<AuthProviders>,
    session_service: web::Data<SessionService>,
    cookies: web::Data<CookieSettings>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, ApiError> {
    // The handler only translates the request into provider terms; every
    // check happens in the registered provider.
    let LoginRequest { email, password } = body.into_inner();
    let credentials = Credentials {
        identifier: email,
        secret: password,
    };
    let Some(provider) = providers.credential(PASSWORD_PROVIDER_ID) else {
        return Err(ApiError::internal_error());
    };
    let user = provider
        .authenticate(credentials)
        .await
        .map_err(|error| match error {
            // Every rejection renders as the same generic 401: the response must
            // not reveal whether the email exists or which check failed.
            AuthError::InvalidCredentials
            | AuthError::Rejected { .. }
            | AuthError::Failed { .. } => ApiError::unauthorized(),
            AuthError::Internal(detail) => {
                eprintln!("password login failed: {detail}");
                ApiError::internal_error()
            }
        })?;
    let cookie = issue_session(session_service.get_ref(), user.id, &cookies).await?;
    Ok(HttpResponse::Ok()
        .cookie(cookie)
        .json(UserResponse::from(&user)))
}

/// Log Out
///
/// Delete the session behind the request's cookie and clear the cookie.
/// Idempotent: logging out with no (valid) session still succeeds.
#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tags = ["auth"],
    responses((status = 204, description = "Session deleted; cookie cleared"))
)]
pub async fn logout(
    req: HttpRequest,
    session_service: web::Data<SessionService>,
    cookies: web::Data<CookieSettings>,
) -> Result<HttpResponse, ApiError> {
    if let Some(cookie) = req.cookie(COOKIE_NAME) {
        session_service
            .revoke(cookie.value())
            .await
            .map_err(repo_error_response)?;
    }
    Ok(HttpResponse::NoContent()
        .cookie(clear_cookie(&cookies))
        .finish())
}

/// Current User
///
/// The user behind the request's session cookie.
#[utoipa::path(
    get,
    path = "/api/auth/me",
    tags = ["auth"],
    responses(
        (status = 200, description = "The authenticated user", body = UserResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn me(auth: AuthenticatedUser) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(UserResponse::from(&auth.user)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::{Service, ServiceResponse};
    use actix_web::http::{StatusCode, header};
    use actix_web::test::{TestRequest, init_service, read_body};
    use application::auth::password::PasswordAuthProvider;
    use application::ports::{SessionRepository, SessionTokens};
    use chrono::Duration;
    use diesel::prelude::*;
    use domain::{Session, SessionId};
    use infrastructure::db::PgPool;
    use infrastructure::repositories::{PostgresSessionRepository, PostgresUserRepository};
    use infrastructure::{Argon2PasswordHasher, Sha256SessionTokens};
    use std::sync::Arc;

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip auth tests in CI");
        }
        url
    }

    /// A one-connection pool: the default settings (max_size 10, min_idle =
    /// max_size) times the parallel handler-test pools would exceed local
    /// Postgres's `max_connections`. Pending migrations are applied once per
    /// process first, so the tests are self-sufficient against a fresh
    /// database.
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

    /// Build the auth app (real Postgres repositories and hasher, routes as
    /// in `main.rs`) and initialize it. A macro because `init_service`'s
    /// service type is opaque and cannot be named in a helper's signature.
    macro_rules! test_app {
        ($url:expr) => {{
            let pool = test_pool($url);
            // The handlers take ports, so register trait objects — a missing
            // required Data is a 500. Sessions are registered both raw and
            // via the service, mirroring `main.rs`.
            let sessions: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool.clone()));
            let sessions_data: web::Data<dyn SessionRepository> = sessions.clone().into();
            let users: Arc<dyn UserRepository> =
                Arc::new(PostgresUserRepository::new(pool.clone()));
            let users_data: web::Data<dyn UserRepository> = users.clone().into();
            let hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
            let hasher_data: web::Data<dyn PasswordHasher> = hasher.clone().into();
            // The login handler resolves providers by id, mirroring `main.rs`.
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
            init_service(
                App::new()
                    .app_data(users_data)
                    .app_data(sessions_data)
                    .app_data(web::Data::new(SessionService::new(
                        sessions,
                        Arc::new(Sha256SessionTokens),
                        SessionService::DEFAULT_SESSION_TTL,
                    )))
                    .app_data(providers)
                    .app_data(hasher_data)
                    .app_data(web::Data::new(CookieSettings { secure: false }))
                    .route("/api/auth/signup", web::post().to(signup))
                    .route("/api/auth/login", web::post().to(login))
                    .route("/api/auth/logout", web::post().to(logout))
                    .route("/api/auth/me", web::get().to(me)),
            )
            .await
        }};
    }

    // Request helpers are macros rather than generic functions: `init_service`'s
    // service type is opaque, and the request type it takes
    // (`actix_http::Request`) cannot be named here without a direct `actix-http`
    // dependency (oidc.rs's test module uses macros for the same reason).

    macro_rules! post_json {
        ($app:expr, $uri:expr, $body:expr $(,)?) => {{
            $app.call(TestRequest::post().uri($uri).set_json($body).to_request())
                .await
                .unwrap()
        }};
    }

    /// GET `uri` with the session cookie set to `token`, or no cookie.
    macro_rules! get {
        ($app:expr, $uri:expr, $token:expr) => {{
            let mut req = TestRequest::get().uri($uri);
            let token: Option<&str> = $token;
            if let Some(t) = token {
                req = req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={t}")));
            }
            $app.call(req.to_request()).await.unwrap()
        }};
    }

    /// POST `/api/auth/logout` with the session cookie set to `token`, or no
    /// cookie.
    macro_rules! post_logout {
        ($app:expr, $token:expr) => {{
            let mut req = TestRequest::post().uri("/api/auth/logout");
            let token: Option<&str> = $token;
            if let Some(t) = token {
                req = req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={t}")));
            }
            $app.call(req.to_request()).await.unwrap()
        }};
    }

    /// The full value of the first `Set-Cookie` for `name`, if any.
    fn set_cookie(res: &ServiceResponse, name: &str) -> Option<String> {
        res.headers()
            .get_all(header::SET_COOKIE)
            .filter_map(|value| value.to_str().ok())
            .find(|set| set.starts_with(&format!("{name}=")))
            .map(str::to_owned)
    }

    /// The raw session token out of the `minerva_session` Set-Cookie.
    fn session_token(res: &ServiceResponse) -> String {
        set_cookie(res, COOKIE_NAME)
            .expect("session cookie set")
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{COOKIE_NAME}="))
            .to_owned()
    }

    fn unique_email(prefix: &str) -> String {
        format!("{prefix}-{}@example.com", Uuid::new_v4())
    }

    /// A password account with a real Argon2 hash, created directly so login
    /// tests do not depend on signup (which roadmap 2.5 removes).
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
            display_name: "Auth test user".into(),
            created_at: now,
            updated_at: now,
        };
        users.create(user).await.expect("create user")
    }

    /// Delete a user row directly (the repository has no delete); its
    /// sessions cascade-delete with it.
    fn delete_user(pool: &PgPool, user_id: UserId) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::users::table.find(user_id.0))
            .execute(&mut conn)
            .expect("delete user");
    }

    // ---- signup ----
    // These pin CURRENT behaviour: open signup exists today and is removed by
    // roadmap 2.5 (invite-only accounts). Replace when that lands.

    #[actix_web::test]
    async fn signup_creates_the_user_and_sets_a_session_cookie() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let email = unique_email("signup");
        let app = test_app!(&url);
        let res = post_json!(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "email": format!("  {}  ", email.to_uppercase()),
                "password": "password123",
                "display_name": "Signup user",
            }),
        );
        assert_eq!(res.status(), StatusCode::CREATED);

        // The cookie is HttpOnly (JS cannot read it), Lax (CSRF baseline)
        // and scoped to the whole site.
        let set = set_cookie(&res, COOKIE_NAME).expect("session cookie set");
        assert!(set.contains("HttpOnly"), "{set}");
        assert!(set.contains("SameSite=Lax"), "{set}");
        assert!(set.contains("Path=/"), "{set}");

        // The email is trimmed and lowercased; the hash never crosses the wire.
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["email"], email);
        assert_eq!(json["display_name"], "Signup user");
        assert!(json.get("password_hash").is_none(), "no hash in response");

        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let user = users
            .find_by_email(email)
            .await
            .unwrap()
            .expect("user created");
        delete_user(&pool, user.id);
    }

    #[actix_web::test]
    async fn signup_rejects_a_blank_email() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(&url);
        let res = post_json!(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "email": "   ",
                "password": "password123",
                "display_name": "Blank",
            }),
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "bad_request");
    }

    #[actix_web::test]
    async fn signup_rejects_a_short_password() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(&url);
        let res = post_json!(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "email": unique_email("short"),
                "password": "short",
                "display_name": "Short password",
            }),
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "bad_request");
    }

    #[actix_web::test]
    async fn signup_duplicate_email_is_a_conflict() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let email = unique_email("dup");
        let app = test_app!(&url);
        let first = post_json!(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "email": email.clone(),
                "password": "password123",
                "display_name": "First",
            }),
        );
        assert_eq!(first.status(), StatusCode::CREATED);

        // Same address in another case: normalization must still catch it.
        let second = post_json!(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "email": email.to_uppercase(),
                "password": "password123",
                "display_name": "Second",
            }),
        );
        assert_eq!(second.status(), StatusCode::CONFLICT);
        let body = read_body(second).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "conflict");

        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let user = users
            .find_by_email(email)
            .await
            .unwrap()
            .expect("user created");
        delete_user(&pool, user.id);
    }

    // ---- login ----

    #[actix_web::test]
    async fn login_success_sets_a_session_cookie() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let email = unique_email("login");
        let user = create_password_user(&pool, email.clone()).await;
        let app = test_app!(&url);
        let res = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": email, "password": "password123" }),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!session_token(&res).is_empty());
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["email"], email);

        delete_user(&pool, user.id);
    }

    #[actix_web::test]
    async fn login_email_is_case_insensitive() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let email = unique_email("case");
        let user = create_password_user(&pool, email.clone()).await;
        let app = test_app!(&url);
        let res = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": email.to_uppercase(), "password": "password123" }),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!session_token(&res).is_empty());

        delete_user(&pool, user.id);
    }

    #[actix_web::test]
    async fn login_failures_are_indistinguishable() {
        // Wrong password, unknown email and a passwordless account must all
        // produce the same status and a byte-identical body: the response
        // must not reveal which accounts exist.
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let known = create_password_user(&pool, unique_email("known")).await;
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let passwordless = User {
            id: UserId::new(),
            email: unique_email("sso"),
            password_hash: None,
            display_name: "SSO only".into(),
            created_at: now,
            updated_at: now,
        };
        users
            .create(passwordless.clone())
            .await
            .expect("create passwordless user");

        let app = test_app!(&url);
        let wrong_password = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": known.email, "password": "not-the-password" }),
        );
        assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
        let unknown_email = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": unique_email("ghost"), "password": "password123" }),
        );
        assert_eq!(unknown_email.status(), StatusCode::UNAUTHORIZED);
        let passwordless_login = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": passwordless.email, "password": "password123" }),
        );
        assert_eq!(passwordless_login.status(), StatusCode::UNAUTHORIZED);

        let bodies = [
            read_body(wrong_password).await,
            read_body(unknown_email).await,
            read_body(passwordless_login).await,
        ];
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(bodies[0], bodies[2]);
        let json: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
        assert_eq!(json["error"]["code"], "unauthorized");

        delete_user(&pool, known.id);
        delete_user(&pool, passwordless.id);
    }

    // ---- me ----

    #[actix_web::test]
    async fn me_returns_the_user_for_a_valid_session() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let email = unique_email("me");
        let user = create_password_user(&pool, email.clone()).await;
        let app = test_app!(&url);
        let login = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": email, "password": "password123" }),
        );
        assert_eq!(login.status(), StatusCode::OK);
        let token = session_token(&login);

        let res = get!(&app, "/api/auth/me", Some(&token));
        assert_eq!(res.status(), StatusCode::OK);
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["email"], email);
        assert_eq!(json["id"], user.id.0.to_string());

        delete_user(&pool, user.id);
    }

    #[actix_web::test]
    async fn me_without_a_cookie_is_unauthorized() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(&url);
        let res = get!(&app, "/api/auth/me", None);
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body = read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "unauthorized");
    }

    #[actix_web::test]
    async fn me_with_a_garbage_cookie_is_unauthorized() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(&url);
        let res = get!(&app, "/api/auth/me", Some("not-a-real-token"));
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn me_with_an_expired_session_is_unauthorized() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let user = create_password_user(&pool, unique_email("expired")).await;
        // A session already past its expiry, created directly: the repository
        // still returns it, so the 401 must come from the extractor's
        // `is_expired` check.
        let sessions = PostgresSessionRepository::new(pool.clone());
        let tokens = Sha256SessionTokens;
        let now = Utc::now();
        sessions
            .create(Session {
                id: SessionId::new(),
                user_id: user.id,
                token_hash: tokens.hash("expired-token"),
                created_at: now - Duration::hours(2),
                expires_at: now - Duration::hours(1),
                last_seen_at: now - Duration::hours(2),
            })
            .await
            .expect("create expired session");

        let app = test_app!(&url);
        let res = get!(&app, "/api/auth/me", Some("expired-token"));
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        delete_user(&pool, user.id);
    }

    // ---- logout ----

    #[actix_web::test]
    async fn logout_deletes_the_session_and_clears_the_cookie() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let email = unique_email("logout");
        let user = create_password_user(&pool, email.clone()).await;
        let app = test_app!(&url);
        let login = post_json!(
            &app,
            "/api/auth/login",
            serde_json::json!({ "email": email, "password": "password123" }),
        );
        assert_eq!(login.status(), StatusCode::OK);
        let token = session_token(&login);

        let res = post_logout!(&app, Some(&token));
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        // The browser is told to drop the cookie.
        let set = set_cookie(&res, COOKIE_NAME).expect("cookie cleared");
        assert!(set.contains("Max-Age=0"), "{set}");

        // The session row is gone: the old cookie no longer authenticates.
        let me = get!(&app, "/api/auth/me", Some(&token));
        assert_eq!(me.status(), StatusCode::UNAUTHORIZED);

        delete_user(&pool, user.id);
    }

    #[actix_web::test]
    async fn logout_without_a_cookie_still_succeeds() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(&url);
        let res = post_logout!(&app, None);
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(set_cookie(&res, COOKIE_NAME).is_some(), "cookie cleared");
    }
}
