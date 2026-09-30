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
use application::ports::{PasswordHashError, PasswordHasher, SessionRepository, UserRepository};
use chrono::{DateTime, Duration, Utc};
use domain::{Session, SessionId, User, UserId};
use infrastructure::Argon2PasswordHasher;
use infrastructure::repositories::PostgresUserRepository;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, repo_error_response};

/// Name of the session cookie.
const COOKIE_NAME: &str = "minerva_session";

/// How long a session stays valid after it is created.
const SESSION_TTL: Duration = Duration::days(30);

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
            let sessions = req
                .app_data::<web::Data<dyn SessionRepository>>()
                .ok_or_else(ApiError::internal_error)?;
            let users = req
                .app_data::<web::Data<PostgresUserRepository>>()
                .ok_or_else(ApiError::internal_error)?;

            // Every failure mode (no cookie, unknown token, expired session,
            // vanished user) is the same 401: the response must not hint which.
            let Some(cookie) = req.cookie(COOKIE_NAME) else {
                return Err(ApiError::unauthorized());
            };
            let session = sessions
                .find_by_token_hash(hash_token(cookie.value()))
                .await
                .map_err(repo_error_response)?;
            let Some(session) = session.filter(|s| !s.is_expired(Utc::now())) else {
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

/// Hash a raw session token for storage and lookup. One-way (SHA-256) is all
/// that is needed: the threat is a leaked database revealing live tokens, not
/// an attacker computing hashes to compare.
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Run a (CPU-bound) password hash or verify off the async runtime's worker
/// threads, mirroring `run_on_postgres`/`run_on_redis` in infrastructure.
async fn run_hasher<T, F>(hasher: web::Data<Argon2PasswordHasher>, op: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&Argon2PasswordHasher) -> Result<T, PasswordHashError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || op(hasher.get_ref()))
        .await
        .map_err(|_| ApiError::internal_error())?
        .map_err(|_| ApiError::internal_error())
}

/// Create a session for `user_id` and return the cookie carrying its raw
/// token. Only the token's hash is stored; from here on the raw token exists
/// only in the response cookie. Shared with the OIDC callback so sessions are
/// indistinguishable regardless of how the user signed in.
pub(crate) async fn issue_session(
    sessions: &web::Data<dyn SessionRepository>,
    user_id: UserId,
) -> Result<Cookie<'static>, ApiError> {
    let now = Utc::now();
    let expires_at = now + SESSION_TTL;
    let token = Uuid::new_v4().to_string();
    let session = Session {
        id: SessionId::new(),
        user_id,
        token_hash: hash_token(&token),
        created_at: now,
        expires_at,
        last_seen_at: now,
    };
    sessions
        .create(session)
        .await
        .map_err(repo_error_response)?;
    Ok(session_cookie(&token, expires_at))
}

/// Build the session cookie: HttpOnly so JavaScript cannot read it,
/// SameSite=Lax as a CSRF baseline, scoped to the whole site, and expiring
/// with the session. `Secure` is opt-in via `COOKIE_SECURE=true` because
/// local dev talks plain HTTP (bun run dev -> localhost API), where browsers
/// would drop a Secure cookie; production must set it.
fn session_cookie(token: &str, expires_at: DateTime<Utc>) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE_NAME, "");
    cookie.set_value(token.to_owned());
    apply_session_attributes(&mut cookie);
    // Second precision is all a cookie expiry needs; the unix-timestamp
    // constructor avoids time's chrono feature (not enabled in our tree).
    cookie.set_expires(
        OffsetDateTime::from_unix_timestamp(expires_at.timestamp()).expect("valid session expiry"),
    );
    cookie
}

/// A cookie that makes the browser drop the session (empty value, past
/// expiry), carrying the same attributes so it matches the original.
fn clear_cookie() -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE_NAME, "");
    apply_session_attributes(&mut cookie);
    cookie.make_removal();
    cookie
}

fn apply_session_attributes(cookie: &mut Cookie<'_>) {
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    if std::env::var("COOKIE_SECURE").is_ok_and(|value| value.eq_ignore_ascii_case("true")) {
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
    users: web::Data<PostgresUserRepository>,
    sessions: web::Data<dyn SessionRepository>,
    password_hasher: web::Data<Argon2PasswordHasher>,
    body: web::Json<SignupRequest>,
) -> Result<HttpResponse, ApiError> {
    // Normalize once at the boundary: lookups are case-insensitive, so the
    // stored email must be too or duplicate detection would miss "Foo@x.com".
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::bad_request("email must not be empty"));
    }
    if body.password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters long",
        ));
    }
    let password = body.password.clone();
    let password_hash = run_hasher(password_hasher.clone(), move |h| h.hash(&password)).await?;
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
    let cookie = issue_session(&sessions, user.id).await?;
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
    users: web::Data<PostgresUserRepository>,
    sessions: web::Data<dyn SessionRepository>,
    password_hasher: web::Data<Argon2PasswordHasher>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, ApiError> {
    let email = body.email.trim().to_lowercase();
    let user = users
        .find_by_email(email)
        .await
        .map_err(repo_error_response)?;
    // "No such user", "wrong password", and "account has no password" all
    // fall through to the same 401: the response must not reveal that a
    // passwordless account (one that can only sign in via an external
    // identity provider) exists.
    let Some(user) = user else {
        return Err(ApiError::unauthorized());
    };
    let Some(password_hash) = user.password_hash.clone() else {
        return Err(ApiError::unauthorized());
    };
    let password = body.password.clone();
    let valid = run_hasher(password_hasher.clone(), move |h| {
        h.verify(&password, &password_hash)
    })
    .await?;
    if !valid {
        return Err(ApiError::unauthorized());
    }
    let cookie = issue_session(&sessions, user.id).await?;
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
    sessions: web::Data<dyn SessionRepository>,
) -> Result<HttpResponse, ApiError> {
    if let Some(cookie) = req.cookie(COOKIE_NAME) {
        let session = sessions
            .find_by_token_hash(hash_token(cookie.value()))
            .await
            .map_err(repo_error_response)?;
        if let Some(session) = session {
            sessions
                .delete(session.id)
                .await
                .map_err(repo_error_response)?;
        }
    }
    Ok(HttpResponse::NoContent().cookie(clear_cookie()).finish())
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
