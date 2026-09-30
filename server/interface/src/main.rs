mod auth;
mod debug;
mod error;
mod goals;
mod milestones;
mod openapi;
mod tasks;

use actix_web::{web, App, HttpResponse, HttpServer};
use application::ports::SessionRepository;
use infrastructure::db::build_pool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresSessionRepository,
    PostgresTaskRelationRepository, PostgresTaskRepository, PostgresUserIdentityRepository,
    PostgresUserRepository, RedisSessionRepository,
};
use infrastructure::Argon2PasswordHasher;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::error::ApiError;
use crate::openapi::ApiDoc;

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);

    // Fail fast if the primary datastore is missing or unreachable: a server
    // without its database has no meaningful degraded mode. `build_pool` also
    // checks one connection, so an unreachable Postgres panics here at startup
    // rather than on the first request.
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set to run minerva-server");
    let pool = build_pool(&database_url);

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
    // Sessions are the swappable storage: Redis when REDIS_URL is configured,
    // Postgres otherwise. A missing REDIS_URL is not an error — it just means
    // "use Postgres for sessions" (see docs/architecture.md). A present but
    // unreachable one fails fast, like DATABASE_URL does.
    let sessions: web::Data<dyn SessionRepository> = match std::env::var("REDIS_URL") {
        Ok(redis_url) => {
            println!("using Redis for session storage ({redis_url})");
            let repo: Arc<dyn SessionRepository> = Arc::new(
                RedisSessionRepository::connect(&redis_url)
                    .expect("REDIS_URL is set but could not connect to Redis"),
            );
            repo.into()
        }
        Err(_) => {
            println!("REDIS_URL not set; using Postgres for session storage");
            let repo: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool));
            repo.into()
        }
    };
    // The password hasher holds no state; it is registered like the
    // repositories so handlers name their dependency in their signature.
    let password_hasher = web::Data::new(Argon2PasswordHasher);

    println!("minerva-server listening on 0.0.0.0:{port}");

    HttpServer::new(move || {
        let openapi = ApiDoc::openapi();
        App::new()
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
            .route("/health", web::get().to(health))
            // The real API surface, built endpoint-group by endpoint-group.
            .service(
                web::scope("/api")
                    // Malformed or unparsable JSON bodies get the standard
                    // error envelope instead of Actix's default plaintext.
                    .app_data(
                        web::JsonConfig::default().error_handler(|err, _req| {
                            ApiError::bad_request(format!("invalid JSON body: {err}")).into()
                        }),
                    )
                    .route("/goals", web::post().to(goals::create_goal))
                    .route("/goals", web::get().to(goals::list_goals))
                    .route("/goals/{id}", web::get().to(goals::get_goal))
                    .route("/goals/{id}", web::put().to(goals::update_goal))
                    .route("/goals/{id}", web::delete().to(goals::delete_goal))
                    .route("/milestones", web::post().to(milestones::create_milestone))
                    .route("/milestones", web::get().to(milestones::list_milestones))
                    .route("/milestones/{id}", web::get().to(milestones::get_milestone))
                    .route("/milestones/{id}", web::put().to(milestones::update_milestone))
                    .route("/milestones/{id}", web::delete().to(milestones::delete_milestone))
                    .route("/tasks", web::post().to(tasks::create_task))
                    .route("/tasks", web::get().to(tasks::list_tasks))
                    .route("/tasks/{id}", web::get().to(tasks::get_task))
                    .route("/tasks/{id}", web::put().to(tasks::update_task))
                    .route("/tasks/{id}", web::delete().to(tasks::delete_task))
                    .route("/auth/signup", web::post().to(auth::signup))
                    .route("/auth/login", web::post().to(auth::login))
                    .route("/auth/logout", web::post().to(auth::logout))
                    .route("/auth/me", web::get().to(auth::me)),
            )
            // TEMPORARY: verifies repository wiring end-to-end; unauthenticated
            // and not meant to ship. Remove this scope before /debug is a real API.
            .service(
                web::scope("/debug")
                    .route("/task-relations", web::get().to(debug::list_task_relations))
                    .route("/progress-snapshots", web::get().to(debug::list_progress_snapshots))
                    .route("/goal-milestones", web::get().to(debug::list_goal_milestones)),
            )
            // API documentation (not part of the /api surface): a Swagger UI
            // rendering the generated OpenAPI 3 document, plus the raw JSON at
            // /api-docs/openapi.json (registered by `.url`). Unauthenticated
            // like the rest of the server until auth lands in Milestone 3.
            .service(
                SwaggerUi::new("/api-docs/swagger-ui/{_:.*}")
                    .url("/api-docs/openapi.json", openapi),
            )
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
