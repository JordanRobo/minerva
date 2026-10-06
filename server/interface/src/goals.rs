//! Goals HTTP API: the `/api/goals` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Goal`]: the wire
//! format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{HttpResponse, web};
use application::ports::GoalRepository;
use application::status_override::StatusOverrideService;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Goal, GoalId, Status, StatusSource};
use infrastructure::repositories::PostgresGoalRepository;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, repo_error_response, status_override_error_response};
use crate::openapi::{StatusDoc, StatusSourceDoc};

/// JSON shape of a goal in responses.
#[derive(Serialize, ToSchema)]
pub struct GoalResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// The status as it stands now: the manual setting if one is active,
    /// otherwise the automatic value worked out from the milestones.
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Whether [`GoalResponse::status`] was set by hand ("manual") or worked
    /// out by the system ("automatic").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Goal> for GoalResponse {
    fn from(goal: &Goal) -> Self {
        // The wire shape reports the effective status and where it came
        // from; the raw automatic value and override are domain-side.
        Self {
            id: goal.id.0,
            title: goal.title.clone(),
            description: goal.description.clone(),
            status: goal.effective_status(),
            status_source: goal.status_source(),
            target_date: goal.target_date,
            created_at: goal.created_at,
            updated_at: goal.updated_at,
        }
    }
}

/// Body for `POST /api/goals` and `PUT /api/goals/{id}`. The id, status, and
/// timestamps are server-managed and never accepted from the client.
#[derive(Deserialize, ToSchema)]
pub struct GoalRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// Body for `PUT /api/goals/{id}/status-override` and
/// `PUT /api/milestones/{id}/status-override`: the status to set by hand. An
/// unknown status string is a 400, rejected by JSON parsing before the
/// handler runs.
#[derive(Deserialize, ToSchema)]
pub struct StatusOverrideRequest {
    #[schema(value_type = StatusDoc)]
    pub status: Status,
}

/// Create Goal
///
/// Create a new goal - a strategic outcome the school is working toward.
/// Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/goals",
    tags = ["goals"],
    security(("session_cookie" = [])),
    request_body = GoalRequest,
    responses(
        (status = 201, description = "Goal created", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError)
    )
)]
pub async fn create_goal(
    goals: web::Data<PostgresGoalRepository>,
    _access: EditAccess,
    body: web::Json<GoalRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let now = Utc::now();
    let goal = Goal {
        id: GoalId::new(),
        title: body.title.clone(),
        description: body.description.clone(),
        // A new goal has no milestones yet, so the rollup rule reports OnTrack.
        status: Status::OnTrack,
        status_override: None,
        target_date: body.target_date,
        created_at: now,
        updated_at: now,
    };
    match goals.create(goal).await {
        Ok(goal) => Ok(HttpResponse::Created().json(GoalResponse::from(&goal))),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// List Goals
///
/// List every goal, across all statuses. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/goals",
    tags = ["goals"],
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "All goals", body = Vec<GoalResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_goals(
    goals: web::Data<PostgresGoalRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    match goals.list().await {
        Ok(goals) => {
            Ok(HttpResponse::Ok().json(goals.iter().map(GoalResponse::from).collect::<Vec<_>>()))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Get Individual Goal
///
/// Fetch a single goal by its ID. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The goal", body = GoalResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn get_goal(
    goals: web::Data<PostgresGoalRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.find_by_id(GoalId(*path)).await {
        Ok(Some(goal)) => Ok(HttpResponse::Ok().json(GoalResponse::from(&goal))),
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Update Goal
///
/// Update a goal's title, description, or target date. Status and timestamps are managed by the server.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = GoalRequest,
    responses(
        (status = 200, description = "The updated goal", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn update_goal(
    goals: web::Data<PostgresGoalRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<GoalRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = GoalId(*path);
    match goals.find_by_id(id).await {
        Ok(Some(existing)) => {
            let updated = Goal {
                title: body.title.clone(),
                description: body.description.clone(),
                target_date: body.target_date,
                status: existing.status,
                created_at: existing.created_at,
                updated_at: Utc::now(),
                ..existing
            };
            match goals.update(updated).await {
                Ok(goal) => Ok(HttpResponse::Ok().json(GoalResponse::from(&goal))),
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Delete Goal
///
/// Delete a goal, including its links to any milestones.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 204, description = "Goal deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn delete_goal(
    goals: web::Data<PostgresGoalRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.delete(GoalId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Set Goal Status Override
///
/// Manually set a goal's status. The manual setting is sticky: it holds until
/// cleared, even if the goal's milestones change in the meantime.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{id}/status-override",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = StatusOverrideRequest,
    responses(
        (status = 200, description = "The goal with its new status", body = GoalResponse),
        (status = 400, description = "Unknown status value", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn set_goal_status_override(
    overrides: web::Data<StatusOverrideService>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<StatusOverrideRequest>,
) -> Result<HttpResponse, ApiError> {
    match overrides
        .set_goal_override(GoalId(*path), body.status)
        .await
    {
        Ok(goal) => Ok(HttpResponse::Ok().json(GoalResponse::from(&goal))),
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Clear Goal Status Override
///
/// Remove a goal's manual status setting so it returns to automatic — the
/// status worked out from its milestones.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}/status-override",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The goal with its automatic status", body = GoalResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn clear_goal_status_override(
    overrides: web::Data<StatusOverrideService>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match overrides.clear_goal_override(GoalId(*path)).await {
        Ok(goal) => Ok(HttpResponse::Ok().json(GoalResponse::from(&goal))),
        Err(err) => Err(status_override_error_response(err)),
    }
}
