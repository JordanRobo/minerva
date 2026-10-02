mod auth;
mod config;
mod debug;
mod error;
mod goals;
mod milestones;
mod oidc;
mod openapi;
mod tasks;

use actix_web::{App, HttpResponse, HttpServer, web};
use application::ports::{OidcProvider, SessionRepository};
use infrastructure::Argon2PasswordHasher;
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
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::error::ApiError;
use crate::oidc::OidcAuth;
use crate::openapi::ApiDoc;

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

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
    let users = web::Data::new(PostgresUserRepository::new(pool.clone()));
    let user_identities = web::Data::new(PostgresUserIdentityRepository::new(pool.clone()));
    // Sessions are the swappable storage: Redis when redis.url is configured,
    // Postgres otherwise. A missing URL is not an error — it just means "use
    // Postgres for sessions" (see docs/architecture.md). A present but
    // unreachable one fails fast, like database.url does. The URL may carry
    // credentials, so it stays out of the log line.
    let sessions: web::Data<dyn SessionRepository> = match config.redis.url.expose().trim() {
        "" => {
            println!("redis not configured; using Postgres for session storage");
            let repo: Arc<dyn SessionRepository> = Arc::new(PostgresSessionRepository::new(pool));
            repo.into()
        }
        redis_url => {
            println!("using Redis for session storage");
            let repo: Arc<dyn SessionRepository> = Arc::new(
                RedisSessionRepository::connect(redis_url)
                    .expect("redis.url is set but could not connect to Redis"),
            );
            repo.into()
        }
    };
    // The password hasher holds no state; it is registered like the
    // repositories so handlers name their dependency in their signature.
    let password_hasher = web::Data::new(Argon2PasswordHasher);
    // Shared cookie attributes (the `Secure` flag) for the session and OIDC
    // state cookies, from server.cookie_secure.
    let cookies = web::Data::new(auth::CookieSettings {
        secure: config.server.cookie_secure,
    });

    // OIDC sign-in is optional: without oidc.issuer_url the server behaves
    // exactly as before and nothing OIDC-related is registered. With it, an
    // unreachable/misconfigured IdP that answers 4xx fails startup like
    // database.url does; a merely unreachable IdP only warns (discovery
    // retries lazily on first use). The configuration has already validated
    // every URL and secret, so the parses below cannot fail.
    let oidc: Option<web::Data<OidcAuth>> = if config.oidc_enabled() {
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
        let provider: Arc<dyn OidcProvider> = Arc::new(
            OpenIdConnectProvider::connect(provider_config)
                .await
                .unwrap_or_else(|err| panic!("{err}")),
        );
        println!("OIDC enabled (issuer {})", config.oidc.issuer_url);
        Some(web::Data::new(OidcAuth::new(
            provider,
            config.server.web_base_url.clone(),
            &config.oidc.state_secret,
            config.oidc.auto_create_users,
        )))
    } else {
        println!("OIDC not configured");
        None
    };

    println!("minerva-server listening on 0.0.0.0:{port}");

    HttpServer::new(move || {
        let openapi = ApiDoc::openapi();
        let mut app = App::new()
            .app_data(goals.clone())
            .app_data(milestones.clone())
            .app_data(goal_milestones.clone())
            .app_data(tasks.clone())
            .app_data(task_relations.clone())
            .app_data(progress_snapshots.clone())
            .app_data(users.clone())
            .app_data(user_identities.clone())
            .app_data(sessions.clone())
            .app_data(password_hasher.clone())
            .app_data(cookies.clone());
        // Registered only when OIDC is configured; the OIDC handlers take it
        // as an `Option` extractor and treat its absence as "OIDC off".
        if let Some(oidc) = oidc.clone() {
            app = app.app_data(oidc);
        }
        app.route("/health", web::get().to(health))
            // The real API surface, built endpoint-group by endpoint-group.
            .service(
                web::scope("/api")
                    // Malformed or unparsable JSON bodies get the standard
                    // error envelope instead of Actix's default plaintext.
                    .app_data(web::JsonConfig::default().error_handler(|err, _req| {
                        ApiError::bad_request(format!("invalid JSON body: {err}")).into()
                    }))
                    .route("/goals", web::post().to(goals::create_goal))
                    .route("/goals", web::get().to(goals::list_goals))
                    .route("/goals/{id}", web::get().to(goals::get_goal))
                    .route("/goals/{id}", web::put().to(goals::update_goal))
                    .route("/goals/{id}", web::delete().to(goals::delete_goal))
                    .route("/milestones", web::post().to(milestones::create_milestone))
                    .route("/milestones", web::get().to(milestones::list_milestones))
                    .route("/milestones/{id}", web::get().to(milestones::get_milestone))
                    .route(
                        "/milestones/{id}",
                        web::put().to(milestones::update_milestone),
                    )
                    .route(
                        "/milestones/{id}",
                        web::delete().to(milestones::delete_milestone),
                    )
                    .route("/tasks", web::post().to(tasks::create_task))
                    .route("/tasks", web::get().to(tasks::list_tasks))
                    .route("/tasks/{id}", web::get().to(tasks::get_task))
                    .route("/tasks/{id}", web::put().to(tasks::update_task))
                    .route("/tasks/{id}", web::delete().to(tasks::delete_task))
                    .route("/auth/signup", web::post().to(auth::signup))
                    .route("/auth/login", web::post().to(auth::login))
                    .route("/auth/logout", web::post().to(auth::logout))
                    .route("/auth/me", web::get().to(auth::me))
                    .route("/auth/providers", web::get().to(oidc::list_auth_providers))
                    .route("/auth/oidc/login", web::get().to(oidc::oidc_login))
                    .route("/auth/oidc/callback", web::get().to(oidc::oidc_callback)),
            )
            // TEMPORARY: verifies repository wiring end-to-end; unauthenticated
            // and not meant to ship. Remove this scope before /debug is a real API.
            .service(
                web::scope("/debug")
                    .route("/task-relations", web::get().to(debug::list_task_relations))
                    .route(
                        "/progress-snapshots",
                        web::get().to(debug::list_progress_snapshots),
                    )
                    .route(
                        "/goal-milestones",
                        web::get().to(debug::list_goal_milestones),
                    ),
            )
            // API documentation (not part of the /api surface): a Swagger UI
            // rendering the generated OpenAPI 3 document, plus the raw JSON at
            // /api-docs/openapi.json (registered by `.url`). Unauthenticated
            // like the rest of the server; route protection is roadmap item 2.4.
            .service(
                SwaggerUi::new("/api-docs/swagger-ui/{_:.*}")
                    .url("/api-docs/openapi.json", openapi),
            )
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
