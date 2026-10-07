//! Goal–milestone linkage HTTP API (roadmap 3.4): link and unlink a goal to
//! a milestone, and list each side of the relation. The handlers are thin —
//! existence checks and idempotency live in [`GoalMilestoneLinkService`], and
//! the list bodies reuse the existing goal/milestone response shapes.

use actix_web::{HttpResponse, web};
use application::goal_milestone_links::GoalMilestoneLinkService;
use domain::{GoalId, MilestoneId};
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, goal_milestone_link_error_response};
use crate::goals::GoalResponse;
use crate::milestones::MilestoneResponse;

/// Link Goal to Milestone
///
/// Link a milestone to a goal. Safe to repeat: linking a pair that is already
/// linked is a no-op that answers 204 like a fresh link. A goal and a
/// milestone may each be linked to many of the other.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{goal_id}/milestones/{milestone_id}",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(
        ("goal_id" = Uuid, Path, description = "Goal identifier"),
        ("milestone_id" = Uuid, Path, description = "Milestone identifier")
    ),
    responses(
        (status = 204, description = "Linked (or already linked)"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal or milestone with this id", body = ApiError)
    )
)]
pub async fn link_goal_milestone(
    links: web::Data<GoalMilestoneLinkService>,
    _access: EditAccess,
    path: web::Path<(Uuid, Uuid)>,
) -> Result<HttpResponse, ApiError> {
    let (goal_id, milestone_id) = *path;
    match links.link(GoalId(goal_id), MilestoneId(milestone_id)).await {
        Ok(_) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}

/// Unlink Goal from Milestone
///
/// Remove the link between a goal and a milestone. Safe to repeat: unlinking
/// a pair that is not linked is a no-op that answers 204 like a real unlink.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{goal_id}/milestones/{milestone_id}",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(
        ("goal_id" = Uuid, Path, description = "Goal identifier"),
        ("milestone_id" = Uuid, Path, description = "Milestone identifier")
    ),
    responses(
        (status = 204, description = "Unlinked (or not linked)"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal or milestone with this id", body = ApiError)
    )
)]
pub async fn unlink_goal_milestone(
    links: web::Data<GoalMilestoneLinkService>,
    _access: EditAccess,
    path: web::Path<(Uuid, Uuid)>,
) -> Result<HttpResponse, ApiError> {
    let (goal_id, milestone_id) = *path;
    match links
        .unlink(GoalId(goal_id), MilestoneId(milestone_id))
        .await
    {
        Ok(_) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}

/// List Goal's Milestones
///
/// List the milestones linked to a goal, ordered by target date (goals and
/// milestones without one last), then creation date. Any signed-in user may
/// read.
#[utoipa::path(
    get,
    path = "/api/goals/{id}/milestones",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The milestones linked to the goal", body = Vec<MilestoneResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn list_goal_milestones(
    links: web::Data<GoalMilestoneLinkService>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match links.milestones_for_goal(GoalId(*path)).await {
        Ok(milestones) => Ok(HttpResponse::Ok().json(
            milestones
                .iter()
                .map(MilestoneResponse::from)
                .collect::<Vec<_>>(),
        )),
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}

/// List Milestone's Goals
///
/// List the goals linked to a milestone, ordered by target date (goals and
/// milestones without one last), then creation date. Any signed-in user may
/// read.
#[utoipa::path(
    get,
    path = "/api/milestones/{id}/goals",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The goals linked to the milestone", body = Vec<GoalResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn list_milestone_goals(
    links: web::Data<GoalMilestoneLinkService>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match links.goals_for_milestone(MilestoneId(*path)).await {
        Ok(goals) => {
            Ok(HttpResponse::Ok().json(goals.iter().map(GoalResponse::from).collect::<Vec<_>>()))
        }
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}
