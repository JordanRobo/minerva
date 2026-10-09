//! Goal–milestone linkage HTTP API (roadmap 3.4): link and unlink a goal to
//! a milestone, and list each side of the relation. The handlers are thin —
//! existence checks and idempotency live in [`GoalMilestoneLinkService`], and
//! the list bodies reuse the existing goal/milestone response shapes.

use actix_web::{HttpResponse, web};
use application::goal_milestone_links::GoalMilestoneLinkService;
use domain::{GoalId, MilestoneId};
use serde::Deserialize;
use serde::de::Deserializer;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{ApiError, goal_milestone_link_error_response};
use crate::goals::{GoalPage, GoalResponse, collect_list_query, parse_page_request};
use crate::milestones::{MilestonePage, MilestoneResponse};

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

/// Query params for the goal–milestone link lists: paging only — the path id
/// is the filter, so there are no other parameters. Plain strings because the
/// handler validates each one itself so a 400 can name the offending
/// parameter.
#[derive(IntoParams)]
pub struct LinkListQuery {
    /// How many linked rows per page (1–200). Defaults to 50.
    pub limit: Option<String>,
    /// How many matching rows to skip before the page starts. Defaults to 0.
    pub offset: Option<String>,
}

impl<'de> Deserialize<'de> for LinkListQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let raw = collect_list_query(pairs).map_err(serde::de::Error::custom)?;
        Ok(LinkListQuery {
            limit: raw.limit,
            offset: raw.offset,
        })
    }
}

/// List Goal's Milestones
///
/// List the milestones linked to a goal, one page at a time, ordered by
/// target date (goals and milestones without one last), then creation date —
/// so pages are stable. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/goals/{id}/milestones",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Goal identifier"),
        LinkListQuery
    ),
    responses(
        (status = 200, description = "One page of the linked milestones plus the total count", body = MilestonePage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn list_goal_milestones(
    links: web::Data<GoalMilestoneLinkService>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
    query: web::Query<LinkListQuery>,
) -> Result<HttpResponse, ApiError> {
    let page_request = parse_page_request(query.0.limit.as_deref(), query.0.offset.as_deref())?;
    match links
        .milestones_for_goal_page(GoalId(*path), &page_request)
        .await
    {
        Ok(page) => Ok(HttpResponse::Ok().json(MilestonePage {
            items: page.items.iter().map(MilestoneResponse::from).collect(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })),
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}

/// List Milestone's Goals
///
/// List the goals linked to a milestone, one page at a time, ordered by
/// target date (goals and milestones without one last), then creation date —
/// so pages are stable. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/milestones/{id}/goals",
    tags = ["goal-milestone-links"],
    security(("session_cookie" = [])),
    params(
        ("id" = Uuid, Path, description = "Milestone identifier"),
        LinkListQuery
    ),
    responses(
        (status = 200, description = "One page of the linked goals plus the total count", body = GoalPage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn list_milestone_goals(
    links: web::Data<GoalMilestoneLinkService>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
    query: web::Query<LinkListQuery>,
) -> Result<HttpResponse, ApiError> {
    let page_request = parse_page_request(query.0.limit.as_deref(), query.0.offset.as_deref())?;
    match links
        .goals_for_milestone_page(MilestoneId(*path), &page_request)
        .await
    {
        Ok(page) => Ok(HttpResponse::Ok().json(GoalPage {
            items: page.items.iter().map(GoalResponse::from).collect(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })),
        Err(err) => Err(goal_milestone_link_error_response(err)),
    }
}
