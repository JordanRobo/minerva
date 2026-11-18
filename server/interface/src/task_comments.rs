//! Task-comment HTTP API (roadmap 3.12, D20): flat, plain-text notes on a
//! task. The handlers are thin — the existence checks and ownership rules
//! live in [`TaskCommentService`] (with the author/Admin decisions in
//! `application::authz`), and every response embeds the author as a person
//! summary or null, resolved with one user query per response or page (D17).

use actix_web::{HttpResponse, web};
use application::pagination::{PageRequest, PageRequestError};
use application::task_comments::TaskCommentService;
use chrono::{DateTime, Utc};
use domain::{TaskComment, TaskCommentId, TaskId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, repo_error_response, task_comment_error_response};
use crate::people::{PersonSummary, resolve_summaries};

/// Body for `POST /api/tasks/{id}/comments` and
/// `PUT /api/tasks/{id}/comments/{comment_id}`: the (re)written text. The
/// server trims it and enforces the D20 length rule.
#[derive(Deserialize, ToSchema)]
pub struct CommentRequest {
    /// The comment text; surrounding whitespace is trimmed, 1–4000 characters.
    pub body: String,
}

/// JSON shape of a task comment in responses (D20): the author as a person
/// summary or null (a deleted author leaves `null`, like D17), never the
/// email.
#[derive(Serialize, ToSchema)]
pub struct TaskCommentResponse {
    pub id: Uuid,
    pub task_id: Uuid,
    /// The author as a person summary; null when the author has since been
    /// deleted. A deactivated author still appears, with `active` false.
    pub author: Option<PersonSummary>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One page of a task's comments plus the pagination metadata (D16, D20):
/// `total` is the number of comments on the task, ignoring paging.
#[derive(Serialize, ToSchema)]
pub struct TaskCommentPage {
    pub items: Vec<TaskCommentResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u64,
}

/// Query params for `GET /api/tasks/{id}/comments`. Plain strings so the
/// handler validates each one and a 400 can name the offending parameter; a
/// duplicated parameter is rejected by the query deserialiser and routed to
/// the shared `QueryConfig` error handler.
#[derive(Deserialize, IntoParams)]
pub struct CommentListQuery {
    /// How many comments per page (1–200). Defaults to 50.
    pub limit: Option<String>,
    /// How many comments to skip before the page starts. Defaults to 0.
    pub offset: Option<String>,
}

/// Validate and translate the raw query string into the page the service
/// takes. Every failure is a 400 `invalid_query` naming the offending
/// parameter, like the other paginated lists.
fn parse_comment_list_query(params: &CommentListQuery) -> Result<PageRequest, ApiError> {
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
    PageRequest::new(limit, offset).map_err(|err| match err {
        PageRequestError::LimitOutOfRange(value) => ApiError::invalid_query(format!(
            "limit must be between 1 and {}, got {value}",
            PageRequest::MAX_PAGE_LIMIT
        )),
        PageRequestError::NegativeOffset(value) => {
            ApiError::invalid_query(format!("offset must not be negative, got {value}"))
        }
    })
}

/// Build the response for one comment, looking its author up in the batch
/// resolved by [`resolve_summaries`] — one user query per response or page,
/// never one per row (D17). An author that no longer resolves (deleted) is
/// `null`.
fn comment_response(
    comment: &TaskComment,
    people: &HashMap<Uuid, PersonSummary>,
) -> TaskCommentResponse {
    TaskCommentResponse {
        id: comment.id.0,
        task_id: comment.task_id.0,
        author: comment
            .author_id
            .as_ref()
            .and_then(|id| people.get(&id.0).cloned()),
        body: comment.body.clone(),
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }
}

/// List Task Comments
///
/// List a task's comments, oldest first (created time, then id) — the thread
/// reads top to bottom. The task must exist; an unknown task is a 404 even
/// when it has no comments. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/tasks/{id}/comments",
    tags = ["task-comments"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Task identifier"),
        CommentListQuery
    ),
    responses(
        (status = 200, description = "One page of the task's comments, oldest first", body = TaskCommentPage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn list_task_comments(
    comments: web::Data<TaskCommentService>,
    users: web::Data<dyn application::ports::UserRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
    query: web::Query<CommentListQuery>,
) -> Result<HttpResponse, ApiError> {
    let page_request = parse_comment_list_query(&query.0)?;
    match comments.list(TaskId(*path), &page_request).await {
        Ok(page) => {
            // One user query for the whole page's authors (D17).
            let people = resolve_summaries(
                users.as_ref(),
                page.items.iter().map(|comment| comment.author_id),
            )
            .await
            .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(TaskCommentPage {
                items: page
                    .items
                    .iter()
                    .map(|comment| comment_response(comment, &people))
                    .collect(),
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }))
        }
        Err(err) => Err(task_comment_error_response(err)),
    }
}

/// Create Task Comment
///
/// Add a comment to a task. The acting user is recorded as the author and
/// `created_at` equals `updated_at` until the body is edited. Any Staff or
/// Admin may comment on any task; the body is trimmed and must be 1–4000
/// characters.
#[utoipa::path(
    post,
    path = "/api/tasks/{id}/comments",
    tags = ["task-comments"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = CommentRequest,
    responses(
        (status = 201, description = "The created comment", body = TaskCommentResponse),
        (
            status = 400,
            description = "The body is blank or longer than 4000 characters",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn create_task_comment(
    comments: web::Data<TaskCommentService>,
    users: web::Data<dyn application::ports::UserRepository>,
    access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<CommentRequest>,
) -> Result<HttpResponse, ApiError> {
    match comments
        .create(&access.user, TaskId(*path), &body.body)
        .await
    {
        Ok(comment) => {
            let people = resolve_summaries(users.as_ref(), [comment.author_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Created().json(comment_response(&comment, &people)))
        }
        Err(err) => Err(task_comment_error_response(err)),
    }
}

/// Update Task Comment
///
/// Replace a comment's body in place. Only the author may edit a comment,
/// whatever their role — a Staff or Admin who is not the author gets a 403.
/// Re-submitting the stored body changes nothing and leaves `updated_at`
/// alone. A comment that lives on another task is a 404 like an unknown id,
/// so its existence is not leaked.
#[utoipa::path(
    put,
    path = "/api/tasks/{id}/comments/{comment_id}",
    tags = ["task-comments"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Task identifier"),
        ("comment_id" = Uuid, Path, description = "Comment identifier")
    ),
    request_body = CommentRequest,
    responses(
        (status = 200, description = "The updated comment", body = TaskCommentResponse),
        (
            status = 400,
            description = "The body is blank or longer than 4000 characters",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (
            status = 403,
            description = "Requires the Staff or Admin role, and only the author may edit a comment",
            body = ApiError
        ),
        (
            status = 404,
            description = "No task, or no comment on that task, with this id",
            body = ApiError
        )
    )
)]
pub async fn update_task_comment(
    comments: web::Data<TaskCommentService>,
    users: web::Data<dyn application::ports::UserRepository>,
    access: EditAccess,
    path: web::Path<(Uuid, Uuid)>,
    body: web::Json<CommentRequest>,
) -> Result<HttpResponse, ApiError> {
    let (task_id, comment_id) = *path;
    match comments
        .update(
            &access.user,
            TaskId(task_id),
            TaskCommentId(comment_id),
            &body.body,
        )
        .await
    {
        Ok(comment) => {
            let people = resolve_summaries(users.as_ref(), [comment.author_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(comment_response(&comment, &people)))
        }
        Err(err) => Err(task_comment_error_response(err)),
    }
}

/// Delete Task Comment
///
/// Remove a comment (a hard delete — no edit history is kept). The author or
/// any Admin may delete it; anyone else gets a 403. A comment that lives on
/// another task is a 404 like an unknown id, so its existence is not leaked.
#[utoipa::path(
    delete,
    path = "/api/tasks/{id}/comments/{comment_id}",
    tags = ["task-comments"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Task identifier"),
        ("comment_id" = Uuid, Path, description = "Comment identifier")
    ),
    responses(
        (status = 204, description = "The comment was deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (
            status = 403,
            description = "Requires the Staff or Admin role, and only the author or an Admin may delete a comment",
            body = ApiError
        ),
        (
            status = 404,
            description = "No task, or no comment on that task, with this id",
            body = ApiError
        )
    )
)]
pub async fn delete_task_comment(
    comments: web::Data<TaskCommentService>,
    access: EditAccess,
    path: web::Path<(Uuid, Uuid)>,
) -> Result<HttpResponse, ApiError> {
    let (task_id, comment_id) = *path;
    match comments
        .delete(&access.user, TaskId(task_id), TaskCommentId(comment_id))
        .await
    {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(task_comment_error_response(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::UserId;

    /// The list query validation answers the same 400s as the other
    /// paginated lists, naming the offending parameter.
    #[test]
    fn the_list_query_validation_matches_the_other_lists() {
        // Defaults apply when nothing is given.
        let page = parse_comment_list_query(&CommentListQuery {
            limit: None,
            offset: None,
        })
        .expect("defaults are valid");
        assert_eq!(page.limit, PageRequest::DEFAULT_PAGE_LIMIT);
        assert_eq!(page.offset, 0);

        // Out-of-range and malformed values name their parameter.
        let cases = [
            (Some("0"), None, "limit must be between 1 and 200, got 0"),
            (
                Some(&format!("{}", PageRequest::MAX_PAGE_LIMIT + 1)),
                None,
                &format!(
                    "limit must be between 1 and {}, got {}",
                    PageRequest::MAX_PAGE_LIMIT,
                    PageRequest::MAX_PAGE_LIMIT + 1
                ),
            ),
            (
                Some("abc"),
                None,
                "limit must be a whole number, got \"abc\"",
            ),
            (None, Some("-1"), "offset must not be negative, got -1"),
            (
                None,
                Some("abc"),
                "offset must be a whole number, got \"abc\"",
            ),
        ];
        for (limit, offset, expected) in &cases {
            let err = parse_comment_list_query(&CommentListQuery {
                limit: (*limit).map(str::to_owned),
                offset: (*offset).map(str::to_owned),
            })
            .unwrap_err();
            assert_eq!(err.code, "invalid_query", "{expected}");
            assert_eq!(err.message, *expected, "{expected}");
        }

        // A valid pair passes through.
        let page = parse_comment_list_query(&CommentListQuery {
            limit: Some("5".into()),
            offset: Some("10".into()),
        })
        .expect("a valid pair is accepted");
        assert_eq!(page.limit, 5);
        assert_eq!(page.offset, 10);
    }

    /// A comment whose author no longer resolves renders `author: null`; one
    /// that does keeps the summary.
    #[test]
    fn a_deleted_author_renders_null() {
        let now = Utc::now();
        let comment = TaskComment {
            id: TaskCommentId::new(),
            task_id: TaskId::new(),
            author_id: Some(UserId::new()),
            body: "hi".into(),
            created_at: now,
            updated_at: now,
        };
        // No summaries resolved: the author is unknown, so null.
        let empty = HashMap::new();
        assert!(comment_response(&comment, &empty).author.is_none());

        // A resolved author keeps its summary.
        let mut people = HashMap::new();
        people.insert(
            comment.author_id.unwrap().0,
            PersonSummary {
                id: comment.author_id.unwrap().0,
                display_name: "Someone".into(),
                active: false,
            },
        );
        let response = comment_response(&comment, &people);
        assert_eq!(response.author.as_ref().unwrap().display_name, "Someone");
        assert!(!response.author.unwrap().active);
    }
}
