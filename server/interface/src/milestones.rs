//! Milestones HTTP API: the `/api/milestones` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Milestone`]: the
//! wire format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{HttpResponse, web};
use application::ports::MilestoneRepository;
use application::status_override::StatusOverrideService;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Milestone, MilestoneId, Status, StatusSource};
use infrastructure::repositories::PostgresMilestoneRepository;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, repo_error_response, status_override_error_response};
use crate::goals::StatusOverrideRequest;
use crate::openapi::{StatusDoc, StatusSourceDoc};

/// JSON shape of a milestone in responses.
#[derive(Serialize, ToSchema)]
pub struct MilestoneResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// The status as it stands now: the manual setting if one is active,
    /// otherwise the automatic value worked out from its tasks and target date.
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Whether [`MilestoneResponse::status`] was set by hand ("manual") or
    /// worked out by the system ("automatic").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Milestone> for MilestoneResponse {
    fn from(milestone: &Milestone) -> Self {
        // The wire shape reports the effective status and where it came
        // from; the raw automatic value and override are domain-side.
        Self {
            id: milestone.id.0,
            title: milestone.title.clone(),
            description: milestone.description.clone(),
            status: milestone.effective_status(),
            status_source: milestone.status_source(),
            target_date: milestone.target_date,
            created_at: milestone.created_at,
            updated_at: milestone.updated_at,
        }
    }
}

/// Body for `POST /api/milestones` and `PUT /api/milestones/{id}`. The id,
/// status, and timestamps are server-managed and never accepted from the
/// client.
#[derive(Deserialize, ToSchema)]
pub struct MilestoneRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// Create Milestone
///
/// Create a new milestone - a measurable step toward a goal.
/// Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/milestones",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    request_body = MilestoneRequest,
    responses(
        (status = 201, description = "Milestone created", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError)
    )
)]
pub async fn create_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: EditAccess,
    body: web::Json<MilestoneRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let now = Utc::now();
    let milestone = Milestone {
        id: MilestoneId::new(),
        title: body.title.clone(),
        description: body.description.clone(),
        status: Status::OnTrack,
        status_override: None,
        target_date: body.target_date,
        created_at: now,
        updated_at: now,
    };
    match milestones.create(milestone).await {
        Ok(milestone) => Ok(HttpResponse::Created().json(MilestoneResponse::from(&milestone))),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// List all Milestones
///
/// List every milestone, across all statuses. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/milestones",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "All milestones", body = Vec<MilestoneResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_milestones(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    match milestones.list().await {
        Ok(milestones) => Ok(HttpResponse::Ok().json(
            milestones
                .iter()
                .map(MilestoneResponse::from)
                .collect::<Vec<_>>(),
        )),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Get Individual Milestone
///
/// Fetch a single milestone by its ID. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The milestone", body = MilestoneResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn get_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.find_by_id(MilestoneId(*path)).await {
        Ok(Some(milestone)) => Ok(HttpResponse::Ok().json(MilestoneResponse::from(&milestone))),
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Update Milestone
///
/// Update a milestone's title, description, or target date. Status and timestamps are managed by the server.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = MilestoneRequest,
    responses(
        (status = 200, description = "The updated milestone", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn update_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<MilestoneRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = MilestoneId(*path);
    match milestones.find_by_id(id).await {
        Ok(Some(existing)) => {
            let updated = Milestone {
                title: body.title.clone(),
                description: body.description.clone(),
                target_date: body.target_date,
                status: existing.status,
                created_at: existing.created_at,
                updated_at: Utc::now(),
                ..existing
            };
            match milestones.update(updated).await {
                Ok(milestone) => Ok(HttpResponse::Ok().json(MilestoneResponse::from(&milestone))),
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Delete Milestone
///
/// Delete a milestone. Tasks assigned to it become unassigned rather than being deleted.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 204, description = "Milestone deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn delete_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.delete(MilestoneId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Set Milestone Status Override
///
/// Manually set a milestone's status. The manual setting is sticky: it holds
/// until cleared, even if the milestone's tasks or target date change in the
/// meantime.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}/status-override",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = StatusOverrideRequest,
    responses(
        (status = 200, description = "The milestone with its new status", body = MilestoneResponse),
        (status = 400, description = "Unknown status value", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn set_milestone_status_override(
    overrides: web::Data<StatusOverrideService>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<StatusOverrideRequest>,
) -> Result<HttpResponse, ApiError> {
    match overrides
        .set_milestone_override(MilestoneId(*path), body.status)
        .await
    {
        Ok(milestone) => Ok(HttpResponse::Ok().json(MilestoneResponse::from(&milestone))),
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Clear Milestone Status Override
///
/// Remove a milestone's manual status setting so it returns to automatic —
/// the status worked out from its tasks and target date.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}/status-override",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The milestone with its automatic status", body = MilestoneResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn clear_milestone_status_override(
    overrides: web::Data<StatusOverrideService>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match overrides.clear_milestone_override(MilestoneId(*path)).await {
        Ok(milestone) => Ok(HttpResponse::Ok().json(MilestoneResponse::from(&milestone))),
        Err(err) => Err(status_override_error_response(err)),
    }
}
