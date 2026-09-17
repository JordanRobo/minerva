//! Milestones HTTP API: the `/api/milestones` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Milestone`]: the
//! wire format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{web, HttpResponse};
use application::ports::MilestoneRepository;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{GoalStatus, Milestone, MilestoneId, Status};
use infrastructure::repositories::PostgresMilestoneRepository;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{repo_error_response, ApiError};
use crate::openapi::GoalStatusDoc;

/// JSON shape of a milestone in responses.
#[derive(Serialize, ToSchema)]
pub struct MilestoneResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    #[schema(value_type = GoalStatusDoc)]
    pub status: GoalStatus,
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Milestone> for MilestoneResponse {
    fn from(milestone: &Milestone) -> Self {
        Self {
            id: milestone.id.0,
            title: milestone.title.clone(),
            description: milestone.description.clone(),
            status: milestone.status,
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

/// `POST /api/milestones` — create a milestone. 201 with the created
/// milestone; 400 if the title is missing or blank.
#[utoipa::path(
    post,
    path = "/api/milestones",
    tags = ["milestones"],
    request_body = MilestoneRequest,
    responses(
        (status = 201, description = "Milestone created", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError)
    )
)]
pub async fn create_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
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
        status: GoalStatus::computed(Status::OnTrack),
        target_date: body.target_date,
        created_at: now,
        updated_at: now,
    };
    match milestones.create(milestone).await {
        Ok(milestone) => Ok(HttpResponse::Created().json(MilestoneResponse::from(&milestone))),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/milestones` — every milestone.
#[utoipa::path(
    get,
    path = "/api/milestones",
    tags = ["milestones"],
    responses((status = 200, description = "All milestones", body = Vec<MilestoneResponse>))
)]
pub async fn list_milestones(
    milestones: web::Data<PostgresMilestoneRepository>,
) -> Result<HttpResponse, ApiError> {
    match milestones.list().await {
        Ok(milestones) => Ok(HttpResponse::Ok()
            .json(milestones.iter().map(MilestoneResponse::from).collect::<Vec<_>>())),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/milestones/{id}` — one milestone, or 404.
#[utoipa::path(
    get,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The milestone", body = MilestoneResponse),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn get_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.find_by_id(MilestoneId(*path)).await {
        Ok(Some(milestone)) => Ok(HttpResponse::Ok().json(MilestoneResponse::from(&milestone))),
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `PUT /api/milestones/{id}` — replace a milestone's fields. The stored
/// status and created_at are preserved; updated_at is refreshed. 404 if the
/// milestone is gone.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = MilestoneRequest,
    responses(
        (status = 200, description = "The updated milestone", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn update_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
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

/// `DELETE /api/milestones/{id}` — remove a milestone. 204 on success, 404 if missing.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 204, description = "Milestone deleted"),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn delete_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.delete(MilestoneId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}
