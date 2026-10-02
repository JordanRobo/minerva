mod access;
mod auth;
mod config;
mod debug;
mod error;
mod goals;
mod milestones;
mod openapi;
mod redirect;
mod routes;
mod tasks;

#[cfg(test)]
mod access_tests;

use actix_web::cookie::Key;
use actix_web::{App, HttpServer, web};
use application::auth::SessionService;
use application::auth::oidc::OidcAuthProvider;
use application::auth::password::PasswordAuthProvider;
use application::auth::provider::{AuthProviders, RedirectProvider};
use application::oidc_login::LoginPolicy;
use application::ports::{
    OidcProvider, PasswordHasher, SessionRepository, UserIdentityRepository, UserRepository,
};
use infrastructure::Argon2PasswordHasher;
use infrastructure::Sha256SessionTokens;
use infrastructure::db::build_pool;
use infrastructure::migrations::run_migrations;
use infrastructure::oidc::{OidcConfig, OpenIdConnectProvider};
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresSessionRepository, PostgresTaskRelationRepository,
    PostgresTaskRepository, PostgresUserIdentityRepository, PostgresUserRepository,
    RedisSessionRepository,
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

    // One shared pool, nine repositories. Each is registered as its own
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
    // The auth handlers take ports, not concrete repositories, so these are
    // registered as trait objects (the same way sessions below are).
    let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
    let users_data: web::Data<dyn UserRepository> = users.clone().into();
    // The OIDC provider below also takes these ports, so the Arcs stay
    // around instead of being converted straight to `web::Data`.
    let user_identities: Arc<dyn UserIdentityRepository> =
        Arc::new(PostgresUserIdentityRepository::new(pool.clone()));
    let user_identities_data: web::Data<dyn UserIdentityRepository> =
        user_identities.clone().into();
    // Sessions are the swappable storage: Redis when redis.url is configured,
    // Postgres otherwise. A missing URL is not an error — it just means "use
    // Postgres for sessions" (see docs/architecture.md). A present but
    // unreachable one fails fast, like database.url does. The URL may carry
    // credentials, so it stays out of the log line.
    let sessions: Arc<dyn SessionRepository> = match config.redis.url.expose().trim() {
        "" => {
            println!("redis not configured; using Postgres for session storage");
            Arc::new(PostgresSessionRepository::new(pool))
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
    let session_service = web::Data::new(SessionService::new(
        sessions,
        Arc::new(Sha256SessionTokens),
        SessionService::DEFAULT_SESSION_TTL,
    ));
    // The password hasher holds no state; it is registered like the
    // repositories so handlers name their dependency in their signature.
    let password_hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
    let password_hasher_data: web::Data<dyn PasswordHasher> = password_hasher.clone().into();
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
            .app_data(users_data.clone())
            .app_data(user_identities_data.clone())
            .app_data(sessions_data.clone())
            .app_data(session_service.clone())
            .app_data(auth_providers.clone())
            .app_data(password_hasher_data.clone())
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
