mod access;
mod account_links;
mod auth;
mod client_ip;
mod config;
mod debug;
mod error;
mod goals;
mod invites;
mod maintenance;
mod milestones;
mod openapi;
mod redirect;
mod routes;
mod sso_rules;
mod tasks;
mod users;

#[cfg(test)]
mod access_tests;

#[cfg(test)]
mod public_routes;

use actix_web::cookie::Key;
use actix_web::{App, HttpServer, web};
use application::account_links::AccountLinkService;
use application::auth::SessionService;
use application::auth::oidc::OidcAuthProvider;
use application::auth::password::PasswordAuthProvider;
use application::auth::provider::{AuthProviders, RedirectProvider};
use application::bootstrap::{BootstrapAdmin, BootstrapOutcome, bootstrap_admin};
use application::oidc_login::LoginPolicy;
use application::ports::{
    AccountEmailSender, AccountTokenRepository, OidcProvider, PasswordHasher, SessionRepository,
    SessionTokens, UserIdentityRepository, UserRepository,
};
use application::rate_limit::{RateLimitService, RateLimiter};
use application::sso_roles::SsoRoleService;
use application::sso_rules::SsoGroupRuleService;
use application::user_admin::UserAdminService;
use chrono::Utc;
use infrastructure::Argon2PasswordHasher;
use infrastructure::NoEmailSender;
use infrastructure::Sha256SessionTokens;
use infrastructure::db::build_pool;
use infrastructure::migrations::run_migrations;
use infrastructure::oidc::{OidcConfig, OpenIdConnectProvider};
use infrastructure::repositories::{
    PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
    PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresRateLimiter,
    PostgresSessionRepository, PostgresSsoGroupRuleRepository, PostgresTaskRelationRepository,
    PostgresTaskRepository, PostgresUserIdentityRepository, PostgresUserRepository,
    RedisRateLimiter, RedisSessionRepository,
};
use std::sync::Arc;

use crate::redirect::RedirectFlow;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // All file and environment access happens in the config module; it layers
    // defaults, minerva.toml and environment overrides, and reports every
    // problem together instead of failing one fix at a time.
    let config = match config::load() {
        Ok(config) => config,
        Err(errors) => {
            eprintln!("configuration error(s):");
            for error in &errors {
                eprintln!("  - {error}");
            }
            std::process::exit(1);
        }
    };

    let port = config.server.port;

    // Fail fast if the primary datastore is missing or unreachable: a server
    // without its database has no meaningful degraded mode. `build_pool` also
    // checks one connection, so an unreachable Postgres panics here at startup
    // rather than on the first request.
    let database_url = config.database.url.expose().to_owned();
    let pool = build_pool(&database_url);

    // Apply pending migrations so a fresh database is usable without a
    // separate migration step. Several nodes can start at once (the API
    // scales horizontally), and `run_migrations` serializes them with a
    // Postgres advisory lock. server.run_migrations = false opts out, e.g.
    // when a dedicated migration job owns the schema.
    if config.server.run_migrations {
        match run_migrations(&pool) {
            Ok(applied) if applied.is_empty() => println!("no pending migrations"),
            Ok(applied) => println!("applied {} migration(s)", applied.len()),
            Err(err) => panic!("{err}"),
        }
    } else {
        println!("run_migrations is false; skipping migrations");
    }

    // One shared pool, ten repositories. Each is registered as its own
    // `web::Data` rather than wrapped in a single AppState struct: every
    // handler uses exactly one repository, so per-repo Data keeps each
    // handler's signature naming only the repo it actually calls. (The pool
    // clones are cheap — r2d2 pools share their connections behind an Arc.)
    let goals = web::Data::new(PostgresGoalRepository::new(pool.clone()));
    let milestones = web::Data::new(PostgresMilestoneRepository::new(pool.clone()));
    let goal_milestones = web::Data::new(PostgresGoalMilestoneRepository::new(pool.clone()));
    let tasks = web::Data::new(PostgresTaskRepository::new(pool.clone()));
    let task_relations = web::Data::new(PostgresTaskRelationRepository::new(pool.clone()));
    let progress_snapshots = web::Data::new(PostgresProgressSnapshotRepository::new(pool.clone()));
    // SSO group rules (roadmap 2.7, D15): the admin-only API goes through the
    // service so name validation and the duplicate-name conflict live in one
    // place (see application::sso_rules).
    // The user-admin service below shares the same repository: it asks
    // whether any rule exists to lock SSO-managed roles against hand edits.
    let sso_group_rules_repo = Arc::new(PostgresSsoGroupRuleRepository::new(pool.clone()));
    let sso_group_rules = web::Data::new(SsoGroupRuleService::new(sso_group_rules_repo.clone()));
    // The auth handlers take ports, not concrete repositories, so these are
    // registered as trait objects (the same way sessions below are).
    let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
    let users_data: web::Data<dyn UserRepository> = users.clone().into();
    // The token store behind invites and password resets (roadmap 2.6); the
    // account-link service below and the OIDC provider (when enabled) both
    // take it — the latter to consume a pending invite when an invited email
    // signs in via SSO before accepting it.
    let account_tokens: Arc<dyn AccountTokenRepository> =
        Arc::new(PostgresAccountTokenRepository::new(pool.clone()));
    // The OIDC provider below also takes these ports, so the Arcs stay
    // around instead of being converted straight to `web::Data`.
    let user_identities: Arc<dyn UserIdentityRepository> =
        Arc::new(PostgresUserIdentityRepository::new(pool.clone()));
    let user_identities_data: web::Data<dyn UserIdentityRepository> =
        user_identities.clone().into();
    // SSO group-to-role mapping (roadmap 2.7, D15): while any rule exists,
    // the OIDC provider below recomputes roles from the IdP groups at every
    // login. It shares the rules repository with the user-admin service's
    // hand-edit lock, so both read the same committed rules.
    let sso_roles = SsoRoleService::new(sso_group_rules_repo.clone(), users.clone());
    // Rate limiting (roadmap 2.8): fixed-window counters behind the same
    // storage selection as sessions — Redis when redis.url is configured,
    // Postgres otherwise. In-process counters are deliberately not an option:
    // they break horizontal scaling. Built before the session match below
    // because that one moves the pool into the maintenance runner.
    let rate_limiter: Arc<dyn RateLimiter> = if config.redis.url.expose().trim().is_empty() {
        println!("redis not configured; using Postgres for rate-limit counters");
        Arc::new(PostgresRateLimiter::new(pool.clone()))
    } else {
        println!("using Redis for rate-limit counters");
        Arc::new(
            RedisRateLimiter::connect(config.redis.url.expose())
                .expect("redis.url is set but could not connect to Redis"),
        )
    };
    // Sessions are the swappable storage: Redis when redis.url is configured,
    // Postgres otherwise. A missing URL is not an error — it just means "use
    // Postgres for sessions" (see docs/architecture.md). A present but
    // unreachable one fails fast, like database.url does. The URL may carry
    // credentials, so it stays out of the log line.
    let sessions: Arc<dyn SessionRepository> = match config.redis.url.expose().trim() {
        "" => {
            println!("redis not configured; using Postgres for session storage");
            let sessions = Arc::new(PostgresSessionRepository::new(pool.clone()));
            // With Postgres as the session store, expired rows and stale
            // rate-limit counters are only removed by these hourly
            // maintenance jobs (with Redis, native TTLs evict keys instead).
            // Every node runs them; their advisory locks mean exactly one of
            // the nodes does the work per tick (roadmap 2.8).
            maintenance::start(
                pool,
                vec![
                    maintenance::purge_expired_sessions(sessions.clone()),
                    maintenance::purge_rate_limit_counters(rate_limiter.clone()),
                ],
            );
            sessions
        }
        redis_url => {
            println!("using Redis for session storage");
            Arc::new(
                RedisSessionRepository::connect(redis_url)
                    .expect("redis.url is set but could not connect to Redis"),
            )
        }
    };
    // The raw repository stays registered alongside the service: handlers go
    // through the service (the TTL is the fixed 30 days), but code that needs
    // session storage directly can still extract the port.
    let sessions_data: web::Data<dyn SessionRepository> = sessions.clone().into();
    // The raw-token generator is shared between the session service and the
    // account-link service, so both kinds of token come from one port.
    let session_tokens: Arc<dyn SessionTokens> = Arc::new(Sha256SessionTokens);
    let session_service = web::Data::new(SessionService::new(
        sessions,
        session_tokens.clone(),
        SessionService::DEFAULT_SESSION_TTL,
    ));
    // Rate limiting (roadmap 2.8): the service hashes subjects and fails open
    // on store errors (see application::rate_limit). Step 5 of 2.8 attaches it
    // to the login and token-link routes; until then it is only wired up.
    let rate_limit_service = web::Data::new(RateLimitService::new(
        rate_limiter,
        config.rate_limit.enabled,
    ));
    // User administration goes through the service so the self-modification
    // and last-admin rules live in one place (see application::user_admin).
    let user_admin = web::Data::new(UserAdminService::new(
        users.clone(),
        sso_group_rules_repo,
        session_service.get_ref().clone(),
    ));
    // The password hasher holds no state; the password provider and the
    // bootstrap admin below both need it, so it is built once here.
    let password_hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
    // Account links (roadmap 2.6): invites and password resets go through the
    // service so the link rules live in one place. Email delivery is not
    // configured yet (roadmap 7.3), so NoEmailSender: every send fails and
    // the endpoints return the links to the caller instead of emailing them.
    let email: Arc<dyn AccountEmailSender> = Arc::new(NoEmailSender);
    let account_link_service = web::Data::new(AccountLinkService::new(
        account_tokens.clone(),
        users.clone(),
        password_hasher.clone(),
        session_tokens,
        session_service.get_ref().clone(),
        email,
        // Links are built from server.web_base_url — already trailing-slash-
        // stripped when set, empty for site-relative links.
        config.server.web_base_url.clone(),
    ));
    // First-admin bootstrap (roadmap 2.5): while the database has no users,
    // create the configured admin so there is someone who can log in and
    // start inviting people. The repository decides atomically whether the
    // table is empty, so racing nodes cannot both create an admin; once any
    // user exists this prints only the skip line. It runs regardless of
    // run_migrations — the schema exists either way (migrated above or owned
    // by a dedicated job). The password never reaches these log lines.
    if config.bootstrap_enabled() {
        match bootstrap_admin(
            users.as_ref(),
            password_hasher.as_ref(),
            BootstrapAdmin {
                email: config.bootstrap.admin_email.clone(),
                display_name: config.bootstrap.admin_display_name.clone(),
                password: config.bootstrap.admin_password.expose().to_owned(),
            },
            Utc::now(),
        )
        .await
        {
            Ok(BootstrapOutcome::Created(user)) => {
                println!("created bootstrap admin account {}", user.email)
            }
            Ok(BootstrapOutcome::UsersAlreadyExist) => {
                println!("bootstrap admin skipped: users already exist")
            }
            Err(err) => panic!("{err}"),
        }
    }
    // OIDC sign-in is optional: without oidc.issuer_url the server behaves
    // exactly as before and nothing redirect-related is registered. With it,
    // an unreachable/misconfigured IdP that answers 4xx fails startup like
    // database.url does; a merely unreachable IdP only warns (discovery
    // retries lazily on first use). The configuration has already validated
    // every URL and secret, so the parses below cannot fail.
    let (redirect_providers, redirect_flow) = if config.oidc_enabled() {
        let provider_config = OidcConfig {
            issuer_url: url::Url::parse(&config.oidc.issuer_url)
                .expect("validated by the configuration"),
            client_id: config.oidc.client_id.clone(),
            client_secret: config.oidc.client_secret.expose().to_owned(),
            redirect_url: url::Url::parse(&config.oidc.redirect_url)
                .expect("validated by the configuration"),
            display_name: config.oidc.display_name.clone(),
            scopes: config.oidc.scopes.clone(),
            groups_claim: config.oidc.groups_claim.clone(),
        };
        let protocol: Arc<dyn OidcProvider> = Arc::new(
            OpenIdConnectProvider::connect(provider_config)
                .await
                .unwrap_or_else(|err| panic!("{err}")),
        );
        println!("OIDC enabled (issuer {})", config.oidc.issuer_url);
        // The protocol provider is wrapped in the login-policy provider and
        // registered like any other redirect provider.
        let oidc: Arc<dyn RedirectProvider> = Arc::new(OidcAuthProvider::new(
            protocol,
            users.clone(),
            user_identities.clone(),
            account_tokens,
            sso_roles,
            LoginPolicy {
                auto_create_users: config.oidc.auto_create_users,
            },
        ));
        (
            vec![oidc],
            // The state cookie is shared by all redirect providers, so it is
            // registered exactly when at least one exists; its key still comes
            // from oidc.state_secret (the only secret of the right size today).
            Some(web::Data::new(RedirectFlow {
                key: Key::derive_from(config.oidc.state_secret.expose().as_bytes()),
                base_url: config.server.web_base_url.clone(),
            })),
        )
    } else {
        println!("OIDC not configured");
        (Vec::new(), None)
    };

    // Every sign-in method is a registered provider, looked up by id: adding
    // one later means implementing a trait and extending this list, not
    // touching session handling or the existing handlers.
    let auth_providers = web::Data::new(
        AuthProviders::new(
            vec![Arc::new(PasswordAuthProvider::new(
                users.clone(),
                password_hasher.clone(),
            ))],
            redirect_providers,
        )
        .expect("static provider ids are valid and unique"),
    );
    // Shared cookie attributes (the `Secure` flag) for the session and
    // redirect-state cookies, from server.cookie_secure.
    let cookies = web::Data::new(auth::CookieSettings {
        secure: config.server.cookie_secure,
    });

    println!("minerva-server listening on 0.0.0.0:{port}");

    HttpServer::new(move || {
        let mut app = App::new()
            .app_data(goals.clone())
            .app_data(milestones.clone())
            .app_data(goal_milestones.clone())
            .app_data(tasks.clone())
            .app_data(task_relations.clone())
            .app_data(progress_snapshots.clone())
            .app_data(sso_group_rules.clone())
            .app_data(users_data.clone())
            .app_data(user_identities_data.clone())
            .app_data(sessions_data.clone())
            .app_data(session_service.clone())
            .app_data(rate_limit_service.clone())
            .app_data(user_admin.clone())
            .app_data(account_link_service.clone())
            .app_data(auth_providers.clone())
            .app_data(cookies.clone());
        // Registered only when at least one redirect provider exists; the
        // flow handlers take it as an `Option` extractor and treat its
        // absence as "no redirect login" (404 on the flow routes).
        if let Some(flow) = redirect_flow.clone() {
            app = app.app_data(flow);
        }
        app.configure(routes::configure)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
