//! OpenAPI document for the Minerva API, assembled from the
//! `#[utoipa::path]` annotations on the handlers in `goals`, `milestones`,
//! and `tasks`. Served as JSON at `/api-docs/openapi.json` and rendered by
//! Swagger UI at `/api-docs/swagger-ui/` (wired up in `main.rs`).
//!
//! The `*Doc` schema types mirror the wire shape of domain status types:
//! `domain` is pure logic and cannot depend on utoipa, so the documentation
//! restates those JSON shapes here instead of deriving them from the domain.
// The `*Doc` types are only referenced through utoipa proc macros (which emit
// their variant/field names as strings), so rustc's dead-code analysis never
// sees a runtime use of them.
#![allow(dead_code)]

use utoipa::{OpenApi, ToSchema};

/// Wire shape of [`domain::TaskStatus`]: a snake_case string.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusDoc {
    Backlog,
    ToDo,
    InProgress,
    Done,
}

/// Wire shape of [`domain::Status`].
#[derive(ToSchema)]
pub enum StatusDoc {
    OnTrack,
    AtRisk,
    OffTrack,
    Complete,
}

/// Wire shape of [`domain::StatusSource`].
#[derive(ToSchema)]
pub enum StatusSourceDoc {
    Computed,
    ManualOverride,
}

/// Wire shape of [`domain::GoalStatus`]: a status together with where it came from.
#[derive(ToSchema)]
pub struct GoalStatusDoc {
    pub status: StatusDoc,
    pub source: StatusSourceDoc,
}

/// The OpenAPI 3 document for the Minerva API.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Minerva API",
        version = "0.1.0",
        description = "REST API for Minerva, a goal/milestone-first project-management platform for schools.",
    ),
    tags(
        (name = "goals", description = "Goals: the top-level outcomes a school works toward."),
        (name = "milestones", description = "Milestones: dated checkpoints within a goal."),
        (name = "tasks", description = "Tasks: day-to-day work, optionally attached to a milestone."),
    ),
    paths(
        crate::goals::create_goal,
        crate::goals::list_goals,
        crate::goals::get_goal,
        crate::goals::update_goal,
        crate::goals::delete_goal,
        crate::milestones::create_milestone,
        crate::milestones::list_milestones,
        crate::milestones::get_milestone,
        crate::milestones::update_milestone,
        crate::milestones::delete_milestone,
        crate::tasks::create_task,
        crate::tasks::list_tasks,
        crate::tasks::get_task,
        crate::tasks::update_task,
        crate::tasks::delete_task,
    ),
    components(schemas(
        crate::error::ApiError,
        crate::goals::GoalRequest,
        crate::goals::GoalResponse,
        crate::milestones::MilestoneRequest,
        crate::milestones::MilestoneResponse,
        crate::tasks::TaskListQuery,
        crate::tasks::TaskRequest,
        crate::tasks::TaskResponse,
        GoalStatusDoc,
        StatusDoc,
        StatusSourceDoc,
        TaskStatusDoc,
    ))
)]
pub struct ApiDoc;
