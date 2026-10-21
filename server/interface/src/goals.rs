//! Goals HTTP API: the `/api/goals` handlers and their JSON DTOs.
//!
//! The DTOs are deliberately separate types from [`domain::Goal`]: the wire
//! format is a contract with API clients and should be able to evolve
//! independently of the domain model.

use actix_web::{HttpResponse, web};
use application::assignment::AssignmentService;
use application::goal_list::GoalListService;
use application::goal_service::GoalService;
use application::pagination::{PageRequest, PageRequestError};
use application::ports::{GoalListFilter, GoalRepository, UserRepository};
use application::status_override::StatusOverrideService;
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Goal, GoalId, Status, StatusSource, UserId};
use infrastructure::repositories::PostgresGoalRepository;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::access::{EditAccess, ViewAccess};
use crate::error::{
    ApiError, assignment_error_response, repo_error_response, status_override_error_response,
};
use crate::openapi::{StatusDoc, StatusSourceDoc};
use crate::people::{OwnerRequest, PersonSummary, resolve_summaries};

/// JSON shape of a goal in responses.
#[derive(Serialize, ToSchema)]
pub struct GoalResponse {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// The status as it stands now: the manual setting if one is active,
    /// otherwise the automatic value worked out from the milestones.
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Whether [`GoalResponse::status`] was set by hand ("manual") or worked
    /// out by the system ("automatic").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    /// The status the system works out from the goal's linked milestones,
    /// reported even while a manual setting is active — so a client can show
    /// "manually set to At Risk (automatic: On Track)" (roadmap 3.13).
    #[schema(value_type = StatusDoc)]
    pub automatic_status: Status,
    pub target_date: Option<NaiveDate>,
    /// The person the goal is owned by, or null when unowned (D17). A
    /// deactivated owner still appears, with `active` false.
    pub owner: Option<PersonSummary>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Build the response for one goal, looking its owner up in the batch
/// resolved by [`resolve_summaries`] — one user query per response or page,
/// never one per row (D17). `pub(crate)` so the goal–milestone link lists
/// reuse it.
pub(crate) fn goal_response(goal: &Goal, people: &HashMap<Uuid, PersonSummary>) -> GoalResponse {
    // The wire shape reports the effective status and where it came
    // from, plus the automatic value underneath any manual setting.
    let mut response = GoalResponse {
        id: goal.id.0,
        title: goal.title.clone(),
        description: goal.description.clone(),
        status: goal.effective_status(),
        status_source: goal.status_source(),
        automatic_status: goal.status,
        target_date: goal.target_date,
        owner: None,
        created_at: goal.created_at,
        updated_at: goal.updated_at,
    };
    if let Some(owner_id) = goal.owner_id {
        response.owner = people.get(&owner_id.0).cloned();
    }
    response
}

/// Body for `PUT /api/goals/{id}`. The id, status, and timestamps are
/// server-managed and never accepted from the client; the owner is not part
/// of an ordinary update — only creation and the dedicated owner routes set
/// it (D17).
#[derive(Deserialize, ToSchema)]
pub struct GoalRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
}

/// Body for `POST /api/goals`: the update fields plus the optional owner.
#[derive(Deserialize, ToSchema)]
pub struct CreateGoalRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
    /// The user to make the owner; must be an existing active account.
    /// Omitted or null leaves the goal unowned.
    #[serde(default)]
    pub owner_id: Option<Uuid>,
}

/// Body for `PUT /api/goals/{id}/status-override` and
/// `PUT /api/milestones/{id}/status-override`: the status to set by hand. An
/// unknown status string is a 400, rejected by JSON parsing before the
/// handler runs.
#[derive(Deserialize, ToSchema)]
pub struct StatusOverrideRequest {
    #[schema(value_type = StatusDoc)]
    pub status: Status,
}

/// One page of goals plus the pagination metadata (roadmap 3.10, D16):
/// `total` is the number of goals matching the filters, ignoring paging, so
/// a client can tell how many pages there are without fetching them all.
#[derive(Serialize, ToSchema)]
pub struct GoalPage {
    pub items: Vec<GoalResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u64,
}

/// The raw form of the list query parameters shared by the goal and
/// milestone lists: every value is still a plain string because each handler
/// validates its own parameters so a 400 can name the offending one.
pub(crate) struct RawListQuery {
    pub limit: Option<String>,
    pub offset: Option<String>,
    pub status: Vec<String>,
    pub q: Option<String>,
    pub target_after: Option<String>,
    pub target_before: Option<String>,
}

/// Group the raw query-string pairs into a [`RawListQuery`]. A derived
/// visitor cannot express what a query string allows: serde_urlencoded hands
/// every key=value through as a scalar (a `Vec` field would be rejected
/// outright) and errors on repeated keys, but `status` must accept both
/// repeats and comma-separated values. So the pairs are grouped here:
/// `status` accumulates, every other parameter may appear at most once (a
/// repeat is an error naming it), and unknown parameters are ignored — the
/// same leniency the derive had.
pub(crate) fn collect_list_query(pairs: Vec<(String, String)>) -> Result<RawListQuery, String> {
    let mut query = RawListQuery {
        limit: None,
        offset: None,
        status: Vec::new(),
        q: None,
        target_after: None,
        target_before: None,
    };
    for (key, value) in pairs {
        match key.as_str() {
            "status" => query.status.push(value),
            "limit" | "offset" | "q" | "target_after" | "target_before" => {
                let slot: &mut Option<String> = match key.as_str() {
                    "limit" => &mut query.limit,
                    "offset" => &mut query.offset,
                    "q" => &mut query.q,
                    "target_after" => &mut query.target_after,
                    _ => &mut query.target_before,
                };
                if slot.is_some() {
                    return Err(format!("duplicate parameter {key}"));
                }
                *slot = Some(value);
            }
            _ => {}
        }
    }
    Ok(query)
}

/// Validate the raw `limit` and `offset` into a [`PageRequest`]; every
/// failure is a 400 `invalid_query` naming the offending parameter.
pub(crate) fn parse_page_request(
    limit: Option<&str>,
    offset: Option<&str>,
) -> Result<PageRequest, ApiError> {
    let limit = limit.map(|raw| {
        raw.parse::<i32>().map_err(|_| {
            ApiError::invalid_query(format!("limit must be a whole number, got {raw:?}"))
        })
    });
    let offset = offset.map(|raw| {
        raw.parse::<i64>().map_err(|_| {
            ApiError::invalid_query(format!("offset must be a whole number, got {raw:?}"))
        })
    });
    PageRequest::new(limit.transpose()?, offset.transpose()?).map_err(|err| match err {
        PageRequestError::LimitOutOfRange(value) => ApiError::invalid_query(format!(
            "limit must be between 1 and {}, got {value}",
            PageRequest::MAX_PAGE_LIMIT
        )),
        PageRequestError::NegativeOffset(value) => {
            ApiError::invalid_query(format!("offset must not be negative, got {value}"))
        }
    })
}

/// Validate the raw `q` search term: trimmed, empty after trimming means "no
/// search", at most 100 characters.
pub(crate) fn parse_q(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let q = raw
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(ToOwned::to_owned);
    let q_len = q.as_deref().map_or(0, |q| q.chars().count());
    if q_len > 100 {
        return Err(ApiError::invalid_query(format!(
            "q must be at most 100 characters, got {q_len}"
        )));
    }
    Ok(q)
}

/// Validate a raw `YYYY-MM-DD` date bound; every failure is a 400
/// `invalid_query` naming the offending parameter.
pub(crate) fn parse_target_date(
    raw: Option<&str>,
    param: &str,
) -> Result<Option<NaiveDate>, ApiError> {
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

/// Query params for `GET /api/goals`. Every parameter is optional and narrows
/// the result set; they are all plain strings here because the handler
/// validates each one itself so a 400 can name the offending parameter.
#[derive(IntoParams)]
pub struct GoalListQuery {
    /// How many goals per page (1–200). Defaults to 50.
    pub limit: Option<String>,
    /// How many matching goals to skip before the page starts. Defaults to 0.
    pub offset: Option<String>,
    /// Only goals in these statuses (`on_track`, `at_risk`, `off_track`,
    /// `complete`). The effective status is matched: a manual setting wins
    /// over the automatic one. May be given several times or comma-separated.
    pub status: Vec<String>,
    /// Case-insensitive substring match on the goal title.
    pub q: Option<String>,
    /// Only goals whose target date is on or after this date (`YYYY-MM-DD`).
    pub target_after: Option<String>,
    /// Only goals whose target date is on or before this date (`YYYY-MM-DD`).
    pub target_before: Option<String>,
}

impl<'de> Deserialize<'de> for GoalListQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let raw = collect_list_query(pairs).map_err(serde::de::Error::custom)?;
        Ok(GoalListQuery {
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
fn parse_goal_list_query(
    params: &GoalListQuery,
) -> Result<(GoalListFilter, PageRequest), ApiError> {
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
        GoalListFilter {
            statuses,
            q,
            target_after,
            target_before,
        },
        page_request,
    ))
}

/// Create Goal
///
/// Create a new goal - a strategic outcome the school is working toward,
/// optionally naming an owner. Requires the Staff or Admin role.
#[utoipa::path(
    post,
    path = "/api/goals",
    tags = ["goals"],
    security(("session_cookie" = [])),
    request_body = CreateGoalRequest,
    responses(
        (status = 201, description = "Goal created", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No user with the given owner_id", body = ApiError),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn create_goal(
    service: web::Data<GoalService>,
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    body: web::Json<CreateGoalRequest>,
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
    let goal = Goal {
        id: GoalId::new(),
        title: body.title.clone(),
        description: body.description.clone(),
        // A new goal has no milestones yet, so the rollup rule reports OnTrack.
        status: Status::OnTrack,
        status_override: None,
        target_date: body.target_date,
        owner_id,
        created_at: now,
        updated_at: now,
    };
    match service.create(goal).await {
        Ok(goal) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Created().json(goal_response(&goal, &people)))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// List Goals
///
/// List goals, one page at a time. Every query parameter is optional and
/// narrows the result set; together they are combined with AND. The status
/// filter matches the effective status — a manual setting wins over the
/// automatic value. The order is fixed: target date ascending (goals without
/// one last), then creation time, then id — so pages are stable. A filter
/// that matches nothing is an empty page, not an error. Any signed-in user
/// may read.
#[utoipa::path(
    get,
    path = "/api/goals",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(GoalListQuery),
    responses(
        (status = 200, description = "One page of the matching goals plus the total count", body = GoalPage),
        (
            status = 400,
            description = "A query parameter is missing or malformed; the message names it",
            body = ApiError
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_goals(
    query: web::Query<GoalListQuery>,
    service: web::Data<GoalListService>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let (filter, page_request) = parse_goal_list_query(&query.0)?;
    match service.list_page(&filter, &page_request).await {
        Ok(page) => {
            // One user query for the whole page's owners (D17).
            let people =
                resolve_summaries(users.as_ref(), page.items.iter().map(|goal| goal.owner_id))
                    .await
                    .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(GoalPage {
                items: page
                    .items
                    .iter()
                    .map(|goal| goal_response(goal, &people))
                    .collect(),
                total: page.total,
                limit: page.limit,
                offset: page.offset,
            }))
        }
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Get Individual Goal
///
/// Fetch a single goal by its ID. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The goal", body = GoalResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn get_goal(
    goals: web::Data<PostgresGoalRepository>,
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.find_by_id(GoalId(*path)).await {
        Ok(Some(goal)) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Update Goal
///
/// Update a goal's title, description, or target date. Status and timestamps are managed by the server.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = GoalRequest,
    responses(
        (status = 200, description = "The updated goal", body = GoalResponse),
        (status = 400, description = "Title is missing or blank", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn update_goal(
    goals: web::Data<PostgresGoalRepository>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<GoalRequest>,
) -> Result<HttpResponse, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    let id = GoalId(*path);
    match goals.find_by_id(id).await {
        Ok(Some(existing)) => {
            // The spread carries the owner through untouched: an ordinary
            // update neither sets nor clears it (D17).
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
                Ok(goal) => {
                    let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                        .await
                        .map_err(repo_error_response)?;
                    Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
                }
                Err(err) => Err(repo_error_response(err)),
            }
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Delete Goal
///
/// Delete a goal, including its links to any milestones.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 204, description = "Goal deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn delete_goal(
    goals: web::Data<PostgresGoalRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match goals.delete(GoalId(*path)).await {
        Ok(()) => Ok(HttpResponse::NoContent().finish()),
        Err(err) => Err(repo_error_response(err)),
    }
}

/// Set Goal Status Override
///
/// Manually set a goal's status. The manual setting is sticky: it holds until
/// cleared, even if the goal's milestones change in the meantime.
/// Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{id}/status-override",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = StatusOverrideRequest,
    responses(
        (status = 200, description = "The goal with its new status", body = GoalResponse),
        (status = 400, description = "Unknown status value", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn set_goal_status_override(
    overrides: web::Data<StatusOverrideService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<StatusOverrideRequest>,
) -> Result<HttpResponse, ApiError> {
    match overrides
        .set_goal_override(GoalId(*path), body.status)
        .await
    {
        Ok(goal) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
        }
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Clear Goal Status Override
///
/// Remove a goal's manual status setting so it returns to automatic — the
/// status worked out from its milestones.
/// Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}/status-override",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The goal with its automatic status", body = GoalResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn clear_goal_status_override(
    overrides: web::Data<StatusOverrideService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match overrides.clear_goal_override(GoalId(*path)).await {
        Ok(goal) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
        }
        Err(err) => Err(status_override_error_response(err)),
    }
}

/// Set Goal Owner
///
/// Name the person who owns the goal. Setting the person it is already owned
/// by is harmless and leaves `updated_at` alone. Requires the Staff or Admin role.
#[utoipa::path(
    put,
    path = "/api/goals/{id}/owner",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    request_body = OwnerRequest,
    responses(
        (status = 200, description = "The updated goal", body = GoalResponse),
        (status = 400, description = "Malformed body or owner_id is not a UUID", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (
            status = 404,
            description = "No goal with this id, or no user with the given owner_id",
            body = ApiError
        ),
        (status = 409, description = "The user is deactivated", body = ApiError)
    )
)]
pub async fn set_goal_owner(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
    body: web::Json<OwnerRequest>,
) -> Result<HttpResponse, ApiError> {
    match assignment
        .set_goal_owner(GoalId(*path), UserId(body.owner_id))
        .await
    {
        Ok(goal) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}

/// Clear Goal Owner
///
/// Remove the goal's owner. Works even when the current owner has since been
/// deactivated, and clearing an unowned goal is harmless. Requires the Staff or Admin role.
#[utoipa::path(
    delete,
    path = "/api/goals/{id}/owner",
    tags = ["goals"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Goal identifier")),
    responses(
        (status = 200, description = "The updated goal", body = GoalResponse),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Staff or Admin role", body = ApiError),
        (status = 404, description = "No goal with this id", body = ApiError)
    )
)]
pub async fn clear_goal_owner(
    assignment: web::Data<AssignmentService>,
    users: web::Data<dyn UserRepository>,
    _access: EditAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    match assignment.clear_goal_owner(GoalId(*path)).await {
        Ok(goal) => {
            let people = resolve_summaries(users.as_ref(), [goal.owner_id])
                .await
                .map_err(repo_error_response)?;
            Ok(HttpResponse::Ok().json(goal_response(&goal, &people)))
        }
        Err(err) => Err(assignment_error_response(err)),
    }
}
