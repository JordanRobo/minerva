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
use uuid::Uuid;

use crate::error::{bad_request, not_found, repo_error_response};

/// JSON shape of a goal in responses.
#[derive(Serialize)]
pub struct GoalResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
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
#[derive(Deserialize)]
pub struct GoalRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// `POST /api/goals` — create a goal. 201 with the created goal; 400 if the
/// title is missing or blank.
pub async fn create_goal(
    goals: web::Data<PostgresGoalRepository>,
    body: web::Json<GoalRequest>,
) -> HttpResponse {
    if body.title.trim().is_empty() {
        return bad_request("title must not be empty");
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
        Ok(goal) => HttpResponse::Created().json(GoalResponse::from(&goal)),
        Err(err) => repo_error_response(err),
    }
}

/// `GET /api/goals` — every goal.
pub async fn list_goals(goals: web::Data<PostgresGoalRepository>) -> HttpResponse {
    match goals.list().await {
        Ok(goals) => HttpResponse::Ok()
            .json(goals.iter().map(GoalResponse::from).collect::<Vec<_>>()),
        Err(err) => repo_error_response(err),
    }
}

/// `GET /api/goals/{id}` — one goal, or 404.
pub async fn get_goal(
    goals: web::Data<PostgresGoalRepository>,
    path: web::Path<Uuid>,
) -> HttpResponse {
    match goals.find_by_id(GoalId(*path)).await {
        Ok(Some(goal)) => HttpResponse::Ok().json(GoalResponse::from(&goal)),
        Ok(None) => not_found(),
        Err(err) => repo_error_response(err),
    }
}

/// `PUT /api/goals/{id}` — replace a goal's fields. The stored status and
/// created_at are preserved; updated_at is refreshed. 404 if the goal is gone.
pub async fn update_goal(
    goals: web::Data<PostgresGoalRepository>,
    path: web::Path<Uuid>,
    body: web::Json<GoalRequest>,
) -> HttpResponse {
    if body.title.trim().is_empty() {
        return bad_request("title must not be empty");
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
                Ok(goal) => HttpResponse::Ok().json(GoalResponse::from(&goal)),
                Err(err) => repo_error_response(err),
            }
        }
        Ok(None) => not_found(),
        Err(err) => repo_error_response(err),
    }
}

/// `DELETE /api/goals/{id}` — remove a goal. 204 on success, 404 if missing.
pub async fn delete_goal(
    goals: web::Data<PostgresGoalRepository>,
    path: web::Path<Uuid>,
) -> HttpResponse {
    match goals.delete(GoalId(*path)).await {
        Ok(()) => HttpResponse::NoContent().finish(),
        Err(err) => repo_error_response(err),
    }
}
