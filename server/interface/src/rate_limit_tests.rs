//! Rate-limit handler tests (roadmap 2.8).
//!
//! These run the real route table with rate limiting *enabled* against a
//! [`TestRateLimiter`] carrying small per-policy limits, so any bucket
//! exhausts in a handful of requests and nothing depends on the wall clock.
//! The other handler-test modules run the same routes with rate limiting
//! disabled: they assert authentication and authorisation, not throttling.

use actix_web::App;
use actix_web::dev::Service;
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::account_links::AccountLinkService;
use application::auth::SessionService;
use application::auth::password::PasswordAuthProvider;
use application::auth::provider::AuthProviders;
use application::ports::{PasswordHasher, SessionRepository, SessionTokens, UserRepository};
use application::rate_limit::{
    RateLimitDecision, RateLimitError, RateLimitPolicy, RateLimitService, RateLimiter,
};
use application::sso_rules::SsoGroupRuleService;
use application::user_admin::UserAdminService;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use domain::{Role, User, UserId};
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
    PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresSessionRepository,
    PostgresSsoGroupRuleRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
    PostgresUserRepository,
};
use infrastructure::{NoEmailSender, Sha256SessionTokens};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{COOKIE_NAME, CookieSettings};
use crate::config::RateLimitConfig;
use crate::routes;

/// The operations the rate limiter guards (roadmap 2.8): login, the three
/// public token-link routes and the three admin issue endpoints. The OpenAPI
/// document test asserts that exactly these declare a 429 and no other does.
pub(crate) const RATE_LIMITED_OPERATIONS: &[(Method, &str)] = &[
    (Method::POST, "/api/auth/login"),
    (Method::POST, "/api/auth/tokens/inspect"),
    (Method::POST, "/api/auth/accept-invite"),
    (Method::POST, "/api/auth/reset-password"),
    (Method::POST, "/api/invites"),
    (Method::POST, "/api/invites/{id}/reissue"),
    (Method::POST, "/api/users/{id}/password-reset"),
];

/// A rate limiter for handler tests: allows a fixed number of hits per
/// (policy, subject) and then limits, with no windows — nothing in these
/// tests may depend on the wall clock. Policies absent from `limits` allow
/// everything.
pub struct TestRateLimiter {
    limits: HashMap<&'static str, u32>,
    counters: Mutex<HashMap<(String, String), u64>>,
}

impl TestRateLimiter {
    pub fn new(limits: &[(&'static str, u32)]) -> Arc<Self> {
        Arc::new(Self {
            limits: limits.iter().copied().collect(),
            counters: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl RateLimiter for TestRateLimiter {
    async fn hit(
        &self,
        policy: &RateLimitPolicy,
        subject: &str,
        _now: DateTime<Utc>,
    ) -> Result<RateLimitDecision, RateLimitError> {
        let mut counters = self.counters.lock().unwrap();
        let count = counters
            .entry((policy.name.to_owned(), subject.to_owned()))
            .or_insert(0);
        *count += 1;
        Ok(match self.limits.get(policy.name) {
            Some(limit) if *count > u64::from(*limit) => RateLimitDecision::Limited {
                retry_after: Duration::from_secs(42),
            },
            _ => RateLimitDecision::Allowed,
        })
    }

    async fn purge_before(&self, _cutoff: DateTime<Utc>) -> Result<u64, RateLimitError> {
        Ok(0)
    }
}

/// A password hasher that counts every verification and accepts only a
/// marker hash, so login tests run without real Argon2 and can prove a
/// limited request never reaches the verifier.
struct CountingHasher {
    calls: Arc<AtomicUsize>,
}

impl CountingHasher {
    /// The hasher for the app and the counter the test reads.
    fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

#[async_trait]
impl PasswordHasher for CountingHasher {
    async fn hash(&self, password: &str) -> Result<String, application::ports::PasswordHashError> {
        Ok(format!("hash-of-{password}"))
    }

    async fn verify(
        &self,
        password: &str,
        hash: &str,
    ) -> Result<bool, application::ports::PasswordHashError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(hash == format!("hash-of-{password}"))
    }

    async fn verify_dummy(
        &self,
        _password: &str,
    ) -> Result<bool, application::ports::PasswordHashError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
}

/// The `DATABASE_URL` the tests run against, or `None` to skip.
fn database_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    // In CI these tests must run: a green build that skipped them proves nothing.
    if url.is_none() && std::env::var_os("CI").is_some() {
        panic!("DATABASE_URL is not set; refusing to skip rate-limit tests in CI");
    }
    url
}

/// A one-connection pool (mirrors the other handler tests: the default pool
/// size times the parallel test pools would exceed local Postgres's
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

/// Build the app with the real route table and every `web::Data` a handler in
/// it can need, mirroring `main.rs`, with rate limiting switched by the
/// caller: `$hasher` the password hasher (a [`CountingHasher`] here),
/// `$limits` the per-policy allowances, `$enabled` the limiter switch and
/// `$header` the configured client-IP header. A macro because
/// `init_service`'s service type is opaque.
macro_rules! test_app {
    ($url:expr, $hasher:expr, $limits:expr, $enabled:expr, $header:expr) => {{
        let pool = test_pool($url);
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
        let users_data: web::Data<dyn UserRepository> = users.clone().into();
        let providers = web::Data::new(
            AuthProviders::new(
                vec![Arc::new(PasswordAuthProvider::new(
                    users.clone(),
                    $hasher.clone(),
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
        let account_link_service = AccountLinkService::new(
            Arc::new(PostgresAccountTokenRepository::new(pool.clone())),
            users.clone(),
            $hasher,
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
                .app_data(web::Data::new(SsoGroupRuleService::new(Arc::new(
                    PostgresSsoGroupRuleRepository::new(pool.clone()),
                ))))
                .app_data(web::Data::new(UserAdminService::new(
                    users,
                    Arc::new(PostgresSsoGroupRuleRepository::new(pool.clone())),
                    session_service,
                )))
                .app_data(providers)
                .app_data(web::Data::new(CookieSettings { secure: false }))
                .app_data(web::Data::new(RateLimitService::new(
                    TestRateLimiter::new($limits),
                    $enabled,
                )))
                .app_data(web::Data::new(RateLimitConfig {
                    enabled: $enabled,
                    client_ip_header: $header,
                }))
                .configure(routes::configure),
        )
        .await
    }};
}

/// One request against the test app: `peer` the TCP peer address, `xff` an
/// optional X-Forwarded-For value, `cookie` a session token (or none) and
/// `body` a JSON body for POST.
macro_rules! request {
    ($app:expr, $method:expr, $uri:expr, $peer:expr, $xff:expr, $cookie:expr, $body:expr) => {{
        let mut req = TestRequest::with_uri(&$uri)
            .method($method.clone())
            .peer_addr($peer);
        if let Some(xff) = $xff {
            req = req.insert_header(("X-Forwarded-For", xff));
        }
        let cookie: Option<&str> = $cookie;
        if let Some(c) = cookie {
            req = req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={c}")));
        }
        if let Some(body) = $body {
            req = req.set_json(body);
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
        display_name: "Rate limit test user".into(),
        role,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
        created_at: now,
        updated_at: now,
    };
    users.create(user).await.expect("create user")
}

/// A password account whose stored hash is the [`CountingHasher`] marker for
/// `password123`, so logins succeed without real Argon2.
async fn create_password_user(pool: &PgPool, email: String) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: Some("hash-of-password123".to_owned()),
        display_name: "Rate limit test user".into(),
        role: Role::Admin,
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

/// The `Retry-After` header of a 429 response, in seconds.
fn retry_after(res: &actix_web::dev::ServiceResponse) -> u32 {
    res.headers()
        .get(header::RETRY_AFTER)
        .expect("Retry-After set on 429")
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[actix_web::test]
async fn login_is_limited_per_client_ip() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, _calls) = CountingHasher::new();
    let user = create_password_user(&pool, unique_email("rl-login")).await;
    // The per-IP bucket allows 3 hits; the per-email bucket stays wide so
    // only the IP dimension can fire.
    let app = test_app!(
        &url,
        hasher,
        &[("login_ip", 3), ("login_ip_email", 100)],
        true,
        String::new()
    );

    let peer: SocketAddr = "203.0.113.7:80".parse().unwrap();
    for _ in 0..3 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            peer,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK, "within the limit");
    }

    let limited = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        peer,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(retry_after(&limited) > 0);
    let body = read_body(limited).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "rate_limited");

    // A different client IP has its own bucket.
    let other: SocketAddr = "198.51.100.8:80".parse().unwrap();
    let res = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        other,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(res.status(), StatusCode::OK, "a fresh IP is not limited");

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_limited_login_does_not_verify_the_password() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, calls) = CountingHasher::new();
    let user = create_password_user(&pool, unique_email("rl-verify")).await;
    let app = test_app!(&url, hasher, &[("login_ip", 3)], true, String::new());

    let peer: SocketAddr = "203.0.113.7:80".parse().unwrap();
    for _ in 0..3 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            peer,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK);
    }

    let limited = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        peer,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    // The limiter answered before the credential check: exactly one
    // verification per allowed attempt, none for the limited one.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "a limited request must not verify"
    );

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn login_buckets_are_per_ip_and_email() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, _calls) = CountingHasher::new();
    let user = create_password_user(&pool, unique_email("rl-buckets")).await;
    // The per-IP bucket is wide, the per-email one narrow: only the email
    // dimension can fire for the first address.
    let app = test_app!(
        &url,
        hasher,
        &[("login_ip", 10), ("login_ip_email", 2)],
        true,
        String::new()
    );

    let ip1: SocketAddr = "203.0.113.7:80".parse().unwrap();
    let ip2: SocketAddr = "198.51.100.8:80".parse().unwrap();
    let other_email = unique_email("rl-other");

    // Two hits fill the (ip1, user) bucket; the third is limited...
    for _ in 0..2 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            ip1,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK);
    }
    let limited = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        ip1,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    // ...but another email from the same IP is not limited by it (the IP
    // bucket still has room). Unknown emails 401 — they must not 429.
    for _ in 0..2 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            ip1,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": other_email, "password": "whatever" }))
        );
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "not the IP bucket");
    }

    // And the same email from another IP starts a fresh per-pair bucket: two
    // successes prove the (ip1, user) limit did not carry over (a third would
    // fill this pair's own bucket of 2).
    for _ in 0..2 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            ip2,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK, "a fresh IP is not limited");
    }

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn token_link_routes_are_limited_per_client_ip() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let (hasher, _calls) = CountingHasher::new();
    // Bogus tokens 400 from the link service — that is expected here; only
    // the 429 boundary is under test.
    let app = test_app!(&url, hasher, &[("token_link_ip", 3)], true, String::new());

    let peer: SocketAddr = "203.0.113.7:80".parse().unwrap();
    for _ in 0..3 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/tokens/inspect",
            peer,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "token": "bogus" }))
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "within the limit");
    }

    // The fourth inspect is limited...
    let limited = request!(
        &app,
        Method::POST,
        "/api/auth/tokens/inspect",
        peer,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "token": "bogus" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(retry_after(&limited) > 0);

    // ...and the other link routes share the same per-IP bucket.
    let accept = request!(
        &app,
        Method::POST,
        "/api/auth/accept-invite",
        peer,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "token": "bogus", "password": "long-enough-pass" }))
    );
    assert_eq!(accept.status(), StatusCode::TOO_MANY_REQUESTS);
    let reset = request!(
        &app,
        Method::POST,
        "/api/auth/reset-password",
        peer,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "token": "bogus", "password": "long-enough-pass" }))
    );
    assert_eq!(reset.status(), StatusCode::TOO_MANY_REQUESTS);

    // A different client IP has its own bucket.
    let other: SocketAddr = "198.51.100.8:80".parse().unwrap();
    let res = request!(
        &app,
        Method::POST,
        "/api/auth/tokens/inspect",
        other,
        None::<&str>,
        None::<&str>,
        Some(&serde_json::json!({ "token": "bogus" }))
    );
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "a fresh IP is not limited"
    );
}

#[actix_web::test]
async fn admin_issue_endpoints_are_limited_per_actor() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, _calls) = CountingHasher::new();
    let app = test_app!(
        &url,
        hasher,
        &[("admin_issue_actor", 3)],
        true,
        String::new()
    );

    let admin = create_user(&pool, unique_email("rl-admin"), Role::Admin).await;
    let target = create_password_user(&pool, unique_email("rl-target")).await;
    let admin_token = issue_token(&pool, admin.id).await;
    let peer: SocketAddr = "203.0.113.7:80".parse().unwrap();

    // The three issue endpoints share one bucket per acting admin...
    let created = request!(
        &app,
        Method::POST,
        "/api/invites",
        peer,
        None::<&str>,
        Some(&admin_token),
        Some(&serde_json::json!({ "email": unique_email("rl-invited"), "role": "staff" }))
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_json: serde_json::Value =
        serde_json::from_slice(&read_body(created).await).unwrap();
    let invite_id: Uuid = created_json["invite"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let reissued = request!(
        &app,
        Method::POST,
        format!("/api/invites/{invite_id}/reissue"),
        peer,
        None::<&str>,
        Some(&admin_token),
        None::<&serde_json::Value>
    );
    assert_eq!(reissued.status(), StatusCode::CREATED);
    let reissue_json: serde_json::Value =
        serde_json::from_slice(&read_body(reissued).await).unwrap();
    let reissue_id: Uuid = reissue_json["invite"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let reset = request!(
        &app,
        Method::POST,
        format!("/api/users/{}/password-reset", target.id.0),
        peer,
        None::<&str>,
        Some(&admin_token),
        None::<&serde_json::Value>
    );
    assert_eq!(reset.status(), StatusCode::CREATED);

    // ...and the next issue of any kind is limited.
    let limited = request!(
        &app,
        Method::POST,
        "/api/invites",
        peer,
        None::<&str>,
        Some(&admin_token),
        Some(&serde_json::json!({ "email": unique_email("rl-invited-2"), "role": "staff" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(retry_after(&limited) > 0);
    let body = read_body(limited).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "rate_limited");

    // A different admin has their own bucket.
    let other_admin = create_user(&pool, unique_email("rl-admin-2"), Role::Admin).await;
    let other_token = issue_token(&pool, other_admin.id).await;
    let fresh = request!(
        &app,
        Method::POST,
        "/api/invites",
        peer,
        None::<&str>,
        Some(&other_token),
        Some(&serde_json::json!({ "email": unique_email("rl-invited-3"), "role": "staff" }))
    );
    assert_eq!(
        fresh.status(),
        StatusCode::CREATED,
        "a fresh admin is not limited"
    );
    let fresh_json: serde_json::Value = serde_json::from_slice(&read_body(fresh).await).unwrap();
    let fresh_id: Uuid = fresh_json["invite"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    delete_token(&pool, invite_id);
    delete_token(&pool, reissue_id);
    delete_token(&pool, fresh_id);
    delete_user(&pool, target.id);
    delete_user(&pool, admin.id);
    delete_user(&pool, other_admin.id);
}

#[actix_web::test]
async fn disabled_rate_limiting_allows_everything() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, _calls) = CountingHasher::new();
    let user = create_password_user(&pool, unique_email("rl-disabled")).await;
    // Limits of 0 would trip on the very first hit if the limiter ran at all.
    let app = test_app!(
        &url,
        hasher,
        &[("login_ip", 0), ("login_ip_email", 0)],
        false,
        String::new()
    );

    let peer: SocketAddr = "203.0.113.7:80".parse().unwrap();
    for _ in 0..3 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            peer,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK, "disabled means unlimited");
    }

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_client_ip_header_is_honoured_with_a_peer_fallback() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let (hasher, _calls) = CountingHasher::new();
    let user = create_password_user(&pool, unique_email("rl-header")).await;
    let app = test_app!(
        &url,
        hasher,
        &[("login_ip", 3)],
        true,
        "X-Forwarded-For".to_owned()
    );

    // Behind a proxy the header value is the subject: three hits fill that
    // bucket, the fourth from the same header value is limited...
    let peer: SocketAddr = "10.0.0.1:80".parse().unwrap();
    for _ in 0..3 {
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            peer,
            Some("198.51.100.1"),
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(res.status(), StatusCode::OK);
    }
    let limited = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        peer,
        Some("198.51.100.1"),
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    // ...a different header value is a different bucket...
    let res = request!(
        &app,
        Method::POST,
        "/api/auth/login",
        peer,
        Some("198.51.100.2"),
        None::<&str>,
        Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
    );
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a fresh header value is not limited"
    );

    // ...and a request without the header falls back to the peer address.
    // Four header-less requests from four different peers must all pass: if
    // the fallback were one shared constant instead of the peer IP, the
    // fourth would land in an exhausted bucket.
    for i in 1..=4u16 {
        let bare_peer: SocketAddr = format!("10.0.1.{i}:80").parse().unwrap();
        let res = request!(
            &app,
            Method::POST,
            "/api/auth/login",
            bare_peer,
            None::<&str>,
            None::<&str>,
            Some(&serde_json::json!({ "email": user.email, "password": "password123" }))
        );
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "the peer fallback buckets per IP"
        );
    }

    delete_user(&pool, user.id);
}
