//! Goals HTTP API: the `/api/goals` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Goal`]: the wire
//! format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{web, HttpResponse};
use application::ports::GoalRepository;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Goal, GoalId, GoalStatus, Status};
use infrastructure::repositories::PostgresGoalRepository;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{repo_error_response, ApiError};
use crate::openapi::GoalStatusDoc;

/// JSON shape of a goal in responses.
#[derive(Serialize, ToSchema)]
pub struct GoalResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    #[schema(value_type = GoalStatusDoc)]
    pub status: GoalStatus,
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Goal> for GoalResponse {
    fn from(goal: &Goal) -> Self {
        Self {
            id: goal.id.0,
            title: goal.title.clone(),
            description: goal.description.clone(),
            status: goal.status,
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

/// `POST /api/goals` — create a goal. 201 with the created goal; 400 if the
/// title is missing or blank.
#[utoipa::path(
    post,
    path = "/api/goals",
    tags = ["goals"],
    request_body = GoalRequest,
    responses(
        (status = 201, description = "Goal created", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError)
    )
)]
pub async fn create_goal(
    goals: web::Data<PostgresGoalRepository>,
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
        status: GoalStatus::computed(Status::OnTrack),
        target_date: body.target_date,
        created_at: now,
        updated_at: now,
    };
    match goals.create(goal).await {
        Ok(goal) => Ok(HttpResponse::Created().json(GoalResponse::from(&goal))),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/goals` — every goal.
#[utoipa::path(
    get,
    path = "/api/goals",
    tags = ["goals"],
    responses((status = 200, description = "All goals", body = Vec<GoalResponse>))
)]
pub async fn list_goals(
    goals: web::Data<PostgresGoalRepository>,
) -> Result<HttpResponse, ApiError> {
    match goals.list().await {
        Ok(goals) => Ok(HttpResponse::Ok()
            .json(goals.iter().map(GoalResponse::from).collect::<Vec<_>>())),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/goals/{id}` — one goal, or 404.
#[utoipa::path(
    get,
    path = "/api/goals/{id}",
    tags = ["goals"],
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The goal", body = GoalResponse),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn get_goal(
    goals: web::Data<PostgresGoalRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.find_by_id(GoalId(*path)).await {
        Ok(Some(goal)) => Ok(HttpResponse::Ok().json(GoalResponse::from(&goal))),
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `PUT /api/goals/{id}` — replace a goal's fields. The stored status and
/// created_at are preserved; updated_at is refreshed. 404 if the goal is gone.
#[utoipa::path(
    put,
    path = "/api/goals/{id}",
    tags = ["goals"],
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = GoalRequest,
    responses(
        (status = 200, description = "The updated goal", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn update_goal(
    goals: web::Data<PostgresGoalRepository>,
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

/// `DELETE /api/goals/{id}` — remove a goal. 204 on success, 404 if missing.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}",
    tags = ["goals"],
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 204, description = "Goal deleted"),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn delete_goal(
    goals: web::Data<PostgresGoalRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.delete(GoalId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}
