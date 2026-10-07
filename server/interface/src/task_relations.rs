//! Task-relation HTTP API (roadmap 3.6): create and delete relations between
//! tasks, and list everything a task is connected to. The handlers are thin —
//! existence checks and canonicalisation live in [`TaskRelationService`], and
//! every response reports the relation from the path task's perspective.

use actix_web::{HttpResponse, web};
use application::task_relations::{TaskRelationService, TaskRelationView};
use chrono::{DateTime, Utc};
use domain::{TaskId, TaskRelationId, TaskRelationType, TaskStatus};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, task_relation_error_response};
use crate::openapi::TaskStatusDoc;

/// Wire shape of [`domain::TaskRelationType`]: a snake_case string. The
/// domain type cannot carry serde rename attributes (the `/debug/*` routes
/// serialize it directly), so the wire format is restated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    /// The task must be finished before the related task can proceed.
    Blocks,
    /// The task cannot proceed until the related task is finished: the same
    /// relationship as `blocks`, seen from the other side.
    BlockedBy,
    /// The two tasks are connected, without saying which one comes first.
    RelatesTo,
}

impl From<RelationType> for TaskRelationType {
    fn from(wire: RelationType) -> Self {
        match wire {
            RelationType::Blocks => TaskRelationType::Blocks,
            RelationType::BlockedBy => TaskRelationType::BlockedBy,
            RelationType::RelatesTo => TaskRelationType::RelatesTo,
        }
    }
}

impl From<TaskRelationType> for RelationType {
    fn from(relation_type: TaskRelationType) -> Self {
        match relation_type {
            TaskRelationType::Blocks => RelationType::Blocks,
            TaskRelationType::BlockedBy => RelationType::BlockedBy,
            TaskRelationType::RelatesTo => RelationType::RelatesTo,
            // New variants cannot reach the wire until a name is chosen for
            // them here; the database check constraint keeps stored rows to
            // the three known types in the meantime.
            _ => unreachable!("a relation type without a wire name"),
        }
    }
}

/// Body for `POST /api/tasks/{id}/relations`: the kind of connection, from
/// the path task's perspective, and the other task.
#[derive(Deserialize, ToSchema)]
pub struct CreateRelationRequest {
    pub relation_type: RelationType,
    pub related_task_id: Uuid,
}

/// The other end of a relation: just enough to display it in a list.
#[derive(Serialize, ToSchema)]
pub struct RelatedTaskResponse {
    pub id: Uuid,
    pub title: String,
    #[schema(value_type = TaskStatusDoc)]
    pub status: TaskStatus,
}

/// JSON shape of a task relation in responses: the type from the path task's
/// perspective plus a summary of the other task.
#[derive(Serialize, ToSchema)]
pub struct TaskRelationResponse {
    pub id: Uuid,
    pub relation_type: RelationType,
    pub related_task: RelatedTaskResponse,
    pub created_at: DateTime<Utc>,
}

impl From<&TaskRelationView> for TaskRelationResponse {
    fn from(view: &TaskRelationView) -> Self {
        Self {
            id: view.relation_id.0,
            relation_type: view.relation_type.into(),
            related_task: RelatedTaskResponse {
                id: view.related_task.id.0,
                title: view.related_task.title.clone(),
                status: view.related_task.status,
            },
            created_at: view.created_at,
        }
    }
}

/// Create Task Relation
///
/// Relate a task to another task. Three kinds of connection exist, all read
/// from the path task's perspective: `blocks` (this task must finish before
/// the related one can proceed), `blocked_by` (the same relationship seen
/// from the other side — creating it is equivalent to the related task
/// blocking this one), and `relates_to` (a symmetric connection with no
/// ordering). A pair of tasks can hold at most one relation of a kind, and
/// two tasks cannot block each other. The response reports the new relation
/// as the path task sees it. Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/tasks/{id}/relations",
    tags = ["task-relations"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    request_body = CreateRelationRequest,
    responses(
        (status = 201, description = "The relation, as the path task sees it", body = TaskRelationResponse),
        (status = 400, description = "Unknown relation type, or a task related to itself", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError),
        (status = 409, description = "The relation already exists, or the opposite blocking relationship does", body = ApiError)
    )
)]
pub async fn create_task_relation(
    relations: web::Data<TaskRelationService>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<CreateRelationRequest>,
) -> Result<HttpResponse, ApiError> {
    match relations
        .create(
            TaskId(*path),
            body.relation_type.into(),
            TaskId(body.related_task_id),
        )
        .await
    {
        Ok(view) => Ok(HttpResponse::Created().json(TaskRelationResponse::from(&view))),
        Err(err) => Err(task_relation_error_response(err)),
    }
}

/// Delete Task Relation
///
/// Remove a relation from a task. The relation must involve the task named
/// in the path; deleting one that belongs to other tasks answers 404 like an
/// unknown id, so its existence is not leaked. Requires the Staff or Admin
/// role.
#[utoipa::path(
    delete,
    path = "/api/tasks/{id}/relations/{relation_id}",
    tags = ["task-relations"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Task identifier"),
        ("relation_id" = Uuid, Path, description = "Relation identifier")
    ),
    responses(
        (status = 204, description = "Relation deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No task or relation with this id", body = ApiError)
    )
)]
pub async fn delete_task_relation(
    relations: web::Data<TaskRelationService>,
    _access: EditAccess,
    path: web::Path<(Uuid, Uuid)>,
) -> Result<HttpResponse, ApiError> {
    let (task_id, relation_id) = *path;
    match relations
        .delete(TaskId(task_id), TaskRelationId(relation_id))
        .await
    {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(task_relation_error_response(err)),
    }
}

/// List Task's Relations
///
/// List every relation in which a task participates, from its perspective:
/// the type each one reads as from this task (`blocks`, `blocked_by` or
/// `relates_to`) plus a summary of the other task. Ordered by creation time,
/// then id. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/tasks/{id}/relations",
    tags = ["task-relations"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Task identifier")),
    responses(
        (status = 200, description = "The task's relations", body = Vec<TaskRelationResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No task with this id", body = ApiError)
    )
)]
pub async fn list_task_relations(
    relations: web::Data<TaskRelationService>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match relations.list_for_task(TaskId(*path)).await {
        Ok(views) => Ok(HttpResponse::Ok().json(
            views
                .iter()
                .map(TaskRelationResponse::from)
                .collect::<Vec<_>>(),
        )),
        Err(err) => Err(task_relation_error_response(err)),
    }
}
