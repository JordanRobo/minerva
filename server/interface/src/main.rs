mod debug;

use actix_web::{web, App, HttpResponse, HttpServer};
use infrastructure::db::build_pool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresProgressSnapshotRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
};

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

    // One shared pool, six repositories. Each is registered as its own
    // `web::Data` rather than wrapped in a single AppState struct: every
    // handler uses exactly one repository, so per-repo Data keeps each
    // handler's signature naming only the repo it actually calls. (The pool
    // clones are cheap — r2d2 pools share their connections behind an Arc.)
    let goals = web::Data::new(PostgresGoalRepository::new(pool.clone()));
    let milestones = web::Data::new(PostgresMilestoneRepository::new(pool.clone()));
    let goal_milestones = web::Data::new(PostgresGoalMilestoneRepository::new(pool.clone()));
    let tasks = web::Data::new(PostgresTaskRepository::new(pool.clone()));
    let task_relations = web::Data::new(PostgresTaskRelationRepository::new(pool.clone()));
    let progress_snapshots = web::Data::new(PostgresProgressSnapshotRepository::new(pool));

    println!("minerva-server listening on 0.0.0.0:{port}");

    HttpServer::new(move || {
        App::new()
            .app_data(goals.clone())
            .app_data(milestones.clone())
            .app_data(goal_milestones.clone())
            .app_data(tasks.clone())
            .app_data(task_relations.clone())
            .app_data(progress_snapshots.clone())
            .route("/health", web::get().to(health))
            // TEMPORARY: verifies repository wiring end-to-end; unauthenticated
            // and not meant to ship. Remove this scope before /debug is a real API.
            .service(
                web::scope("/debug")
                    .route("/goals", web::get().to(debug::list_goals))
                    .route("/milestones", web::get().to(debug::list_milestones))
                    .route("/tasks", web::get().to(debug::list_tasks))
                    .route("/task-relations", web::get().to(debug::list_task_relations))
                    .route("/progress-snapshots", web::get().to(debug::list_progress_snapshots))
                    .route("/goal-milestones", web::get().to(debug::list_goal_milestones)),
            )
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
