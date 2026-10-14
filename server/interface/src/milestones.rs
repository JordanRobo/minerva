//! Milestones HTTP API: the `/api/milestones` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Milestone`]: the
//! wire format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{HttpResponse, web};
use application::assignment::AssignmentService;
use application::milestone_list::MilestoneListService;
use application::pagination::PageRequest;
use application::ports::{MilestoneListFilter, MilestoneRepository, UserRepository};
use application::status_override::StatusOverrideService;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Milestone, MilestoneId, Status, StatusSource, UserId};
use infrastructure::repositories::PostgresMilestoneRepository;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{
    ApiError, assignment_error_response, repo_error_response, status_override_error_response,
};
use crate::goals::{
    StatusOverrideRequest, collect_list_query, parse_page_request, parse_q, parse_target_date,
};
use crate::openapi::{StatusDoc, StatusSourceDoc};
use crate::people::{OwnerRequest, PersonSummary, resolve_summaries};

/// JSON shape of a milestone in responses.
#[derive(Serialize, ToSchema)]
pub struct MilestoneResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// The status as it stands now: the manual setting if one is active,
    /// otherwise the automatic value worked out from its tasks and target date.
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Whether [`MilestoneResponse::status`] was set by hand ("manual") or
    /// worked out by the system ("automatic").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    pub target_date: Option<NaiveDate>,
    /// The person the milestone is owned by, or null when unowned (D17). A
    /// deactivated owner still appears, with `active` false.
    pub owner: Option<PersonSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Build the response for one milestone, looking its owner up in the batch
/// resolved by [`resolve_summaries`] — one user query per response or page,
/// never one per row (D17). `pub(crate)` so the goal–milestone link lists
/// reuse it.
pub(crate) fn milestone_response(
    milestone: &Milestone,
    people: &HashMap<Uuid, PersonSummary>,
) -> MilestoneResponse {
    // The wire shape reports the effective status and where it came
    // from; the raw automatic value and override are domain-side.
    let mut response = MilestoneResponse {
        id: milestone.id.0,
        title: milestone.title.clone(),
        description: milestone.description.clone(),
        status: milestone.effective_status(),
        status_source: milestone.status_source(),
        target_date: milestone.target_date,
        owner: None,
        created_at: milestone.created_at,
        updated_at: milestone.updated_at,
    };
    if let Some(owner_id) = milestone.owner_id {
        response.owner = people.get(&owner_id.0).cloned();
    }
    response
}

/// Body for `PUT /api/milestones/{id}`. The id, status, and timestamps are
/// server-managed and never accepted from the client; the owner is not part
/// of an ordinary update — only creation and the dedicated owner routes set
/// it (D17).
#[derive(Deserialize, ToSchema)]
pub struct MilestoneRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// Body for `POST /api/milestones`: the update fields plus the optional owner.
#[derive(Deserialize, ToSchema)]
pub struct CreateMilestoneRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
    /// The user to make the owner; must be an existing active account.
    /// Omitted or null leaves the milestone unowned.
    #[serde(default)]
    pub owner_id: Option<Uuid>,
}

/// One page of milestones plus the pagination metadata (roadmap 3.10, D16):
/// `total` is the number of milestones matching the filters, ignoring paging,
/// so a client can tell how many pages there are without fetching them all.
#[derive(Serialize, ToSchema)]
pub struct MilestonePage {
    pub items: Vec<MilestoneResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u64,
}

/// Query params for `GET /api/milestones`. Every parameter is optional and
/// narrows the result set; they are all plain strings here because the
/// handler validates each one itself so a 400 can name the offending
/// parameter.
#[derive(IntoParams)]
pub struct MilestoneListQuery {
    /// How many milestones per page (1–200). Defaults to 50.
    pub limit: Option<String>,
    /// How many matching milestones to skip before the page starts. Defaults
    /// to 0.
    pub offset: Option<String>,
    /// Only milestones in these statuses (`on_track`, `at_risk`, `off_track`,
    /// `complete`). The effective status is matched: a manual setting wins
    /// over the automatic one. May be given several times or comma-separated.
    pub status: Vec<String>,
    /// Case-insensitive substring match on the milestone title.
    pub q: Option<String>,
    /// Only milestones whose target date is on or after this date
    /// (`YYYY-MM-DD`).
    pub target_after: Option<String>,
    /// Only milestones whose target date is on or before this date
    /// (`YYYY-MM-DD`).
    pub target_before: Option<String>,
}

impl<'de> Deserialize<'de> for MilestoneListQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let raw = collect_list_query(pairs).map_err(serde::de::Error::custom)?;
        Ok(MilestoneListQuery {
            limit: raw.limit,
            offset: raw.offset,
            status: raw.status,
            q: raw.q,
            target_after: raw.target_after,
            target_before: raw.target_before,
        })
    }
}

/// Validate and translate the raw query string into the filter and page the
/// service takes. Every failure is a 400 `invalid_query` naming the
/// offending parameter; filters that match nothing are still valid (an empty
/// result, never an error).
fn parse_milestone_list_query(
    params: &MilestoneListQuery,
) -> Result<(MilestoneListFilter, PageRequest), ApiError> {
    let page_request = parse_page_request(params.limit.as_deref(), params.offset.as_deref())?;

    let mut statuses = Vec::new();
    for raw in &params.status {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            statuses.push(match token {
                "on_track" => Status::OnTrack,
                "at_risk" => Status::AtRisk,
                "off_track" => Status::OffTrack,
                "complete" => Status::Complete,
                other => {
                    return Err(ApiError::invalid_query(format!(
                        "status must be one of on_track, at_risk, off_track or complete, got {other:?}"
                    )));
                }
            });
        }
    }

    let q = parse_q(params.q.as_deref())?;
    let target_after = parse_target_date(params.target_after.as_deref(), "target_after")?;
    let target_before = parse_target_date(params.target_before.as_deref(), "target_before")?;

    Ok((
        MilestoneListFilter {
            statuses,
            q,
            target_after,
            target_before,
        },
        page_request,
    ))
}

/// Create Milestone
///
/// Create a new milestone - a measurable step toward a goal, optionally
/// naming an owner. Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/milestones",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    request_body = CreateMilestoneRequest,
    responses(
        (status = 201, description = "Milestone created", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No user with the given owner_id", body = ApiError),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn create_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    body: web::Json<CreateMilestoneRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    // Validated exactly like the set endpoint (D17): an unknown user 404s,
    // a deactivated one 409s, before anything is written.
    let owner_id = match body.owner_id {
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
    let milestone = Milestone {
        id: MilestoneId::new(),
        title: body.title.clone(),
        description: body.description.clone(),
        status: Status::OnTrack,
        status_override: None,
        target_date: body.target_date,
        owner_id,
        created_at: now,
        updated_at: now,
    };
    match milestones.create(milestone).await {
        Ok(milestone) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Created().json(milestone_response(&milestone, &people)))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// List all Milestones
///
/// List milestones, one page at a time. Every query parameter is optional and
/// narrows the result set; together they are combined with AND. The status
/// filter matches the effective status — a manual setting wins over the
/// automatic value. The order is fixed: target date ascending (milestones
/// without one last), then creation time, then id — so pages are stable. A
/// filter that matches nothing is an empty page, not an error. Any signed-in
/// user may read.
#[utoipa::path(
    get,
    path = "/api/milestones",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(MilestoneListQuery),
    responses(
        (status = 200, description = "One page of the matching milestones plus the total count", body = MilestonePage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_milestones(
    query: web::Query<MilestoneListQuery>,
    service: web::Data<MilestoneListService>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let (filter, page_request) = parse_milestone_list_query(&query.0)?;
    match service.list_page(&filter, &page_request).await {
        Ok(page) => {
            // One user query for the whole page's owners (D17).
            let people = resolve_summaries(
                users.as_ref(),
                page.items.iter().map(|milestone| milestone.owner_id),
            )
            .await
            .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(MilestonePage {
                items: page
                    .items
                    .iter()
                    .map(|milestone| milestone_response(milestone, &people))
                    .collect(),
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Get Individual Milestone
///
/// Fetch a single milestone by its ID. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The milestone", body = MilestoneResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn get_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.find_by_id(MilestoneId(*path)).await {
        Ok(Some(milestone)) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Update Milestone
///
/// Update a milestone's title, description, or target date. Status and timestamps are managed by the server.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = MilestoneRequest,
    responses(
        (status = 200, description = "The updated milestone", body = MilestoneResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn update_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<MilestoneRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = MilestoneId(*path);
    match milestones.find_by_id(id).await {
        Ok(Some(existing)) => {
            // The spread carries the owner through untouched: an ordinary
            // update neither sets nor clears it (D17).
            let updated = Milestone {
                title: body.title.clone(),
                description: body.description.clone(),
                target_date: body.target_date,
                status: existing.status,
                created_at: existing.created_at,
                updated_at: Utc::now(),
                ..existing
            };
            match milestones.update(updated).await {
                Ok(milestone) => {
                    let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                        .await
                        .map_err(repo_error_response)?;
                    Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
                }
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Delete Milestone
///
/// Delete a milestone. Tasks assigned to it become unassigned rather than being deleted.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 204, description = "Milestone deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn delete_milestone(
    milestones: web::Data<PostgresMilestoneRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match milestones.delete(MilestoneId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Set Milestone Status Override
///
/// Manually set a milestone's status. The manual setting is sticky: it holds
/// until cleared, even if the milestone's tasks or target date change in the
/// meantime.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}/status-override",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = StatusOverrideRequest,
    responses(
        (status = 200, description = "The milestone with its new status", body = MilestoneResponse),
        (status = 400, description = "Unknown status value", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn set_milestone_status_override(
    overrides: web::Data<StatusOverrideService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<StatusOverrideRequest>,
) -> Result<HttpResponse, ApiError> {
    match overrides
        .set_milestone_override(MilestoneId(*path), body.status)
        .await
    {
        Ok(milestone) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
        }
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Clear Milestone Status Override
///
/// Remove a milestone's manual status setting so it returns to automatic —
/// the status worked out from its tasks and target date.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}/status-override",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The milestone with its automatic status", body = MilestoneResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn clear_milestone_status_override(
    overrides: web::Data<StatusOverrideService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match overrides.clear_milestone_override(MilestoneId(*path)).await {
        Ok(milestone) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
        }
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Set Milestone Owner
///
/// Name the person who owns the milestone. Setting the person it is already
/// owned by is harmless and leaves `updated_at` alone. Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/milestones/{id}/owner",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    request_body = OwnerRequest,
    responses(
        (status = 200, description = "The updated milestone", body = MilestoneResponse),
        (status = 400, description = "Malformed body or owner_id is not a UUID", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (
            status = 404,
            description = "No milestone with this id, or no user with the given owner_id",
            body = ApiError
        ),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn set_milestone_owner(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<OwnerRequest>,
) -> Result<HttpResponse, ApiError> {
    match assignment
        .set_milestone_owner(MilestoneId(*path), UserId(body.owner_id))
        .await
    {
        Ok(milestone) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}

/// Clear Milestone Owner
///
/// Remove the milestone's owner. Works even when the current owner has since
/// been deactivated, and clearing an unowned milestone is harmless. Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/milestones/{id}/owner",
    tags = ["milestones"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Milestone identifier")),
    responses(
        (status = 200, description = "The updated milestone", body = MilestoneResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No milestone with this id", body = ApiError)
    )
)]
pub async fn clear_milestone_owner(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match assignment.clear_milestone_owner(MilestoneId(*path)).await {
        Ok(milestone) => {
            let people = resolve_summaries(users.as_ref(), [milestone.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(milestone_response(&milestone, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}
