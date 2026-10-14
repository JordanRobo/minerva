//! Tasks HTTP API: the `/api/tasks` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Task`]: the wire
//! format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{HttpResponse, web};
use application::assignment::AssignmentService;
use application::pagination::{PageRequest, PageRequestError};
use application::ports::{TaskListFilter, TaskRepository, UserRepository};
use application::task_list::TaskListService;
use application::task_status::TaskStatusService;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{MilestoneId, Task, TaskId, TaskStatus, UserId};
use infrastructure::repositories::PostgresTaskRepository;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{
    ApiError, assignment_error_response, repo_error_response, task_status_error_response,
};
use crate::openapi::TaskStatusDoc;
use crate::people::{PersonSummary, resolve_summaries};

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
    /// The person the task is assigned to, or null when unassigned (D17). A
    /// deactivated assignee still appears, with `active` false.
    pub assignee: Option<PersonSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Build the response for one task, looking its assignee up in the batch
/// resolved by [`resolve_summaries`] — one user query per response or page,
/// never one per row (D17).
fn task_response(task: &Task, people: &HashMap<Uuid, PersonSummary>) -> TaskResponse {
    let mut response = TaskResponse {
        id: task.id.0,
        milestone_id: task.milestone_id.map(|id| id.0),
        title: task.title.clone(),
        description: task.description.clone(),
        status: task.status,
        target_date: task.target_date,
        assignee: None,
        created_at: task.created_at,
        updated_at: task.updated_at,
    };
    if let Some(assignee_id) = task.assignee_id {
        response.assignee = people.get(&assignee_id.0).cloned();
    }
    response
}

/// Body for `PUT /api/tasks/{id}`. The id and timestamps are server-managed
/// and never accepted from the client; a missing or null `milestone_id`
/// leaves the task unassigned. The assignee is not part of an ordinary
/// update — only creation and the dedicated assignee routes set it (D17).
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

/// Body for `POST /api/tasks`: the update fields plus the optional assignee.
#[derive(Deserialize, ToSchema)]
pub struct CreateTaskRequest {
    #[serde(default)]
    pub milestone_id: Option<Uuid>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[schema(value_type = TaskStatusDoc)]
    pub status: TaskStatus,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
    /// The user to assign the task to; must be an existing active account.
    /// Omitted or null leaves the task unassigned.
    #[serde(default)]
    pub assignee_id: Option<Uuid>,
}

/// Body for `PUT /api/tasks/{id}/assignee`.
#[derive(Deserialize, ToSchema)]
pub struct TaskAssigneeRequest {
    /// The user to assign the task to; must be an existing active account.
    pub assignee_id: Uuid,
}

/// One page of tasks plus the pagination metadata (roadmap 3.10, D16):
/// `total` is the number of tasks matching the filters, ignoring paging, so
/// a client can tell how many pages there are without fetching them all.
#[derive(Serialize, ToSchema)]
pub struct TaskPage {
    pub items: Vec<TaskResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u64,
}

/// Query params for `GET /api/tasks`. Every parameter is optional and narrows
/// the result set; they are all plain strings here because the handler
/// validates each one itself so a 400 can name the offending parameter.
#[derive(IntoParams)]
pub struct TaskListQuery {
    /// How many tasks per page (1–200). Defaults to 50.
    pub limit: Option<String>,
    /// How many matching tasks to skip before the page starts. Defaults to 0.
    pub offset: Option<String>,
    /// Only tasks in these board columns (`backlog`, `to_do`, `in_progress`,
    /// `done`). May be given several times or comma-separated.
    pub status: Vec<String>,
    /// Only tasks assigned to this milestone (a UUID).
    pub milestone_id: Option<String>,
    /// Case-insensitive substring match on the task title.
    pub q: Option<String>,
    /// Only tasks whose target date is on or after this date (`YYYY-MM-DD`).
    pub target_after: Option<String>,
    /// Only tasks whose target date is on or before this date (`YYYY-MM-DD`).
    pub target_before: Option<String>,
    /// Only the tasks assigned to this person (a UUID); an id no task is
    /// assigned to answers an empty page, not an error.
    pub assignee_id: Option<String>,
}

impl<'de> Deserialize<'de> for TaskListQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A derived visitor cannot express what a query string allows:
        // serde_urlencoded hands every key=value through as a scalar (a `Vec`
        // field would be rejected outright) and errors on repeated keys, but
        // `status` must accept both repeats and comma-separated values. So
        // collect the raw pairs and group them here: `status` accumulates,
        // every other parameter may appear at most once, and unknown
        // parameters are ignored — the same leniency the derive had.
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let mut query = TaskListQuery {
            limit: None,
            offset: None,
            status: Vec::new(),
            milestone_id: None,
            q: None,
            target_after: None,
            target_before: None,
            assignee_id: None,
        };
        for (key, value) in pairs {
            match key.as_str() {
                "status" => query.status.push(value),
                "limit" | "offset" | "milestone_id" | "q" | "target_after" | "target_before"
                | "assignee_id" => {
                    let slot: &mut Option<String> = match key.as_str() {
                        "limit" => &mut query.limit,
                        "offset" => &mut query.offset,
                        "milestone_id" => &mut query.milestone_id,
                        "q" => &mut query.q,
                        "target_after" => &mut query.target_after,
                        "target_before" => &mut query.target_before,
                        _ => &mut query.assignee_id,
                    };
                    if slot.is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate parameter {key}"
                        )));
                    }
                    *slot = Some(value);
                }
                _ => {}
            }
        }
        Ok(query)
    }
}

/// Validate and translate the raw query string into the filter and page the
/// service takes. Every failure is a 400 `invalid_query` naming the
/// offending parameter; filters that match nothing are still valid (an empty
/// result, never an error).
fn parse_task_list_query(
    params: &TaskListQuery,
) -> Result<(TaskListFilter, PageRequest), ApiError> {
    let limit = match params.limit.as_deref() {
        Some(raw) => Some(raw.parse::<i32>().map_err(|_| {
            ApiError::invalid_query(format!("limit must be a whole number, got {raw:?}"))
        })?),
        None => None,
    };
    let offset = match params.offset.as_deref() {
        Some(raw) => Some(raw.parse::<i64>().map_err(|_| {
            ApiError::invalid_query(format!("offset must be a whole number, got {raw:?}"))
        })?),
        None => None,
    };
    let page_request = PageRequest::new(limit, offset).map_err(|err| match err {
        PageRequestError::LimitOutOfRange(value) => ApiError::invalid_query(format!(
            "limit must be between 1 and {}, got {value}",
            PageRequest::MAX_PAGE_LIMIT
        )),
        PageRequestError::NegativeOffset(value) => {
            ApiError::invalid_query(format!("offset must not be negative, got {value}"))
        }
    })?;

    let mut statuses = Vec::new();
    for raw in &params.status {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            statuses.push(match token {
                "backlog" => TaskStatus::Backlog,
                "to_do" => TaskStatus::ToDo,
                "in_progress" => TaskStatus::InProgress,
                "done" => TaskStatus::Done,
                other => {
                    return Err(ApiError::invalid_query(format!(
                        "status must be one of backlog, to_do, in_progress or done, got {other:?}"
                    )));
                }
            });
        }
    }

    let milestone_id = params.milestone_id.as_deref().map(|raw| {
        Uuid::parse_str(raw).map(MilestoneId).map_err(|_| {
            ApiError::invalid_query(format!("milestone_id must be a UUID, got {raw:?}"))
        })
    });

    // Trimmed; empty after trimming means "no search", and the length limit
    // applies to what is actually searched.
    let q = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(ToOwned::to_owned);
    let q_len = q.as_deref().map_or(0, |q| q.chars().count());
    if q_len > 100 {
        return Err(ApiError::invalid_query(format!(
            "q must be at most 100 characters, got {q_len}"
        )));
    }

    let target_after = parse_target_date(params.target_after.as_deref(), "target_after")?;
    let target_before = parse_target_date(params.target_before.as_deref(), "target_before")?;

    // A malformed UUID is a 400 naming the parameter; a well-formed one no
    // user has is still valid — an empty page (D16).
    let assignee_id = params.assignee_id.as_deref().map(|raw| {
        Uuid::parse_str(raw).map(UserId).map_err(|_| {
            ApiError::invalid_query(format!("assignee_id must be a UUID, got {raw:?}"))
        })
    });

    Ok((
        TaskListFilter {
            statuses,
            milestone_id: milestone_id.transpose()?,
            q,
            target_after,
            target_before,
            assignee_id: assignee_id.transpose()?,
        },
        page_request,
    ))
}

fn parse_target_date(raw: Option<&str>, param: &str) -> Result<Option<NaiveDate>, ApiError> {
    match raw {
        Some(raw) => Ok(Some(NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(
            |_| {
                ApiError::invalid_query(format!(
                    "{param} must be a date in YYYY-MM-DD form, got {raw:?}"
                ))
            },
        )?)),
        None => Ok(None),
    }
}

/// Create Task
///
/// Create a new task, optionally assigning it to a milestone and a person.
/// Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/tasks",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    request_body = CreateTaskRequest,
    responses(
        (status = 201, description = "Task created", body = TaskResponse),
        (
            status = 400,
            description = "Title is missing or blank, milestone_id does not reference an existing milestone, or assignee_id is not a UUID",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No user with the given assignee_id", body = ApiError),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn create_task(
    tasks: web::Data<PostgresTaskRepository>,
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    body: web::Json<CreateTaskRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    // Validated exactly like the set endpoint (D17): an unknown user 404s,
    // a deactivated one 409s, before anything is written.
    let assignee_id = match body.assignee_id {
        Some(id) => {
            assignment
                .ensure_active_user(UserId(id))
                .await
                .map_err(assignment_error_response)?;
            Some(UserId(id))
        }
        None => None,
    };
    let now = Utc::now();
    let task = Task {
        id: TaskId::new(),
        milestone_id: body.milestone_id.map(MilestoneId),
        title: body.title.clone(),
        description: body.description.clone(),
        status: body.status,
        target_date: body.target_date,
        assignee_id,
        created_at: now,
        updated_at: now,
    };
    match tasks.create(task).await {
        Ok(task) => {
            let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Created().json(task_response(&task, &people)))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// List Tasks
///
/// List tasks, one page at a time. Every query parameter is optional and
/// narrows the result set; together they are combined with AND. The order is
/// fixed: target date ascending (tasks without one last), then creation
/// time, then id — so pages are stable. A filter that matches nothing is an
/// empty page, not an error. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/tasks",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(TaskListQuery),
    responses(
        (status = 200, description = "One page of the matching tasks plus the total count", body = TaskPage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_tasks(
    query: web::Query<TaskListQuery>,
    service: web::Data<TaskListService>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let (filter, page_request) = parse_task_list_query(&query.0)?;
    match service.list_page(&filter, &page_request).await {
        Ok(page) => {
            // One user query for the whole page's assignees (D17).
            let people = resolve_summaries(
                users.as_ref(),
                page.items.iter().map(|task| task.assignee_id),
            )
            .await
            .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(TaskPage {
                items: page
                    .items
                    .iter()
                    .map(|task| task_response(task, &people))
                    .collect(),
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Get Task
///
/// Fetch a single task by its ID. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 200, description = "The task", body = TaskResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn get_task(
    tasks: web::Data<PostgresTaskRepository>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match tasks.find_by_id(TaskId(*path)).await {
        Ok(Some(task)) => {
            let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(task_response(&task, &people)))
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Update Task
///
/// Update a task's fields, including reassigning it to a different milestone or unassigning it.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = TaskRequest,
    responses(
        (status = 200, description = "The updated task", body = TaskResponse),
        (
            status = 400,
            description = "Title is missing or blank, or milestone_id does not reference an existing milestone",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn update_task(
    tasks: web::Data<PostgresTaskRepository>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<TaskRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = TaskId(*path);
    match tasks.find_by_id(id).await {
        Ok(Some(existing)) => {
            // The spread carries the assignee through untouched: an ordinary
            // update neither sets nor clears it (D17).
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
                Ok(task) => {
                    let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                        .await
                        .map_err(repo_error_response)?;
                    Ok(HttpResponse::Ok().json(task_response(&task, &people)))
                }
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Body for `PATCH /api/tasks/{id}/status`: only the board column changes;
/// the task's other fields are untouched.
#[derive(Deserialize, ToSchema)]
pub struct TaskStatusRequest {
    #[schema(value_type = TaskStatusDoc)]
    pub status: TaskStatus,
}

/// Move Task to Another Column
///
/// Move a task between the board columns (backlog, to_do, in_progress, done),
/// changing only the column — the task's other fields are untouched. Any
/// column may move to any other; a blocked task can still be moved, and
/// repeating the same request is harmless. Requires the Staff or Admin role.
#[utoipa::path(
    patch,
    path = "/api/tasks/{id}/status",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = TaskStatusRequest,
    responses(
        (status = 200, description = "The updated task", body = TaskResponse),
        (status = 400, description = "Unknown status value", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn set_task_status(
    status: web::Data<TaskStatusService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<TaskStatusRequest>,
) -> Result<HttpResponse, ApiError> {
    match status.set_status(TaskId(*path), body.status).await {
        Ok(task) => {
            let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(task_response(&task, &people)))
        }
        Err(err) => Err(task_status_error_response(err)),
    }
}

/// Assign a Task
///
/// Assign the task to a person. Setting the person it is already assigned to
/// is harmless and leaves `updated_at` alone. Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/tasks/{id}/assignee",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = TaskAssigneeRequest,
    responses(
        (status = 200, description = "The updated task", body = TaskResponse),
        (status = 400, description = "Malformed body or assignee_id is not a UUID", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (
            status = 404,
            description = "No task with this id, or no user with the given assignee_id",
            body = ApiError
        ),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn set_task_assignee(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<TaskAssigneeRequest>,
) -> Result<HttpResponse, ApiError> {
    match assignment
        .set_task_assignee(TaskId(*path), UserId(body.assignee_id))
        .await
    {
        Ok(task) => {
            let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(task_response(&task, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}

/// Clear a Task's Assignee
///
/// Remove the task's assignee. Works even when the current assignee has since
/// been deactivated, and clearing an unassigned task is harmless. Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/tasks/{id}/assignee",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 200, description = "The updated task", body = TaskResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn clear_task_assignee(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match assignment.clear_task_assignee(TaskId(*path)).await {
        Ok(task) => {
            let people = resolve_summaries(users.as_ref(), [task.assignee_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(task_response(&task, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}

/// Delete Task
///
/// Delete a task. Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/tasks/{id}",
    tags = ["tasks"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 204, description = "Task deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn delete_task(
    tasks: web::Data<PostgresTaskRepository>,
    _access: EditAccess,
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
        let body: TaskRequest =
            serde_json::from_str(r#"{"title": "t", "status": "backlog"}"#).unwrap();
        assert_eq!(body.milestone_id, None);
        assert_eq!(body.description, None);
        assert_eq!(body.target_date, None);
    }

    #[test]
    fn create_task_request_defaults_the_assignee_to_none_and_reads_it_when_given() {
        let body: CreateTaskRequest =
            serde_json::from_str(r#"{"title": "t", "status": "backlog"}"#).unwrap();
        assert_eq!(body.assignee_id, None);

        let id = Uuid::new_v4();
        let with_assignee: CreateTaskRequest = serde_json::from_str(&format!(
            r#"{{"title": "t", "status": "backlog", "assignee_id": "{id}"}}"#
        ))
        .unwrap();
        assert_eq!(with_assignee.assignee_id, Some(id));

        // An explicit null also means unassigned.
        let nulled: CreateTaskRequest =
            serde_json::from_str(r#"{"title": "t", "status": "backlog", "assignee_id": null}"#)
                .unwrap();
        assert_eq!(nulled.assignee_id, None);
    }

    fn list_query(
        limit: Option<&str>,
        offset: Option<&str>,
        status: &[&str],
        milestone_id: Option<&str>,
        q: Option<&str>,
        target_after: Option<&str>,
        target_before: Option<&str>,
    ) -> TaskListQuery {
        TaskListQuery {
            limit: limit.map(str::to_owned),
            offset: offset.map(str::to_owned),
            status: status.iter().copied().map(str::to_owned).collect(),
            milestone_id: milestone_id.map(str::to_owned),
            q: q.map(str::to_owned),
            target_after: target_after.map(str::to_owned),
            target_before: target_before.map(str::to_owned),
            assignee_id: None,
        }
    }

    #[test]
    fn a_bare_list_query_means_the_default_page_and_no_filters() {
        let (filter, page) =
            parse_task_list_query(&list_query(None, None, &[], None, None, None, None)).unwrap();
        assert_eq!(page.limit, PageRequest::DEFAULT_PAGE_LIMIT);
        assert_eq!(page.offset, 0);
        assert!(filter.statuses.is_empty());
        assert_eq!(filter.milestone_id, None);
        assert_eq!(filter.q, None);
        assert_eq!(filter.target_after, None);
        assert_eq!(filter.target_before, None);
        assert_eq!(filter.assignee_id, None);
    }

    #[test]
    fn every_invalid_value_is_rejected_naming_its_parameter() {
        let cases = [
            (
                list_query(Some("0"), None, &[], None, None, None, None),
                "limit",
            ),
            (
                list_query(
                    Some(&format!("{}", PageRequest::MAX_PAGE_LIMIT + 1)),
                    None,
                    &[],
                    None,
                    None,
                    None,
                    None,
                ),
                "limit",
            ),
            (
                list_query(Some("abc"), None, &[], None, None, None, None),
                "limit",
            ),
            (
                list_query(None, Some("-5"), &[], None, None, None, None),
                "offset",
            ),
            (
                list_query(None, Some("abc"), &[], None, None, None, None),
                "offset",
            ),
            (
                list_query(None, None, &["bogus"], None, None, None, None),
                "status",
            ),
            (
                list_query(None, None, &[], Some("not-a-uuid"), None, None, None),
                "milestone_id",
            ),
            (
                list_query(None, None, &[], None, Some(&"x".repeat(101)), None, None),
                "q",
            ),
            (
                list_query(None, None, &[], None, None, Some("banana"), None),
                "target_after",
            ),
            (
                list_query(None, None, &[], None, None, None, Some("2026-13-40")),
                "target_before",
            ),
        ];
        for (query, param) in cases {
            let err = parse_task_list_query(&query).expect_err("must reject");
            assert_eq!(err.code, "invalid_query", "{param}");
            assert!(
                err.message.contains(param),
                "message names {param}: {}",
                err.message
            );
        }
    }

    #[test]
    fn valid_values_translate_into_the_filter_and_page() {
        let milestone_id = Uuid::new_v4();
        let (filter, page) = parse_task_list_query(&list_query(
            Some("5"),
            Some("10"),
            &["to_do,done", "backlog"],
            Some(&milestone_id.to_string()),
            Some("  Fix login  "),
            Some("2026-01-02"),
            Some("2026-12-31"),
        ))
        .unwrap();
        assert_eq!(page.limit, 5);
        assert_eq!(page.offset, 10);
        assert_eq!(
            filter.statuses,
            vec![TaskStatus::ToDo, TaskStatus::Done, TaskStatus::Backlog]
        );
        assert_eq!(filter.milestone_id, Some(MilestoneId(milestone_id)));
        assert_eq!(filter.q.as_deref(), Some("Fix login"));
        assert_eq!(
            filter.target_after,
            Some(NaiveDate::parse_from_str("2026-01-02", "%Y-%m-%d").unwrap())
        );
        assert_eq!(
            filter.target_before,
            Some(NaiveDate::parse_from_str("2026-12-31", "%Y-%m-%d").unwrap())
        );
    }

    #[test]
    fn empty_status_tokens_and_a_blank_search_are_ignored() {
        let (filter, _page) = parse_task_list_query(&list_query(
            None,
            None,
            &["", ","],
            None,
            Some("   "),
            None,
            None,
        ))
        .unwrap();
        assert!(filter.statuses.is_empty());
        assert_eq!(filter.q, None);
    }

    #[test]
    fn the_assignee_filter_parses_a_uuid_and_rejects_other_values_naming_the_parameter() {
        let id = Uuid::new_v4();
        let query = TaskListQuery {
            limit: None,
            offset: None,
            status: Vec::new(),
            milestone_id: None,
            q: None,
            target_after: None,
            target_before: None,
            assignee_id: Some(id.to_string()),
        };

        // A well-formed UUID lands in the filter...
        let (filter, _page) = parse_task_list_query(&query).unwrap();
        assert_eq!(filter.assignee_id, Some(UserId(id)));

        // ...and anything else is a 400 naming the parameter.
        let query = TaskListQuery {
            assignee_id: Some("not-a-uuid".to_owned()),
            ..query
        };
        let err = parse_task_list_query(&query).expect_err("must reject");
        assert_eq!(err.code, "invalid_query");
        assert!(err.message.contains("assignee_id"), "{}", err.message);
    }
}
