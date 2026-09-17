//! Tasks HTTP API: the `/api/tasks` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Task`]: the wire
//! format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{web, HttpResponse};
use application::ports::TaskRepository;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{MilestoneId, Task, TaskId, TaskStatus};
use infrastructure::repositories::PostgresTaskRepository;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{repo_error_response, ApiError};
use crate::openapi::TaskStatusDoc;

/// JSON shape of a task in responses.
#[derive(Serialize, ToSchema)]
pub struct TaskResponse {
    pub id: Uuid,
    pub milestone_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    #[schema(value_type = TaskStatusDoc)]
    pub status: TaskStatus,
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Task> for TaskResponse {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.0,
            milestone_id: task.milestone_id.map(|id| id.0),
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status,
            target_date: task.target_date,
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

/// Body for `POST /api/tasks` and `PUT /api/tasks/{id}`. The id and
/// timestamps are server-managed and never accepted from the client; a
/// missing or null `milestone_id` leaves the task unassigned.
#[derive(Deserialize, ToSchema)]
pub struct TaskRequest {
    #[serde(default)]
    pub milestone_id: Option<Uuid>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[schema(value_type = TaskStatusDoc)]
    pub status: TaskStatus,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// Query params for `GET /api/tasks`: exactly one of `milestone_id` or
/// `unassigned=true` selects which list to return.
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct TaskListQuery {
    /// Filter to the tasks assigned to this milestone.
    pub milestone_id: Option<Uuid>,
    /// When `true`, filter to the tasks not assigned to any milestone.
    pub unassigned: Option<bool>,
}

/// `POST /api/tasks` — create a task. 201 with the created task; 400 if the
/// title is missing or blank.
#[utoipa::path(
    post,
    path = "/api/tasks",
    tags = ["tasks"],
    request_body = TaskRequest,
    responses(
        (status = 201, description = "Task created", body = TaskResponse),
        (
            status = 400,
            description = "Title is missing or blank, or milestone_id does not reference an existing milestone",
            body = ApiError
        )
    )
)]
pub async fn create_task(
    tasks: web::Data<PostgresTaskRepository>,
    body: web::Json<TaskRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let now = Utc::now();
    let task = Task {
        id: TaskId::new(),
        milestone_id: body.milestone_id.map(MilestoneId),
        title: body.title.clone(),
        description: body.description.clone(),
        status: body.status,
        target_date: body.target_date,
        created_at: now,
        updated_at: now,
    };
    match tasks.create(task).await {
        Ok(task) => Ok(HttpResponse::Created().json(TaskResponse::from(&task))),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/tasks?milestone_id={id}` — the tasks for one milestone, or
/// `GET /api/tasks?unassigned=true` — the tasks with no milestone. 400 if
/// neither (or both) is supplied: the repository has no "list every task"
/// read, so there is nothing else to return.
#[utoipa::path(
    get,
    path = "/api/tasks",
    tags = ["tasks"],
    params(TaskListQuery),
    responses(
        (status = 200, description = "The matching tasks", body = Vec<TaskResponse>),
        (
            status = 400,
            description = "Neither milestone_id nor unassigned=true supplied, or both",
            body = ApiError
        )
    )
)]
pub async fn list_tasks(
    query: web::Query<TaskListQuery>,
    tasks: web::Data<PostgresTaskRepository>,
) -> Result<HttpResponse, ApiError> {
    let listed = match (query.milestone_id, query.unassigned) {
        (Some(milestone_id), None) => tasks.list_by_milestone(MilestoneId(milestone_id)).await,
        (None, Some(true)) => tasks.list_unassigned().await,
        _ => {
            return Err(ApiError::bad_request(
                "provide exactly one of milestone_id or unassigned=true",
            ))
        }
    };
    match listed {
        Ok(tasks) => Ok(HttpResponse::Ok()
            .json(tasks.iter().map(TaskResponse::from).collect::<Vec<_>>())),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `GET /api/tasks/{id}` — one task, or 404.
#[utoipa::path(
    get,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 200, description = "The task", body = TaskResponse),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn get_task(
    tasks: web::Data<PostgresTaskRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match tasks.find_by_id(TaskId(*path)).await {
        Ok(Some(task)) => Ok(HttpResponse::Ok().json(TaskResponse::from(&task))),
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `PUT /api/tasks/{id}` — replace a task's fields, including reassigning it
/// to another milestone or unassigning it (`milestone_id` null). The stored
/// id and created_at are preserved; updated_at is refreshed. 404 if the task
/// is gone.
#[utoipa::path(
    put,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = TaskRequest,
    responses(
        (status = 200, description = "The updated task", body = TaskResponse),
        (
            status = 400,
            description = "Title is missing or blank, or milestone_id does not reference an existing milestone",
            body = ApiError
        ),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn update_task(
    tasks: web::Data<PostgresTaskRepository>,
    path: web::Path<Uuid>,
    body: web::Json<TaskRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = TaskId(*path);
    match tasks.find_by_id(id).await {
        Ok(Some(existing)) => {
            let updated = Task {
                milestone_id: body.milestone_id.map(MilestoneId),
                title: body.title.clone(),
                description: body.description.clone(),
                status: body.status,
                target_date: body.target_date,
                created_at: existing.created_at,
                updated_at: Utc::now(),
                ..existing
            };
            match tasks.update(updated).await {
                Ok(task) => Ok(HttpResponse::Ok().json(TaskResponse::from(&task))),
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// `DELETE /api/tasks/{id}` — remove a task. 204 on success, 404 if missing.
#[utoipa::path(
    delete,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 204, description = "Task deleted"),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn delete_task(
    tasks: web::Data<PostgresTaskRepository>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match tasks.delete(TaskId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_round_trips_as_the_db_snake_case_strings() {
        for (json, status) in [
            ("\"backlog\"", TaskStatus::Backlog),
            ("\"to_do\"", TaskStatus::ToDo),
            ("\"in_progress\"", TaskStatus::InProgress),
            ("\"done\"", TaskStatus::Done),
        ] {
            let parsed: TaskStatus = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, status);
            assert_eq!(serde_json::to_string(&status).unwrap(), json);
        }
    }

    #[test]
    fn task_request_optional_fields_default_to_none() {
        let body: TaskRequest = serde_json::from_str(r#"{"title": "t", "status": "backlog"}"#)
            .unwrap();
        assert_eq!(body.milestone_id, None);
        assert_eq!(body.description, None);
        assert_eq!(body.target_date, None);
    }
}
