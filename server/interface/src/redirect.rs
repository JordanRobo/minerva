//! Generic redirect-login HTTP surface: `/api/auth/{provider}/login` and
//! `/api/auth/{provider}/callback`, for any registered [`RedirectProvider`].
//!
//! The browser-facing half of the flow rides on a short-lived state cookie:
//! `login` asks the provider for an authorization URL, stores the provider's
//! opaque pending state, the validated redirect target and an expiry in an
//! authenticated-encrypted cookie (the cookie crate's private jar), and 302s
//! the browser away. `callback` refuses to run without a valid, unexpired
//! copy of that cookie whose provider matches the route — which is what makes
//! a forged callback URL useless (login CSRF) — then hands every query
//! parameter to the provider's `complete`, issues a session and ends in the
//! same redirect path as password login.
//!
//! Every failure is a browser navigation: a 302 to
//! `{server.web_base_url}/login?error=<code>` with the state cookie cleared.
//! The real reason goes to the server log only; the redirect carries just the
//! stable code, never tokens, codes, or secrets.

use actix_web::cookie::time::Duration as CookieDuration;
use actix_web::cookie::{Cookie, CookieJar, Key, SameSite};
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use application::auth::SessionService;
use application::auth::provider::{AuthError, AuthProviders, CallbackParams, PendingLogin};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};

use crate::auth::{self, issue_session};
use crate::error::ApiError;

/// Name of the short-lived redirect-login state cookie.
const STATE_COOKIE: &str = "minerva_auth_state";

/// How long an unused authorization attempt stays valid.
const STATE_TTL: Duration = Duration::minutes(10);

/// Everything the redirect routes need, registered as a single `web::Data`
/// when at least one redirect provider exists. Handlers take it as
/// `Option<web::Data<RedirectFlow>>`: `None` means "no redirect providers"
/// (404 on the flow routes).
pub struct RedirectFlow {
    /// Key for the authenticated-encrypted state cookie, derived from
    /// `oidc.state_secret`.
    pub key: Key,
    /// `server.web_base_url` with any trailing slash stripped; every redirect
    /// is built as `{base_url}{path}` where the path starts with '/'.
    pub base_url: String,
}

/// What the login route promises the callback route, carried in the state
/// cookie. The private jar encrypts it, so a client can neither read nor
/// forge it; a tampered value simply fails to decrypt and is treated as absent.
#[derive(Debug, Serialize, Deserialize)]
struct RedirectState {
    /// The provider this attempt was started for; the callback route must
    /// match it, so a cookie from one flow cannot feed another.
    provider: String,
    /// The provider's opaque pending state, verbatim.
    pending: BTreeMap<String, String>,
    next: String,
    expires_at: DateTime<Utc>,
}

/// Build the Set-Cookie for a fresh state (value encrypted with the private jar).
fn set_state_cookie(
    key: &Key,
    state: &RedirectState,
    provider: &str,
    cookies: &auth::CookieSettings,
) -> Cookie<'static> {
    let mut cookie = Cookie::new(
        STATE_COOKIE,
        serde_json::to_string(state).expect("state is serializable"),
    );
    apply_state_attributes(&mut cookie, provider, cookies);
    let mut jar = CookieJar::new();
    jar.private_mut(key).add(cookie);
    jar.get(STATE_COOKIE)
        .expect("cookie was just added")
        .clone()
}

/// Decrypt and check a raw state cookie. `None` when the cookie is missing,
/// tampered with, unparsable, or expired — all of which the callback treats
/// the same.
fn read_state(raw: Option<Cookie<'static>>, key: &Key) -> Option<RedirectState> {
    let mut jar = CookieJar::new();
    jar.add_original(raw?);
    let cookie = jar.private(key).get(STATE_COOKIE)?;
    let state: RedirectState = serde_json::from_str(cookie.value()).ok()?;
    (state.expires_at > Utc::now()).then_some(state)
}

/// A Set-Cookie that makes the browser drop the state cookie.
fn clear_state_cookie(provider: &str, cookies: &auth::CookieSettings) -> Cookie<'static> {
    let mut cookie = Cookie::new(STATE_COOKIE, "");
    apply_state_attributes(&mut cookie, provider, cookies);
    cookie.make_removal();
    cookie
}

/// Scoped to the provider's routes (narrower than the session cookie's `/`),
/// HttpOnly so script cannot read it, SameSite=Lax like the session cookie,
/// and `Secure` under the same configuration opt-in as the session cookie.
fn apply_state_attributes(cookie: &mut Cookie<'_>, provider: &str, cookies: &auth::CookieSettings) {
    cookie.set_path(format!("/api/auth/{provider}"));
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

/// The stable code a failed login redirects with: the one carried by the
/// provider's rejection or failure, or `login_failed` for interface-level
/// failures (a missing state cookie, a provider mismatch, an internal error)
/// that no provider named.
fn error_code(error: &AuthError) -> &'static str {
    match error {
        &AuthError::Rejected { code } | &AuthError::Failed { code, .. } => code,
        AuthError::InvalidCredentials | AuthError::Internal(_) => "login_failed",
    }
}

/// Every redirect-login failure is a browser navigation back to the login
/// page with a stable error code, plus a cleared state cookie. The real
/// reason goes to the log only — never into the redirect (no tokens, codes,
/// or secrets).
fn fail_redirect(
    flow: &RedirectFlow,
    provider: &str,
    cookies: &auth::CookieSettings,
    code: &str,
    reason: &str,
) -> HttpResponse {
    eprintln!("login via {provider} failed ({code}): {reason}");
    HttpResponse::Found()
        .insert_header((
            header::LOCATION,
            format!("{}/login?error={}", flow.base_url, code),
        ))
        .cookie(clear_state_cookie(provider, cookies))
        .finish()
}

/// Path parameter for the redirect routes.
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ProviderPath {
    /// The id of a registered redirect provider (e.g. `oidc`).
    pub provider: String,
}

/// Query params for `GET /api/auth/{provider}/login`.
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct RedirectLoginQuery {
    /// Site-relative path to return to after sign-in; anything that is not a
    /// plain relative path is ignored and "/" is used.
    pub next: Option<String>,
}

/// Redirect Login
///
/// Start a redirect sign-in (e.g. OIDC): stores a short-lived state cookie
/// and redirects the browser to the provider's authorization page. If the
/// provider cannot be reached, the browser is sent to the login page with an
/// error code.
#[utoipa::path(
    get,
    path = "/api/auth/{provider}/login",
    tags = ["auth"],
    params(ProviderPath, RedirectLoginQuery),
    responses(
        (status = 302, description = "Redirect to the provider's authorization page; state cookie set"),
        (status = 404, description = "No redirect provider is registered under this id", body = ApiError)
    )
)]
pub async fn redirect_login(
    path: web::Path<ProviderPath>,
    query: web::Query<RedirectLoginQuery>,
    flow: Option<web::Data<RedirectFlow>>,
    providers: web::Data<AuthProviders>,
    cookies: web::Data<auth::CookieSettings>,
) -> Result<HttpResponse, ApiError> {
    let Some(flow_data) = flow else {
        return Err(ApiError::not_found());
    };
    let provider = match providers.get_ref().redirect(&path.provider) {
        Some(provider) => provider,
        None => return Err(ApiError::not_found()),
    };
    let start = match provider.begin().await {
        Ok(start) => start,
        Err(error) => {
            return Ok(fail_redirect(
                flow_data.get_ref(),
                &path.provider,
                &cookies,
                error_code(&error),
                &error.to_string(),
            ));
        }
    };
    let state = RedirectState {
        provider: path.provider.clone(),
        pending: start.pending.0,
        next: validated_next(query.next.as_deref().unwrap_or("/")).to_owned(),
        expires_at: Utc::now() + STATE_TTL,
    };
    Ok(HttpResponse::Found()
        .insert_header((header::LOCATION, start.redirect_url))
        .cookie(set_state_cookie(
            &flow_data.key,
            &state,
            &path.provider,
            &cookies,
        ))
        .finish())
}

/// Redirect Callback
///
/// The provider redirects back here after the user signs in. Requires the
/// state cookie set by the login route — a callback without it (or with one
/// issued for a different provider) is rejected, which is what keeps forged
/// callback URLs from logging anyone in. Success ends in a redirect into the
/// app with a session cookie; every failure ends in a redirect to the login
/// page with an error code.
#[utoipa::path(
    get,
    path = "/api/auth/{provider}/callback",
    tags = ["auth"],
    params(ProviderPath),
    responses(
        (status = 302, description = "Redirect into the app with a session cookie on success, or to the login page with an error code on failure"),
        (status = 404, description = "No redirect provider is registered under this id", body = ApiError)
    )
)]
pub async fn redirect_callback(
    req: HttpRequest,
    path: web::Path<ProviderPath>,
    flow: Option<web::Data<RedirectFlow>>,
    providers: web::Data<AuthProviders>,
    session_service: web::Data<SessionService>,
    cookies: web::Data<auth::CookieSettings>,
) -> Result<HttpResponse, ApiError> {
    let Some(flow_data) = flow else {
        return Err(ApiError::not_found());
    };
    let flow = flow_data.get_ref();
    let provider = match providers.get_ref().redirect(&path.provider) {
        Some(provider) => provider,
        None => return Err(ApiError::not_found()),
    };

    // Login CSRF protection: no valid state cookie, no login. Missing,
    // tampered, and expired are indistinguishable on purpose.
    let Some(state) = read_state(req.cookie(STATE_COOKIE), &flow.key) else {
        return Ok(fail_redirect(
            flow,
            &path.provider,
            &cookies,
            "login_failed",
            "state cookie missing, invalid, or expired",
        ));
    };

    // A state cookie is scoped to the provider it was issued for; one from
    // another flow must not feed this route.
    if state.provider != path.provider {
        return Ok(fail_redirect(
            flow,
            &path.provider,
            &cookies,
            "login_failed",
            &format!("state cookie was issued for provider {}", state.provider),
        ));
    }

    // Every query parameter goes to the provider; it knows which ones its
    // flow uses (code, state, error, ...).
    let params: CallbackParams = url::form_urlencoded::parse(req.query_string().as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    let user = match provider.complete(params, PendingLogin(state.pending)).await {
        Ok(user) => user,
        Err(error) => {
            return Ok(fail_redirect(
                flow,
                &path.provider,
                &cookies,
                error_code(&error),
                &error.to_string(),
            ));
        }
    };

    let cookie = match issue_session(session_service.get_ref(), user.id, &cookies).await {
        Ok(cookie) => cookie,
        Err(error) => {
            return Ok(fail_redirect(
                flow,
                &path.provider,
                &cookies,
                "login_failed",
                &format!("session creation failed: {error}"),
            ));
        }
    };
    Ok(HttpResponse::Found()
        .insert_header((header::LOCATION, format!("{}{}", flow.base_url, state.next)))
        .cookie(cookie)
        .cookie(clear_state_cookie(&path.provider, &cookies))
        .finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::Service;
    use actix_web::http::StatusCode;
    use actix_web::test::{TestRequest, init_service};
    use application::account_links::AccountLinkService;
    use application::auth::oidc::OidcAuthProvider;
    use application::auth::password::PasswordAuthProvider;
    use application::auth::provider::{AuthProvider, RedirectProvider, RedirectStart};
    use application::oidc_login::LoginPolicy;
    use application::ports::{
        AccountTokenRepository, OidcAuthRequest, OidcClaims, OidcError, OidcProvider,
        PendingOidcLogin, RepositoryError, SessionRepository, SessionTokens,
        SsoGroupRuleRepository, UserIdentityRepository, UserRepository,
    };
    use application::sso_roles::SsoRoleService;
    use domain::{
        AccountToken, AccountTokenId, AccountTokenKind, Role, SsoGroupRule, SsoGroupRuleId, User,
    };
    use infrastructure::db::PgPool;
    use infrastructure::repositories::{
        PostgresAccountTokenRepository, PostgresSessionRepository, PostgresUserIdentityRepository,
        PostgresUserRepository,
    };
    use infrastructure::{Argon2PasswordHasher, NoEmailSender, Sha256SessionTokens};
    use std::sync::Arc;
    use uuid::Uuid;

    // ---- pure unit tests (no DB) ----

    fn test_key(seed: u8) -> Key {
        Key::derive_from(&[seed; 32])
    }

    fn test_cookies() -> auth::CookieSettings {
        auth::CookieSettings { secure: false }
    }

    fn state(provider: &str, next: &str, expires_at: DateTime<Utc>) -> RedirectState {
        let mut pending = BTreeMap::new();
        pending.insert("csrf_state".to_owned(), "csrf-1".to_owned());
        pending.insert("nonce".to_owned(), "nonce-1".to_owned());
        pending.insert("pkce_verifier".to_owned(), "verifier-1".to_owned());
        RedirectState {
            provider: provider.to_owned(),
            pending,
            next: next.to_owned(),
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
        let expected = state("oidc", "/dashboard", Utc::now() + STATE_TTL);
        let cookie = set_state_cookie(&key, &expected, "oidc", &test_cookies());
        let loaded =
            read_state(state_cookie_header(cookie.value()), &key).expect("state should round-trip");
        assert_eq!(loaded.provider, "oidc");
        assert_eq!(loaded.pending.get("csrf_state").unwrap(), "csrf-1");
        assert_eq!(loaded.pending.get("nonce").unwrap(), "nonce-1");
        assert_eq!(loaded.pending.get("pkce_verifier").unwrap(), "verifier-1");
        assert_eq!(loaded.next, "/dashboard");
    }

    #[test]
    fn state_cookie_is_rejected_when_tampered_or_wrong_key() {
        let key = test_key(7);
        let value = set_state_cookie(
            &key,
            &state("oidc", "/", Utc::now() + STATE_TTL),
            "oidc",
            &test_cookies(),
        )
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
        let expired = state("oidc", "/", Utc::now() - Duration::seconds(1));
        assert!(
            read_state(
                state_cookie_header(
                    set_state_cookie(&key, &expired, "oidc", &test_cookies()).value()
                ),
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
            &state("oidc", "/", Utc::now() + STATE_TTL),
            "oidc",
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

    /// A second, non-OIDC [`RedirectProvider`]: proves the routes dispatch by
    /// registered id rather than hardcoding OIDC. It never completes a flow.
    struct FakeRedirect;

    impl AuthProvider for FakeRedirect {
        fn id(&self) -> &str {
            "fake"
        }

        fn display_name(&self) -> &str {
            "Fake redirect"
        }
    }

    #[async_trait::async_trait]
    impl RedirectProvider for FakeRedirect {
        async fn begin(&self) -> Result<RedirectStart, AuthError> {
            Ok(RedirectStart {
                redirect_url: "https://fake.example/authorize".to_owned(),
                pending: PendingLogin(BTreeMap::from([(
                    "csrf_state".to_owned(),
                    "fake-csrf-1".to_owned(),
                )])),
            })
        }

        async fn complete(
            &self,
            _callback: CallbackParams,
            _pending: PendingLogin,
        ) -> Result<User, AuthError> {
            Err(AuthError::Internal(
                "test fake does not complete flows".to_owned(),
            ))
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

    /// The shared flow config the test app registers (same key and base URL
    /// as before the abstraction, so cookie assertions keep their values).
    fn test_flow() -> web::Data<RedirectFlow> {
        web::Data::new(RedirectFlow {
            key: test_key(7),
            base_url: "http://localhost:9999".to_owned(),
        })
    }

    /// An empty group-rules table for the handler tests. They verify HTTP
    /// wiring, not the D15 mapping (the application-layer tests cover it),
    /// and an in-memory table keeps them independent of the rows the
    /// sso_rules tests create in the shared database while they run.
    struct NoGroupRules;

    #[async_trait::async_trait]
    impl SsoGroupRuleRepository for NoGroupRules {
        async fn list(&self) -> Result<Vec<SsoGroupRule>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn find_by_id(
            &self,
            _id: SsoGroupRuleId,
        ) -> Result<Option<SsoGroupRule>, RepositoryError> {
            Ok(None)
        }

        async fn create(&self, _rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
            unimplemented!("the redirect tests never manage rules")
        }

        async fn update(&self, _rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
            unimplemented!("the redirect tests never manage rules")
        }

        async fn delete(&self, _id: SsoGroupRuleId) -> Result<(), RepositoryError> {
            unimplemented!("the redirect tests never manage rules")
        }

        async fn any_exist(&self) -> Result<bool, RepositoryError> {
            Ok(false)
        }
    }

    /// A Postgres-backed OIDC redirect provider over the given claims.
    fn oidc_redirect(
        url: &str,
        claims: OidcClaims,
        auto_create_users: bool,
    ) -> Arc<dyn RedirectProvider> {
        let pool = test_pool(url);
        let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
        let identities: Arc<dyn UserIdentityRepository> =
            Arc::new(PostgresUserIdentityRepository::new(pool.clone()));
        // The provider consumes pending invites when an invited email signs
        // in via SSO before accepting one.
        let invites: Arc<dyn AccountTokenRepository> =
            Arc::new(PostgresAccountTokenRepository::new(pool));
        Arc::new(OidcAuthProvider::new(
            Arc::new(FakeProvider { claims }),
            users.clone(),
            identities,
            invites,
            SsoRoleService::new(Arc::new(NoGroupRules), users),
            LoginPolicy { auto_create_users },
        ))
    }

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip redirect-login tests in CI");
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

    /// Build the test app (real Postgres session storage, the given redirect
    /// providers) and initialize it. A macro because `init_service`'s service
    /// type is opaque and cannot be named in a helper's signature.
    macro_rules! test_app {
        ($redirects:expr, $url:expr) => {{
            let pool = test_pool($url);
            // Sessions are registered both raw and via the service, mirroring
            // `main.rs`.
            let sessions: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool.clone()));
            let sessions_data: web::Data<dyn SessionRepository> = sessions.clone().into();
            let redirects: Vec<Arc<dyn RedirectProvider>> = $redirects;
            // Registered like `main.rs`: the providers list drives
            // `/api/auth/providers`, and the shared flow config exists exactly
            // when at least one redirect provider is registered.
            let flow: Option<web::Data<RedirectFlow>> = (!redirects.is_empty()).then(test_flow);
            let providers = web::Data::new(
                AuthProviders::new(
                    vec![Arc::new(PasswordAuthProvider::new(
                        Arc::new(PostgresUserRepository::new(pool.clone())),
                        Arc::new(Argon2PasswordHasher),
                    ))],
                    redirects,
                )
                .expect("static provider ids are valid and unique"),
            );
            // The accept-invite route is registered (mirroring `main.rs`) so
            // the SSO-invite test can try a token after the callback consumed
            // it.
            let session_service = SessionService::new(
                sessions,
                Arc::new(Sha256SessionTokens),
                SessionService::DEFAULT_SESSION_TTL,
            );
            let account_link_service = AccountLinkService::new(
                Arc::new(PostgresAccountTokenRepository::new(pool.clone())),
                Arc::new(PostgresUserRepository::new(pool.clone())),
                Arc::new(Argon2PasswordHasher),
                Arc::new(Sha256SessionTokens),
                session_service.clone(),
                Arc::new(NoEmailSender),
                String::new(),
            );
            let app = App::new()
                .app_data(sessions_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(account_link_service))
                .app_data(providers)
                .app_data(web::Data::new(auth::CookieSettings { secure: false }));
            let app = match flow {
                Some(flow) => app.app_data(flow),
                None => app,
            };
            init_service(
                app.route(
                    "/api/auth/providers",
                    web::get().to(crate::auth::list_auth_providers),
                )
                .route("/api/auth/{provider}/login", web::get().to(redirect_login))
                .route(
                    "/api/auth/{provider}/callback",
                    web::get().to(redirect_callback),
                )
                .route(
                    "/api/auth/accept-invite",
                    web::post().to(crate::account_links::accept_invite),
                ),
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
                .trim_start_matches(&format!("{STATE_COOKIE}="))
                .to_owned();
            let callback = $app
                .call(
                    TestRequest::get()
                        .uri("/api/auth/oidc/callback?code=abc&state=csrf-1")
                        .insert_header((header::COOKIE, format!("{STATE_COOKIE}={state_cookie}")))
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
    async fn providers_lists_only_password_when_no_redirect_provider_is_registered() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(vec![], &url);
        let res = app
            .call(TestRequest::get().uri("/api/auth/providers").to_request())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = actix_web::test::read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "providers": [{
                    "id": "password",
                    "display_name": "Email and password",
                    "kind": "credentials",
                    "login_url": "/api/auth/login"
                }]
            })
        );
    }

    #[actix_web::test]
    async fn providers_lists_password_and_oidc_when_oidc_is_enabled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
        let res = app
            .call(TestRequest::get().uri("/api/auth/providers").to_request())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = actix_web::test::read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "providers": [
                    {
                        "id": "password",
                        "display_name": "Email and password",
                        "kind": "credentials",
                        "login_url": "/api/auth/login"
                    },
                    {
                        "id": "oidc",
                        "display_name": "Fake IdP",
                        "kind": "redirect",
                        "login_url": "/api/auth/oidc/login"
                    }
                ]
            })
        );
    }

    #[actix_web::test]
    async fn flow_routes_404_when_no_redirect_provider_is_registered() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(vec![], &url);
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
    async fn flow_routes_404_for_unknown_or_non_redirect_providers() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
        // An unregistered id, and a registered provider of the other kind.
        for uri in [
            "/api/auth/nope/login",
            "/api/auth/password/login",
            "/api/auth/password/callback?code=x&state=y",
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
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
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
            .trim_start_matches(&format!("{STATE_COOKIE}="));
        let loaded =
            read_state(state_cookie_header(value), &test_key(7)).expect("state cookie decrypts");
        assert_eq!(loaded.provider, "oidc");
        assert_eq!(loaded.next, "/dashboard");
    }

    #[actix_web::test]
    async fn login_ignores_a_next_that_is_not_site_relative() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
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
                .trim_start_matches(&format!("{STATE_COOKIE}="));
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

        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims(&subject, Some(&email), true),
                true
            )],
            &url
        );
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
        assert_eq!(
            user.role,
            Role::ReadOnly,
            "new SSO accounts get the fallback role (DEFAULT_NEW_USER_ROLE), never Admin"
        );
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), subject)
            .await
            .unwrap()
            .expect("identity created");
        assert_eq!(identity.user_id, user.id);
    }

    #[actix_web::test]
    async fn an_sso_login_consumes_a_pending_invite_for_the_verified_email() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let subject = format!("sub-{}", Uuid::new_v4());
        let email = format!("invited-{}@example.com", Uuid::new_v4());

        // A pending Staff invite for the email the IdP will verify, inserted
        // through the repository like an admin's POST /api/invites would be.
        let pool = test_pool(&url);
        let now = Utc::now();
        let raw_token = Uuid::new_v4().to_string();
        let session_tokens = Sha256SessionTokens;
        let invite = AccountToken {
            id: AccountTokenId::new(),
            kind: AccountTokenKind::Invite {
                email: email.clone(),
                role: Role::Staff,
            },
            token_hash: session_tokens.hash(&raw_token),
            created_by: None,
            created_at: now,
            expires_at: now + Duration::hours(48),
            consumed_at: None,
            revoked_at: None,
        };
        PostgresAccountTokenRepository::new(pool)
            .issue(invite, now)
            .await
            .unwrap();

        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims(&subject, Some(&email), true),
                true
            )],
            &url
        );
        let (_login, callback) = login_and_callback!(app, None);
        assert_eq!(callback.status(), StatusCode::FOUND);
        assert!(
            set_cookie(&callback, "minerva_session").is_some(),
            "session cookie set"
        );

        // The SSO-created account took the invite's role...
        let pool = test_pool(&url);
        let users = PostgresUserRepository::new(pool.clone());
        let user = users
            .find_by_email(email)
            .await
            .unwrap()
            .expect("user created");
        assert_eq!(
            user.role,
            Role::Staff,
            "the SSO-created account takes the invite's role"
        );

        // ...and the invite is consumed: accepting it now fails.
        let res = app
            .call(
                TestRequest::post()
                    .uri("/api/auth/accept-invite")
                    .set_json(serde_json::json!({
                        "token": raw_token,
                        "password": "long-enough-pass"
                    }))
                    .to_request(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = actix_web::test::read_body(res).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_token");
    }

    #[actix_web::test]
    async fn second_login_with_the_same_identity_reuses_the_user() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let subject = format!("sub-{}", Uuid::new_v4());
        let email = format!("oidc-{}@example.com", Uuid::new_v4());

        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims(&subject, Some(&email), true),
                true
            )],
            &url
        );
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
            role: Role::Admin,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        };
        let existing = users.create(user).await.unwrap();

        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims(&subject, Some(&email), true),
                true
            )],
            &url
        );
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
        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims("sub-unverified", Some(&email), false),
                true
            )],
            &url
        );
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
        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims("sub-no-email", None, true),
                true
            )],
            &url
        );
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
        let app = test_app!(
            vec![oidc_redirect(
                &url,
                claims("sub-nosignup", Some(&email), true),
                false
            )],
            &url
        );
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
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
        // A forged callback URL: code and state look right, but there is
        // no state cookie behind them. This is an interface-level failure,
        // so it redirects with the generic `login_failed` code.
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
            "http://localhost:9999/login?error=login_failed"
        );
    }

    #[actix_web::test]
    async fn callback_with_a_mismatched_state_fails() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
        // Valid cookie, but the provider echoed a different state back: the
        // mismatch is the OIDC provider's finding, so it keeps its own code.
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
            .trim_start_matches(&format!("{STATE_COOKIE}="))
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
        let app = test_app!(
            vec![oidc_redirect(&url, claims("sub-1", None, false), true)],
            &url
        );
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
            .trim_start_matches(&format!("{STATE_COOKIE}="))
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

    #[actix_web::test]
    async fn a_second_redirect_provider_is_dispatched_by_id() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let redirects = vec![
            oidc_redirect(&url, claims("sub-1", None, false), true),
            Arc::new(FakeRedirect),
        ];
        let app = test_app!(redirects, &url);
        // The generic route dispatches by registered id, not by hardcoding
        // OIDC: the fake provider's login route works with no other change.
        let res = app
            .call(TestRequest::get().uri("/api/auth/fake/login").to_request())
            .await
            .unwrap()
            .into_parts()
            .1;
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(redirect_location(&res), "https://fake.example/authorize");

        // The state cookie is scoped to the provider that started the flow.
        let set = set_cookie(&res, STATE_COOKIE).expect("state cookie set");
        assert!(set.contains("Path=/api/auth/fake"));
        let value = set
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{STATE_COOKIE}="));
        let loaded =
            read_state(state_cookie_header(value), &test_key(7)).expect("state cookie decrypts");
        assert_eq!(loaded.provider, "fake");
    }

    #[actix_web::test]
    async fn a_state_cookie_for_one_provider_is_rejected_by_another() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let redirects = vec![
            oidc_redirect(&url, claims("sub-1", None, false), true),
            Arc::new(FakeRedirect),
        ];
        let app = test_app!(redirects, &url);
        // A state cookie issued for the OIDC flow...
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
            .trim_start_matches(&format!("{STATE_COOKIE}="))
            .to_owned();
        // ...must not be accepted by the fake provider's callback.
        let res = app
            .call(
                TestRequest::get()
                    .uri("/api/auth/fake/callback?code=abc&state=csrf-1")
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
            "http://localhost:9999/login?error=login_failed"
        );
    }
}
