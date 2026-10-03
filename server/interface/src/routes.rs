//! The route table: every route the server serves, in one place. `main` and
//! the tests both build their apps through [`configure`], so a test that
//! passes exercises the real routes — adding a route here without an access
//! extractor (or an allowlist entry in the access tests) is a conscious
//! choice, not an accident.

use actix_web::{HttpResponse, web};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::error::ApiError;
use crate::openapi::ApiDoc;
use crate::{auth, debug, goals, milestones, redirect, tasks};

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

/// Register every route on the app.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health))
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
                .route("/auth/providers", web::get().to(auth::list_auth_providers))
                .route(
                    "/auth/{provider}/login",
                    web::get().to(redirect::redirect_login),
                )
                .route(
                    "/auth/{provider}/callback",
                    web::get().to(redirect::redirect_callback),
                ),
        )
        // TEMPORARY: verifies repository wiring end-to-end; locked down to
        // Admin until roadmap 3.4/3.6 provide real endpoints (removal is 8.5).
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
        // /api-docs/openapi.json (registered by `.url`). Unauthenticated on
        // purpose — it documents the API; documenting its auth requirements
        // is roadmap 2.4 step 3.
        .service(
            SwaggerUi::new("/api-docs/swagger-ui/{_:.*}")
                .url("/api-docs/openapi.json", ApiDoc::openapi()),
        );
}
