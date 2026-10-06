//! TEMPORARY, admin-only debug endpoints for verifying repository wiring
//! end-to-end against a live Postgres. Each handler calls one read method on
//! a single repository and returns the result as JSON; an empty database
//! yields an empty array, which is itself a useful "it works" signal.
//!
//! These are not meant to ship: they stay locked down to Admin (roadmap
//! 2.4) until roadmap 3.4/3.6 provide real endpoints for the same data, and
//! their removal is roadmap 8.5.

use actix_web::{HttpResponse, web};
use application::ports::{
    GoalMilestoneRepository, ProgressSnapshotRepository, TaskRelationRepository,
};
use domain::{GoalId, MilestoneId, ProgressTarget, TaskId};
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresProgressSnapshotRepository,
    PostgresTaskRelationRepository,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::access::AdminAccess;

#[derive(Deserialize)]
pub struct TaskIdQuery {
    pub task_id: Uuid,
}

/// `GET /debug/task-relations?task_id=<uuid>` — relations a task touches.
pub async fn list_task_relations(
    _access: AdminAccess,
    query: web::Query<TaskIdQuery>,
    relations: web::Data<PostgresTaskRelationRepository>,
) -> HttpResponse {
    match relations.list_for_task(TaskId(query.task_id)).await {
        Ok(relations) => HttpResponse::Ok().json(relations),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

#[derive(Deserialize)]
pub struct TargetQuery {
    pub goal_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
}

/// `GET /debug/progress-snapshots?goal_id=<uuid>` or `?milestone_id=<uuid>`.
pub async fn list_progress_snapshots(
    _access: AdminAccess,
    query: web::Query<TargetQuery>,
    snapshots: web::Data<PostgresProgressSnapshotRepository>,
) -> HttpResponse {
    // A snapshot is for exactly one of a goal or a milestone.
    let target = match (query.goal_id, query.milestone_id) {
        (Some(goal_id), None) => ProgressTarget::Goal(GoalId(goal_id)),
        (None, Some(milestone_id)) => ProgressTarget::Milestone(MilestoneId(milestone_id)),
        _ => {
            return HttpResponse::BadRequest()
                .body("provide exactly one of goal_id or milestone_id");
        }
    };
    match snapshots.list_for_target(target).await {
        Ok(snapshots) => HttpResponse::Ok().json(snapshots),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

#[derive(Deserialize)]
pub struct GoalIdQuery {
    pub goal_id: Uuid,
}

/// `GET /debug/goal-milestones?goal_id=<uuid>` — milestones linked to a goal.
pub async fn list_goal_milestones(
    _access: AdminAccess,
    query: web::Query<GoalIdQuery>,
    links: web::Data<PostgresGoalMilestoneRepository>,
) -> HttpResponse {
    // The repository returns full milestones; the debug endpoint has always
    // answered with bare ids, so project back to keep that shape.
    match links.milestones_for_goal(GoalId(query.goal_id)).await {
        Ok(milestones) => HttpResponse::Ok().json(
            milestones
                .into_iter()
                .map(|milestone| milestone.id)
                .collect::<Vec<_>>(),
        ),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}
