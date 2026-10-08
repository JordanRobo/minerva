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
use crate::{
    account_links, auth, debug, goal_milestones, goals, invites, milestones, redirect, sso_rules,
    task_relations, tasks, users,
};

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
                .route(
                    "/goals/{id}/status-override",
                    web::put().to(goals::set_goal_status_override),
                )
                .route(
                    "/goals/{id}/status-override",
                    web::delete().to(goals::clear_goal_status_override),
                )
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
                .route(
                    "/milestones/{id}/status-override",
                    web::put().to(milestones::set_milestone_status_override),
                )
                .route(
                    "/milestones/{id}/status-override",
                    web::delete().to(milestones::clear_milestone_status_override),
                )
                // Goal–milestone linkage (roadmap 3.4): one module owns both
                // sides of the relation.
                .route(
                    "/goals/{goal_id}/milestones/{milestone_id}",
                    web::put().to(goal_milestones::link_goal_milestone),
                )
                .route(
                    "/goals/{goal_id}/milestones/{milestone_id}",
                    web::delete().to(goal_milestones::unlink_goal_milestone),
                )
                .route(
                    "/goals/{id}/milestones",
                    web::get().to(goal_milestones::list_goal_milestones),
                )
                .route(
                    "/milestones/{id}/goals",
                    web::get().to(goal_milestones::list_milestone_goals),
                )
                .route("/tasks", web::post().to(tasks::create_task))
                .route("/tasks", web::get().to(tasks::list_tasks))
                .route("/tasks/{id}", web::get().to(tasks::get_task))
                .route("/tasks/{id}", web::put().to(tasks::update_task))
                .route("/tasks/{id}", web::delete().to(tasks::delete_task))
                // Board-state transitions (roadmap 3.7): the drag-and-drop
                // column move, changing only the status column.
                .route(
                    "/tasks/{id}/status",
                    web::patch().to(tasks::set_task_status),
                )
                // Task relations (roadmap 3.6): one module owns the relation
                // endpoints.
                .route(
                    "/tasks/{id}/relations",
                    web::post().to(task_relations::create_task_relation),
                )
                .route(
                    "/tasks/{id}/relations",
                    web::get().to(task_relations::list_task_relations),
                )
                .route(
                    "/tasks/{id}/relations/{relation_id}",
                    web::delete().to(task_relations::delete_task_relation),
                )
                .route("/users", web::get().to(users::list_users))
                .route("/users/{id}/role", web::put().to(users::change_user_role))
                .route(
                    "/users/{id}/deactivate",
                    web::post().to(users::deactivate_user),
                )
                .route(
                    "/users/{id}/reactivate",
                    web::post().to(users::reactivate_user),
                )
                .route(
                    "/users/{id}/password-reset",
                    web::post().to(users::create_password_reset),
                )
                .route("/invites", web::post().to(invites::create_invite))
                .route("/invites", web::get().to(invites::list_invites))
                .route(
                    "/invites/{id}/revoke",
                    web::post().to(invites::revoke_invite),
                )
                .route(
                    "/invites/{id}/reissue",
                    web::post().to(invites::reissue_invite),
                )
                .route(
                    "/sso/group-rules",
                    web::get().to(sso_rules::list_group_rules),
                )
                .route(
                    "/sso/group-rules",
                    web::post().to(sso_rules::create_group_rule),
                )
                .route(
                    "/sso/group-rules/{id}",
                    web::put().to(sso_rules::update_group_rule),
                )
                .route(
                    "/sso/group-rules/{id}",
                    web::delete().to(sso_rules::delete_group_rule),
                )
                .route("/auth/login", web::post().to(auth::login))
                .route("/auth/logout", web::post().to(auth::logout))
                .route("/auth/me", web::get().to(auth::me))
                .route("/auth/providers", web::get().to(auth::list_auth_providers))
                .route(
                    "/auth/tokens/inspect",
                    web::post().to(account_links::inspect_token),
                )
                .route(
                    "/auth/accept-invite",
                    web::post().to(account_links::accept_invite),
                )
                .route(
                    "/auth/reset-password",
                    web::post().to(account_links::reset_password),
                )
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
