//! OIDC sign-in HTTP surface: `/api/auth/providers` plus the authorization
//! code flow at `/api/auth/oidc/login` and `/api/auth/oidc/callback`.
//!
//! The browser-facing half of the flow rides on a short-lived state cookie:
//! `login` asks the provider for an authorization URL, stores the one-time
//! {state, nonce, PKCE verifier, redirect target, expiry} in an
//! authenticated-encrypted cookie (the cookie crate's private jar), and 302s
//! the browser to the IdP. `callback` refuses to run without a valid,
//! unexpired copy of that cookie — which is what makes a forged callback URL
//! useless (login CSRF) — then hands the code to the provider, asks
//! [`decide_login`] who is logging in, and ends in the same session-issuing
//! path as password login.
//!
//! Every failure is a browser navigation: a 302 to
//! `{server.web_base_url}/login?error=<code>` with the state cookie
//! cleared. The real reason goes to the server log only; the redirect carries just the
//! stable code, never tokens, codes, or secrets.

use actix_web::cookie::time::Duration as CookieDuration;
use actix_web::cookie::{Cookie, CookieJar, Key, SameSite};
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use application::oidc_login::{
    LoginDecision, LoginPolicy, decide_login, identity_from_claims, new_user_from_claims,
};
use application::ports::{
    OidcClaims, OidcError, OidcProvider, PendingOidcLogin, RepositoryError, SessionRepository,
    UserIdentityRepository, UserRepository,
};
use chrono::{DateTime, Duration, Utc};
use domain::UserId;
use infrastructure::repositories::{PostgresUserIdentityRepository, PostgresUserRepository};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::auth::{self, issue_session};
use crate::config;
use crate::error::ApiError;

/// Name of the short-lived OIDC state cookie.
const STATE_COOKIE: &str = "minerva_oidc_state";

/// How long an unused authorization attempt stays valid.
const STATE_TTL: Duration = Duration::minutes(10);

/// Everything the OIDC routes need, registered as a single `web::Data` only
/// when OIDC is enabled. Handlers take it as `Option<web::Data<OidcAuth>>`:
/// `None` means "OIDC off" (404 on the flow routes, `null` in the providers
/// response).
pub struct OidcAuth {
    pub provider: Arc<dyn OidcProvider>,
    /// Key for the authenticated-encrypted state cookie, derived from
    /// `oidc.state_secret`.
    pub key: Key,
    /// `server.web_base_url` with any trailing slash stripped; every redirect
    /// is built as `{base_url}{path}` where the path starts with '/'.
    pub base_url: String,
    pub policy: LoginPolicy,
}

impl OidcAuth {
    /// Build the shared route config from validated configuration values. The
    /// composition root has already checked that `base_url` is an absolute
    /// http(s) URL without a trailing slash and that `state_secret` is at
    /// least 32 bytes (see the config module), so this cannot fail.
    pub fn new(
        provider: Arc<dyn OidcProvider>,
        base_url: String,
        state_secret: &config::Secret,
        auto_create_users: bool,
    ) -> Self {
        Self {
            provider,
            key: Key::derive_from(state_secret.expose().as_bytes()),
            base_url,
            policy: LoginPolicy { auto_create_users },
        }
    }
}

/// What the login route promises the callback route, carried in the state
/// cookie. The private jar encrypts it, so a client can neither read nor
/// forge it; a tampered value simply fails to decrypt and is treated as absent.
#[derive(Debug, Serialize, Deserialize)]
struct OidcState {
    state: String,
    nonce: String,
    pkce_verifier: String,
    next: String,
    expires_at: DateTime<Utc>,
}

/// Build the Set-Cookie for a fresh state (value encrypted with the private jar).
fn set_state_cookie(
    key: &Key,
    state: &OidcState,
    cookies: &auth::CookieSettings,
) -> Cookie<'static> {
    let mut cookie = Cookie::new(
        STATE_COOKIE,
        serde_json::to_string(state).expect("state is serializable"),
    );
    apply_state_attributes(&mut cookie, cookies);
    let mut jar = CookieJar::new();
    jar.private_mut(key).add(cookie);
    jar.get(STATE_COOKIE)
        .expect("cookie was just added")
        .clone()
}

/// Decrypt and check a raw state cookie. `None` when the cookie is missing,
/// tampered with, unparsable, or expired — all of which the callback treats
/// the same.
fn read_state(raw: Option<Cookie<'static>>, key: &Key) -> Option<OidcState> {
    let mut jar = CookieJar::new();
    jar.add_original(raw?);
    let cookie = jar.private(key).get(STATE_COOKIE)?;
    let state: OidcState = serde_json::from_str(cookie.value()).ok()?;
    (state.expires_at > Utc::now()).then_some(state)
}

/// A Set-Cookie that makes the browser drop the state cookie.
fn clear_state_cookie(cookies: &auth::CookieSettings) -> Cookie<'static> {
    let mut cookie = Cookie::new(STATE_COOKIE, "");
    apply_state_attributes(&mut cookie, cookies);
    cookie.make_removal();
    cookie
}

/// Scoped to the OIDC routes (narrower than the session cookie's `/`),
/// HttpOnly so script cannot read it, SameSite=Lax like the session cookie,
/// and `Secure` under the same configuration opt-in as the session cookie.
fn apply_state_attributes(cookie: &mut Cookie<'_>, cookies: &auth::CookieSettings) {
    cookie.set_path("/api/auth/oidc");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(CookieDuration::minutes(10));
    if cookies.secure {
        cookie.set_secure(true);
    }
}

/// The post-login redirect target from `?next=`. It must be a site-relative
/// path: starts with a single '/', no '//' (which browsers may read as a
/// scheme-relative URL), and no backslash (an open-redirect smuggling trick).
/// A leading '/' already rules out any scheme, so anything else is dropped
/// for "/".
fn validated_next(next: &str) -> &str {
    if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') {
        next
    } else {
        "/"
    }
}

/// Every OIDC failure is a browser navigation back to the login page with a
/// stable error code, plus a cleared state cookie. The real reason goes to
/// the log only — never into the redirect (no tokens, codes, or secrets).
fn fail_redirect(
    auth: &OidcAuth,
    cookies: &auth::CookieSettings,
    code: &str,
    reason: &str,
) -> HttpResponse {
    eprintln!("oidc login failed ({code}): {reason}");
    HttpResponse::Found()
        .insert_header((
            header::LOCATION,
            format!("{}/login?error={}", auth.base_url, code),
        ))
        .cookie(clear_state_cookie(cookies))
        .finish()
}

/// Wire shape of `GET /api/auth/providers`.
#[derive(Serialize, ToSchema)]
pub struct AuthProvidersResponse {
    /// Email/password sign-in is always available.
    pub password: bool,
    /// The configured OIDC provider, or null when OIDC is disabled.
    pub oidc: Option<OidcProviderInfo>,
}

/// The OIDC provider's display name, for the login screen.
#[derive(Serialize, ToSchema)]
pub struct OidcProviderInfo {
    /// Provider name as configured on the server.
    pub display_name: String,
}

/// Query params for `GET /api/auth/oidc/login`.
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct OidcLoginQuery {
    /// Site-relative path to return to after sign-in; anything that is not a
    /// plain relative path is ignored and "/" is used.
    pub next: Option<String>,
}

/// Query params for `GET /api/auth/oidc/callback`.
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct OidcCallbackQuery {
    /// Authorization code from the provider.
    pub code: Option<String>,
    /// State value echoed back by the provider.
    pub state: Option<String>,
    /// Set by the provider when the user denies the flow or it fails.
    pub error: Option<String>,
    /// Human-readable detail of the provider error.
    pub error_description: Option<String>,
}

/// Auth Providers
///
/// Which sign-in methods this deployment offers. `oidc` is null when the
/// server was started without OIDC configuration.
#[utoipa::path(
    get,
    path = "/api/auth/providers",
    tags = ["auth"],
    responses((status = 200, description = "The available sign-in methods", body = AuthProvidersResponse))
)]
pub async fn list_auth_providers(oidc: Option<web::Data<OidcAuth>>) -> HttpResponse {
    let response = AuthProvidersResponse {
        password: true,
        oidc: oidc.as_ref().map(|auth| OidcProviderInfo {
            display_name: auth.provider.display_name().to_owned(),
        }),
    };
    HttpResponse::Ok().json(response)
}

/// OIDC Login
///
/// Start an OIDC sign-in: stores a short-lived state cookie and redirects the
/// browser to the provider's authorization page. If the provider cannot be
/// reached, the browser is sent to the login page with error `oidc_unavailable`.
#[utoipa::path(
    get,
    path = "/api/auth/oidc/login",
    tags = ["auth"],
    params(OidcLoginQuery),
    responses(
        (status = 302, description = "Redirect to the provider's authorization page; state cookie set"),
        (status = 404, description = "OIDC is not enabled on this server", body = ApiError)
    )
)]
pub async fn oidc_login(
    query: web::Query<OidcLoginQuery>,
    oidc: Option<web::Data<OidcAuth>>,
    cookies: web::Data<auth::CookieSettings>,
) -> Result<HttpResponse, ApiError> {
    let Some(auth) = oidc else {
        return Err(ApiError::not_found());
    };
    let request = match auth.provider.authorization_request().await {
        Ok(request) => request,
        Err(err) => {
            let code = if matches!(err, OidcError::Unavailable) {
                "oidc_unavailable"
            } else {
                "oidc_login_failed"
            };
            return Ok(fail_redirect(
                auth.get_ref(),
                &cookies,
                code,
                &err.to_string(),
            ));
        }
    };
    let state = OidcState {
        state: request.pending.csrf_state,
        nonce: request.pending.nonce,
        pkce_verifier: request.pending.pkce_verifier,
        next: validated_next(query.next.as_deref().unwrap_or("/")).to_owned(),
        expires_at: Utc::now() + STATE_TTL,
    };
    Ok(HttpResponse::Found()
        .insert_header((header::LOCATION, request.authorization_url))
        .cookie(set_state_cookie(&auth.key, &state, &cookies))
        .finish())
}

/// OIDC Callback
///
/// The provider redirects back here after the user signs in. Requires the
/// state cookie set by the login route — a callback without it is rejected,
/// which is what keeps forged callback URLs from logging anyone in. Success
/// ends in a redirect into the app with a session cookie; every failure ends
/// in a redirect to the login page with an error code.
#[utoipa::path(
    get,
    path = "/api/auth/oidc/callback",
    tags = ["auth"],
    params(OidcCallbackQuery),
    responses(
        (status = 302, description = "Redirect into the app with a session cookie on success, or to the login page with an error code on failure"),
        (status = 404, description = "OIDC is not enabled on this server", body = ApiError)
    )
)]
pub async fn oidc_callback(
    req: HttpRequest,
    query: web::Query<OidcCallbackQuery>,
    oidc: Option<web::Data<OidcAuth>>,
    users: web::Data<PostgresUserRepository>,
    identities: web::Data<PostgresUserIdentityRepository>,
    sessions: web::Data<dyn SessionRepository>,
    cookies: web::Data<auth::CookieSettings>,
) -> Result<HttpResponse, ApiError> {
    let Some(auth_data) = oidc else {
        return Err(ApiError::not_found());
    };
    let auth = auth_data.get_ref();

    // Login CSRF protection: no valid state cookie, no login. Missing,
    // tampered, and expired are indistinguishable on purpose.
    let Some(state) = read_state(req.cookie(STATE_COOKIE), &auth.key) else {
        return Ok(fail_redirect(
            auth,
            &cookies,
            "oidc_login_failed",
            "state cookie missing, invalid, or expired",
        ));
    };

    // The provider reported a failure before we ever got a code.
    if let Some(error) = &query.error {
        let detail = query
            .error_description
            .as_deref()
            .unwrap_or("no description");
        return Ok(fail_redirect(
            auth,
            &cookies,
            "oidc_provider_error",
            &format!("provider error {error}: {detail}"),
        ));
    }

    let (Some(code), Some(returned_state)) = (&query.code, &query.state) else {
        return Ok(fail_redirect(
            auth,
            &cookies,
            "oidc_login_failed",
            "callback is missing the code or state parameter",
        ));
    };

    let pending = PendingOidcLogin {
        csrf_state: state.state,
        nonce: state.nonce,
        pkce_verifier: state.pkce_verifier,
    };
    let claims = match auth
        .provider
        .complete_login(code.clone(), returned_state.clone(), pending)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            let code = if matches!(err, OidcError::Unavailable) {
                "oidc_unavailable"
            } else {
                "oidc_login_failed"
            };
            return Ok(fail_redirect(auth, &cookies, code, &err.to_string()));
        }
    };

    // Who is logging in? The identity lookup always runs; the email lookup
    // only when it could matter (no known identity and a verified, non-blank
    // email claim), mirroring decide_login's rule order.
    let identity = match identities
        .find_by_issuer_and_subject(claims.issuer.clone(), claims.subject.clone())
        .await
    {
        Ok(identity) => identity,
        Err(err) => {
            return Ok(fail_redirect(
                auth,
                &cookies,
                "oidc_login_failed",
                &format!("identity lookup failed: {err}"),
            ));
        }
    };

    let user_with_email = match (&identity, &claims.email) {
        (None, Some(email)) if claims.email_verified => {
            let email = email.trim().to_lowercase();
            if email.is_empty() {
                None
            } else {
                match users.find_by_email(email).await {
                    Ok(user) => user,
                    Err(err) => {
                        return Ok(fail_redirect(
                            auth,
                            &cookies,
                            "oidc_login_failed",
                            &format!("user lookup failed: {err}"),
                        ));
                    }
                }
            }
        }
        _ => None,
    };

    let now = Utc::now();
    let user_id = match decide_login(
        auth.policy,
        &claims,
        identity.as_ref(),
        user_with_email.as_ref(),
    ) {
        LoginDecision::ExistingIdentity { user_id } => user_id,
        LoginDecision::LinkToExistingUser { user_id } => {
            match create_identity(&identities, user_id, &claims, now).await {
                Ok(()) => user_id,
                Err(reason) => {
                    return Ok(fail_redirect(auth, &cookies, "oidc_login_failed", &reason));
                }
            }
        }
        LoginDecision::CreateUser => {
            let user = new_user_from_claims(&claims, now);
            let user = match users.create(user).await {
                Ok(user) => user,
                Err(err) => {
                    return Ok(fail_redirect(
                        auth,
                        &cookies,
                        "oidc_login_failed",
                        &format!("user creation failed: {err}"),
                    ));
                }
            };
            // ponytail: creating the user and its identity is two repository
            // calls with no cross-repo transaction; if the second fails, a
            // passwordless user stays behind and the next login re-links it
            // via its verified email. Belongs in an application-layer unit of
            // work when one exists.
            match create_identity(&identities, user.id, &claims, now).await {
                Ok(()) => user.id,
                Err(reason) => {
                    return Ok(fail_redirect(auth, &cookies, "oidc_login_failed", &reason));
                }
            }
        }
        LoginDecision::Reject(rejection) => {
            return Ok(fail_redirect(
                auth,
                &cookies,
                rejection.code(),
                rejection.code(),
            ));
        }
    };

    let cookie = match issue_session(&sessions, user_id, &cookies).await {
        Ok(cookie) => cookie,
        Err(err) => {
            return Ok(fail_redirect(
                auth,
                &cookies,
                "oidc_login_failed",
                &format!("session creation failed: {err}"),
            ));
        }
    };
    Ok(HttpResponse::Found()
        .insert_header((header::LOCATION, format!("{}{}", auth.base_url, state.next)))
        .cookie(cookie)
        .cookie(clear_state_cookie(&cookies))
        .finish())
}

/// Create the (issuer, subject) -> user link. A `Conflict` means a concurrent
/// callback for the same provider identity won the race; re-look it up and
/// carry on.
async fn create_identity(
    identities: &PostgresUserIdentityRepository,
    user_id: UserId,
    claims: &OidcClaims,
    now: DateTime<Utc>,
) -> Result<(), String> {
    match identities
        .create(identity_from_claims(user_id, claims, now))
        .await
    {
        Ok(_identity) => Ok(()),
        Err(RepositoryError::Conflict(_)) => {
            match identities
                .find_by_issuer_and_subject(claims.issuer.clone(), claims.subject.clone())
                .await
            {
                Ok(Some(_identity)) => Ok(()),
                _ => Err("identity creation raced and the re-lookup failed".to_owned()),
            }
        }
        Err(err) => Err(format!("identity creation failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::Service;
    use actix_web::http::StatusCode;
    use actix_web::test::{TestRequest, init_service};
    use application::ports::OidcAuthRequest;
    use domain::User;
    use infrastructure::db::PgPool;
    use infrastructure::repositories::PostgresSessionRepository;
    use uuid::Uuid;

    // ---- pure unit tests (no DB) ----

    fn test_key(seed: u8) -> Key {
        Key::derive_from(&[seed; 32])
    }

    fn test_cookies() -> auth::CookieSettings {
        auth::CookieSettings { secure: false }
    }

    fn state(next: &str, expires_at: DateTime<Utc>) -> OidcState {
        OidcState {
            state: "csrf-1".into(),
            nonce: "nonce-1".into(),
            pkce_verifier: "verifier-1".into(),
            next: next.into(),
            expires_at,
        }
    }

    fn state_cookie_header(value: &str) -> Option<Cookie<'static>> {
        Some(Cookie::new(STATE_COOKIE, value.to_owned()))
    }

    #[test]
    fn validated_next_accepts_site_relative_paths() {
        assert_eq!(validated_next("/"), "/");
        assert_eq!(validated_next("/dashboard"), "/dashboard");
        assert_eq!(validated_next("/goals/1?tab=plan"), "/goals/1?tab=plan");
    }

    #[test]
    fn validated_next_rejects_open_redirects() {
        for bad in [
            "",
            "//evil.com",
            "https://evil.com",
            "http://evil.com",
            "/\\evil",
            "evil.com",
        ] {
            assert_eq!(validated_next(bad), "/", "{bad:?} should be rejected");
        }
    }

    #[test]
    fn state_cookie_round_trips() {
        let key = test_key(7);
        let expected = state("/dashboard", Utc::now() + STATE_TTL);
        let cookie = set_state_cookie(&key, &expected, &test_cookies());
        let loaded =
            read_state(state_cookie_header(cookie.value()), &key).expect("state should round-trip");
        assert_eq!(loaded.state, "csrf-1");
        assert_eq!(loaded.nonce, "nonce-1");
        assert_eq!(loaded.pkce_verifier, "verifier-1");
        assert_eq!(loaded.next, "/dashboard");
    }

    #[test]
    fn state_cookie_is_rejected_when_tampered_or_wrong_key() {
        let key = test_key(7);
        let value = set_state_cookie(&key, &state("/", Utc::now() + STATE_TTL), &test_cookies())
            .value()
            .to_owned();

        // A different key cannot decrypt the cookie.
        assert!(read_state(state_cookie_header(&value), &test_key(8)).is_none());

        // Flipping one character breaks the authentication tag.
        let mut tampered = value.clone();
        let idx = tampered.len() / 2;
        tampered.replace_range(
            idx..idx + 1,
            if tampered.as_bytes()[idx] == b'A' {
                "B"
            } else {
                "A"
            },
        );
        assert!(read_state(state_cookie_header(&tampered), &key).is_none());
    }

    #[test]
    fn expired_or_missing_state_cookie_is_rejected() {
        let key = test_key(7);
        let expired = state("/", Utc::now() - Duration::seconds(1));
        assert!(
            read_state(
                state_cookie_header(set_state_cookie(&key, &expired, &test_cookies()).value()),
                &key
            )
            .is_none()
        );
        assert!(read_state(None, &key).is_none());
    }

    #[test]
    fn state_cookie_carries_the_expected_attributes() {
        let cookie = set_state_cookie(
            &test_key(7),
            &state("/", Utc::now() + STATE_TTL),
            &test_cookies(),
        );
        assert_eq!(cookie.name(), STATE_COOKIE);
        assert_eq!(cookie.path().unwrap(), "/api/auth/oidc");
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(CookieDuration::minutes(10)));
    }

    // ---- handler tests (need a real Postgres; skipped without DATABASE_URL) ----

    struct FakeProvider {
        claims: OidcClaims,
    }

    #[async_trait::async_trait]
    impl OidcProvider for FakeProvider {
        fn display_name(&self) -> &str {
            "Fake IdP"
        }

        async fn authorization_request(&self) -> Result<OidcAuthRequest, OidcError> {
            Ok(OidcAuthRequest {
                authorization_url: "https://idp.example/authorize?client_id=minerva".into(),
                pending: PendingOidcLogin {
                    csrf_state: "csrf-1".into(),
                    nonce: "nonce-1".into(),
                    pkce_verifier: "verifier-1".into(),
                },
            })
        }

        async fn complete_login(
            &self,
            _code: String,
            returned_state: String,
            pending: PendingOidcLogin,
        ) -> Result<OidcClaims, OidcError> {
            if returned_state != pending.csrf_state {
                return Err(OidcError::StateMismatch);
            }
            Ok(self.claims.clone())
        }
    }

    fn claims(subject: &str, email: Option<&str>, verified: bool) -> OidcClaims {
        OidcClaims {
            issuer: "https://idp.example".into(),
            subject: subject.into(),
            email: email.map(str::to_owned),
            email_verified: verified,
            display_name: None,
            groups: Vec::new(),
        }
    }

    fn test_auth(provider: FakeProvider, auto_create_users: bool) -> OidcAuth {
        OidcAuth {
            provider: Arc::new(provider),
            key: test_key(7),
            base_url: "http://localhost:9999".to_owned(),
            policy: LoginPolicy { auto_create_users },
        }
    }

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip OIDC tests in CI");
        }
        url
    }

    /// A one-connection pool: the default settings (max_size 10, min_idle =
    /// max_size) times the ~14 parallel handler-test pools would exceed local
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

    /// Build the test app (real Postgres repositories, optional OIDC) and
    /// initialize it. A macro because `init_service`'s service type is opaque
    /// and cannot be named in a helper's signature.
    macro_rules! test_app {
        ($oidc:expr, $url:expr) => {{
            let pool = test_pool($url);
            let oidc: Option<OidcAuth> = $oidc;
            // The handler takes the trait object, so register it as one —
            // `Data<PostgresSessionRepository>` would not satisfy the
            // extractor (a missing required Data is a 500).
            let session_repo: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool.clone()));
            let sessions: web::Data<dyn SessionRepository> = session_repo.into();
            let app = App::new()
                .app_data(web::Data::new(PostgresUserRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresUserIdentityRepository::new(pool)))
                .app_data(sessions)
                .app_data(web::Data::new(auth::CookieSettings { secure: false }));
            let app = match oidc {
                Some(auth) => app.app_data(web::Data::new(auth)),
                None => app,
            };
            init_service(
                app.route("/api/auth/providers", web::get().to(list_auth_providers))
                    .route("/api/auth/oidc/login", web::get().to(oidc_login))
                    .route("/api/auth/oidc/callback", web::get().to(oidc_callback)),
            )
            .await
        }};
    }

    /// The full value of the first `Set-Cookie` for `name`, if any.
    fn set_cookie(res: &HttpResponse, name: &str) -> Option<String> {
        res.headers()
            .get_all(header::SET_COOKIE)
            .filter_map(|value| value.to_str().ok())
            .find(|set| set.starts_with(&format!("{name}=")))
            .map(str::to_owned)
    }

    /// GET the login route, then the callback carrying the state cookie it
    /// set. Returns both responses.
    macro_rules! login_and_callback {
        ($app:expr, $next:expr) => {{
            let next: Option<&str> = $next;
            let mut uri = String::from("/api/auth/oidc/login");
            if let Some(next) = next {
                uri.push_str(&format!("?next={next}"));
            }
            let login = $app
                .call(TestRequest::get().uri(&uri).to_request())
                .await
                .unwrap()
                .into_parts()
                .1;
            let state_cookie = set_cookie(&login, STATE_COOKIE)
                .expect("login sets the state cookie")
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("minerva_oidc_state=")
                .to_owned();
            let callback = $app
                .call(
                    TestRequest::get()
                        .uri("/api/auth/oidc/callback?code=abc&state=csrf-1")
                        .insert_header((
                            header::COOKIE,
                            format!("minerva_oidc_state={state_cookie}"),
                        ))
                        .to_request(),
                )
                .await
                .unwrap()
                .into_parts()
                .1;
            (login, callback)
        }};
    }

    fn redirect_location(res: &HttpResponse) -> String {
        res.headers()
            .get(header::LOCATION)
            .expect("redirect has a Location")
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[actix_web::test]
    async fn providers_reports_oidc_null_when_disabled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(None, &url);
        let res = app
            .call(TestRequest::get().uri("/api/auth/providers").to_request())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = actix_web::test::read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "password": true, "oidc": null }));
    }

    #[actix_web::test]
    async fn providers_reports_the_oidc_provider_when_enabled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        let res = app
            .call(TestRequest::get().uri("/api/auth/providers").to_request())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = actix_web::test::read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "password": true, "oidc": { "display_name": "Fake IdP" } })
        );
    }

    #[actix_web::test]
    async fn flow_routes_404_when_oidc_is_disabled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(None, &url);
        for uri in [
            "/api/auth/oidc/login",
            "/api/auth/oidc/callback?code=x&state=y",
        ] {
            let res = app
                .call(TestRequest::get().uri(uri).to_request())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
            let body = actix_web::test::read_body(res).await;
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["error"]["code"], "not_found", "{uri}");
        }
    }

    #[actix_web::test]
    async fn login_redirects_to_the_provider_and_sets_a_valid_state_cookie() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        let res = app
            .call(
                TestRequest::get()
                    .uri("/api/auth/oidc/login?next=/dashboard")
                    .to_request(),
            )
            .await
            .unwrap()
            .into_parts()
            .1;
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&res),
            "https://idp.example/authorize?client_id=minerva"
        );

        // The cookie the browser would store decrypts to the promise.
        let set = set_cookie(&res, STATE_COOKIE).expect("state cookie set");
        assert!(set.contains("HttpOnly"));
        assert!(set.contains("Path=/api/auth/oidc"));
        assert!(set.contains("Max-Age=600"));
        let value = set
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("minerva_oidc_state=");
        let loaded =
            read_state(state_cookie_header(value), &test_key(7)).expect("state cookie decrypts");
        assert_eq!(loaded.next, "/dashboard");
    }

    #[actix_web::test]
    async fn login_ignores_a_next_that_is_not_site_relative() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        for next in ["//evil.com", "https://evil.com", "/\\evil"] {
            let res = app
                .call(
                    TestRequest::get()
                        .uri(&format!("/api/auth/oidc/login?next={next}"))
                        .to_request(),
                )
                .await
                .unwrap()
                .into_parts()
                .1;
            let set = set_cookie(&res, STATE_COOKIE).expect("state cookie set");
            let value = set
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("minerva_oidc_state=");
            let loaded = read_state(state_cookie_header(value), &test_key(7))
                .expect("state cookie decrypts");
            assert_eq!(loaded.next, "/", "next {next:?} should be dropped");
        }
    }

    #[actix_web::test]
    async fn callback_creates_user_identity_and_session() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let subject = format!("sub-{}", Uuid::new_v4());
        let email = format!("oidc-{}@example.com", Uuid::new_v4());
        let auth = test_auth(
            FakeProvider {
                claims: claims(&subject, Some(&email), true),
            },
            true,
        );

        let app = test_app!(Some(auth), &url);
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(redirect_location(&callback), "http://localhost:9999/");
        assert!(
            set_cookie(&callback, "minerva_session").is_some(),
            "session cookie set"
        );
        let cleared = set_cookie(&callback, STATE_COOKIE).expect("state cookie cleared");
        assert!(
            cleared.contains("Max-Age=0"),
            "state cookie removal: {cleared}"
        );

        // The user and its identity actually landed in the database.
        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let identities = PostgresUserIdentityRepository::new(pool);
        let user = users
            .find_by_email(email.clone())
            .await
            .unwrap()
            .expect("user created");
        assert_eq!(user.password_hash, None, "SSO users are passwordless");
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), subject)
            .await
            .unwrap()
            .expect("identity created");
        assert_eq!(identity.user_id, user.id);
    }

    #[actix_web::test]
    async fn second_login_with_the_same_identity_reuses_the_user() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let subject = format!("sub-{}", Uuid::new_v4());
        let email = format!("oidc-{}@example.com", Uuid::new_v4());
        let auth = test_auth(
            FakeProvider {
                claims: claims(&subject, Some(&email), true),
            },
            true,
        );

        let app = test_app!(Some(auth), &url);
        let (_login, first) = login_and_callback!(app, None);
        assert_eq!(first.status(), StatusCode::FOUND);
        assert_eq!(redirect_location(&first), "http://localhost:9999/");

        // Same provider identity again: no new user, same redirect.
        let (_login, second) = login_and_callback!(app, None);
        assert_eq!(second.status(), StatusCode::FOUND);
        assert_eq!(redirect_location(&second), "http://localhost:9999/");

        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let identities = PostgresUserIdentityRepository::new(pool);
        let user = users
            .find_by_email(email)
            .await
            .unwrap()
            .expect("user exists");
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), subject)
            .await
            .unwrap()
            .expect("identity exists");
        assert_eq!(identity.user_id, user.id);
    }

    #[actix_web::test]
    async fn callback_links_to_an_existing_password_user() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let subject = format!("sub-{}", Uuid::new_v4());
        let email = format!("password-{}@example.com", Uuid::new_v4());

        // A password account that already exists for the claimed email.
        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let user = User {
            id: domain::UserId::new(),
            email: email.clone(),
            password_hash: Some("not-a-real-hash".into()),
            display_name: "Existing".into(),
            created_at: now,
            updated_at: now,
        };
        let existing = users.create(user).await.unwrap();

        let auth = test_auth(
            FakeProvider {
                claims: claims(&subject, Some(&email), true),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(redirect_location(&callback), "http://localhost:9999/");

        // The identity linked to the pre-existing user, which is untouched.
        let identities = PostgresUserIdentityRepository::new(pool);
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), subject)
            .await
            .unwrap()
            .expect("identity linked");
        assert_eq!(identity.user_id, existing.id);
        let user = users.find_by_email(email).await.unwrap().unwrap();
        assert_eq!(user.password_hash.as_deref(), Some("not-a-real-hash"));
    }

    #[actix_web::test]
    async fn unverified_email_is_rejected_with_its_code() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let email = format!("oidc-{}@example.com", Uuid::new_v4());
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-unverified", Some(&email), false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&callback),
            "http://localhost:9999/login?error=oidc_email_not_verified"
        );
    }

    #[actix_web::test]
    async fn missing_email_is_rejected_with_its_code() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-no-email", None, true),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&callback),
            "http://localhost:9999/login?error=oidc_email_missing"
        );
    }

    #[actix_web::test]
    async fn unknown_user_is_rejected_when_signup_is_disabled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let email = format!("oidc-{}@example.com", Uuid::new_v4());
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-nosignup", Some(&email), true),
            },
            false,
        );
        let app = test_app!(Some(auth), &url);
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&callback),
            "http://localhost:9999/login?error=oidc_signup_disabled"
        );
    }

    #[actix_web::test]
    async fn callback_without_a_state_cookie_fails() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        // A forged callback URL: code and state look right, but there is
        // no state cookie behind them.
        let res = app
            .call(
                TestRequest::get()
                    .uri("/api/auth/oidc/callback?code=abc&state=csrf-1")
                    .to_request(),
            )
            .await
            .unwrap()
            .into_parts()
            .1;
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&res),
            "http://localhost:9999/login?error=oidc_login_failed"
        );
    }

    #[actix_web::test]
    async fn callback_with_a_mismatched_state_fails() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        // Valid cookie, but the provider echoed a different state back.
        let login = app
            .call(TestRequest::get().uri("/api/auth/oidc/login").to_request())
            .await
            .unwrap()
            .into_parts()
            .1;
        let state_cookie = set_cookie(&login, STATE_COOKIE)
            .expect("state cookie set")
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("minerva_oidc_state=")
            .to_owned();
        let res = app
            .call(
                TestRequest::get()
                    .uri("/api/auth/oidc/callback?code=abc&state=wrong")
                    .insert_header((header::COOKIE, format!("{STATE_COOKIE}={state_cookie}")))
                    .to_request(),
            )
            .await
            .unwrap()
            .into_parts()
            .1;
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&res),
            "http://localhost:9999/login?error=oidc_login_failed"
        );
    }

    #[actix_web::test]
    async fn provider_reported_error_redirects_with_its_code() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let auth = test_auth(
            FakeProvider {
                claims: claims("sub-1", None, false),
            },
            true,
        );
        let app = test_app!(Some(auth), &url);
        // The user denied the flow at the IdP: it redirects back with
        // ?error= and no code.
        let login = app
            .call(TestRequest::get().uri("/api/auth/oidc/login").to_request())
            .await
            .unwrap()
            .into_parts()
            .1;
        let state_cookie = set_cookie(&login, STATE_COOKIE)
            .expect("state cookie set")
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("minerva_oidc_state=")
            .to_owned();
        let res = app
            .call(
                TestRequest::get()
                    .uri("/api/auth/oidc/callback?error=access_denied&error_description=User%20denied")
                    .insert_header((header::COOKIE, format!("{STATE_COOKIE}={state_cookie}")))
                    .to_request(),
            )
            .await
            .unwrap()
            .into_parts()
            .1;
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_location(&res),
            "http://localhost:9999/login?error=oidc_provider_error"
        );
    }
}
