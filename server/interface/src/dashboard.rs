//! The dashboard endpoints (roadmap 3.15, D22): read-only overviews for any
//! signed-in role — the live status summary, the status trend series from the
//! progress snapshots, and the timeline of goals and milestones against their
//! target dates.

use actix_web::{HttpResponse, web};
use application::dashboard::{
    DashboardError, DashboardService, DashboardSummary, MAX_TREND_WINDOW_DAYS, Timeline,
    TimelineGoalItem, TimelineMilestoneItem, TrendError, TrendPoint, Trends,
};
use application::ports::{
    DashboardEntityId, ManuallySetItem, StatusCount, StatusRollup, TimelineFilter,
};
use chrono::{DateTime, NaiveDate, Utc};
use domain::{Status, StatusSource};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::access::ViewAccess;
use crate::error::{ApiError, repo_error_response};
use crate::goals::parse_target_date;
use crate::openapi::{StatusDoc, StatusSourceDoc};
use crate::people::PersonSummary;

/// Wire shape of [`application::ports::DashboardKind`]: a snake_case string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DashboardKindDoc {
    Goal,
    Milestone,
}

/// One status's share of a kind summary (roadmap 3.15, D22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct StatusCountResponse {
    /// How many items carry this effective status.
    pub count: u64,
    /// How many of those are manually set.
    pub manual: u64,
}

/// The four statuses of a kind summary; all always present (D22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct ByStatus {
    pub on_track: StatusCountResponse,
    pub at_risk: StatusCountResponse,
    pub off_track: StatusCountResponse,
    pub complete: StatusCountResponse,
}

/// The goals' or milestones' share of the summary (roadmap 3.15, D22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct KindSummary {
    /// Every item of this kind.
    pub total: u64,
    /// The manually set ones.
    pub manual: u64,
    /// Counts per effective status; all four always present.
    pub by_status: ByStatus,
}

/// A goal or milestone whose status is manually set (roadmap 3.15, D22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManuallySetItemResponse {
    /// Which kind of entity this is.
    pub kind: DashboardKindDoc,
    pub id: Uuid,
    pub title: String,
    /// The effective status — the manual override itself.
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// The stored automatic status, alongside.
    #[schema(value_type = StatusDoc)]
    pub automatic_status: Status,
    /// `YYYY-MM-DD`, or null when undated.
    pub target_date: Option<NaiveDate>,
}

/// The dashboard summary (roadmap 3.15, D22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DashboardSummaryResponse {
    /// When the summary was taken (UTC, RFC 3339).
    pub as_of: DateTime<Utc>,
    pub goals: KindSummary,
    pub milestones: KindSummary,
    /// Every manually set goal and milestone, goals before milestones.
    pub manually_set: Vec<ManuallySetItemResponse>,
}

fn count_response(count: StatusCount) -> StatusCountResponse {
    StatusCountResponse {
        count: count.count,
        manual: count.manual,
    }
}

impl From<&StatusRollup> for KindSummary {
    fn from(rollup: &StatusRollup) -> Self {
        Self {
            total: rollup.total(),
            manual: rollup.manual(),
            by_status: ByStatus {
                on_track: count_response(rollup.on_track),
                at_risk: count_response(rollup.at_risk),
                off_track: count_response(rollup.off_track),
                complete: count_response(rollup.complete),
            },
        }
    }
}

impl From<&ManuallySetItem> for ManuallySetItemResponse {
    fn from(item: &ManuallySetItem) -> Self {
        let (kind, id) = match item.entity {
            DashboardEntityId::Goal(id) => (DashboardKindDoc::Goal, id.0),
            DashboardEntityId::Milestone(id) => (DashboardKindDoc::Milestone, id.0),
        };
        Self {
            kind,
            id,
            title: item.title.clone(),
            status: item.status,
            automatic_status: item.automatic_status,
            target_date: item.target_date,
        }
    }
}

impl From<&DashboardSummary> for DashboardSummaryResponse {
    fn from(summary: &DashboardSummary) -> Self {
        Self {
            as_of: summary.as_of,
            goals: (&summary.goals).into(),
            milestones: (&summary.milestones).into(),
            manually_set: summary
                .manually_set
                .iter()
                .map(ManuallySetItemResponse::from)
                .collect(),
        }
    }
}

fn dashboard_error_response(error: DashboardError) -> ApiError {
    match error {
        DashboardError::Repository(repository) => repo_error_response(repository),
    }
}

/// Dashboard summary
///
/// A read-only status overview: live counts of goals and milestones by their effective status — the manual override when one is set, otherwise the stored automatic value (D12) — all four statuses always present, zero when empty, each with how many of its items are manually set; plus every manually set goal and milestone with its automatic status alongside. It reads only: it never changes anything. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/dashboard/summary",
    tags = ["dashboard"],
    security(("session_cookie" = [])),
    responses(
        (
            status = 200,
            description = "The dashboard summary",
            body = DashboardSummaryResponse
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn summary(
    service: web::Data<DashboardService>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let summary = service.summary().await.map_err(dashboard_error_response)?;
    Ok(HttpResponse::Ok().json(DashboardSummaryResponse::from(&summary)))
}

/// The four statuses of one trend point; all always present (D22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct TrendCounts {
    pub on_track: u64,
    pub at_risk: u64,
    pub off_track: u64,
    pub complete: u64,
}

/// One UTC day of a status trend series (roadmap 3.15, D22). A day with no
/// snapshots never produces a point.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TrendPointResponse {
    /// `YYYY-MM-DD`.
    pub date: NaiveDate,
    /// How many items sat under each status that day; all four present.
    pub counts: TrendCounts,
    /// Every item counted that day.
    pub total: u64,
    /// The items whose latest snapshot of the day was manually set.
    pub manual: u64,
    /// Milestones only: the average percent complete of that day's rows,
    /// one decimal, or null when none carry one. Omitted for goals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_percent_complete: Option<f64>,
}

/// The status trend series (roadmap 3.15, D22): goals and milestones as
/// separate day-by-day series over an inclusive UTC window.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TrendsResponse {
    /// The first day of the window (inclusive), `YYYY-MM-DD`.
    pub from: NaiveDate,
    /// The last day of the window (inclusive), `YYYY-MM-DD`.
    pub to: NaiveDate,
    pub goals: Vec<TrendPointResponse>,
    pub milestones: Vec<TrendPointResponse>,
}

impl From<&TrendPoint> for TrendPointResponse {
    fn from(point: &TrendPoint) -> Self {
        Self {
            date: point.date,
            counts: TrendCounts {
                on_track: point.on_track,
                at_risk: point.at_risk,
                off_track: point.off_track,
                complete: point.complete,
            },
            total: point.total,
            manual: point.manual,
            average_percent_complete: point.average_percent_complete,
        }
    }
}

impl From<&Trends> for TrendsResponse {
    fn from(trends: &Trends) -> Self {
        Self {
            from: trends.from,
            to: trends.to,
            goals: trends.goals.iter().map(TrendPointResponse::from).collect(),
            milestones: trends
                .milestones
                .iter()
                .map(TrendPointResponse::from)
                .collect(),
        }
    }
}

/// Query params for `GET /api/dashboard/trends`. Both are optional and plain
/// strings so a 400 can name the offending parameter.
#[derive(IntoParams)]
pub struct TrendQuery {
    /// The first day of the window (inclusive), `YYYY-MM-DD`. Defaults to 89 days before `to`, giving a 90-day window.
    pub from: Option<String>,
    /// The last day of the window (inclusive), `YYYY-MM-DD`. Defaults to today (UTC).
    pub to: Option<String>,
}

impl<'de> Deserialize<'de> for TrendQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A derived impl would keep one of a repeated parameter silently;
        // collect the pairs so a duplicate is an error naming it (like the
        // list queries). Unknown parameters are ignored.
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let mut from: Option<String> = None;
        let mut to: Option<String> = None;
        for (key, value) in pairs {
            let slot = match key.as_str() {
                "from" => &mut from,
                "to" => &mut to,
                _ => continue,
            };
            if slot.is_some() {
                return Err(serde::de::Error::custom(format!(
                    "duplicate parameter {key}"
                )));
            }
            *slot = Some(value);
        }
        Ok(TrendQuery { from, to })
    }
}

fn trends_error_response(error: TrendError) -> ApiError {
    match error {
        // Both range failures can only happen when an explicit `from` was
        // sent — the defaults always give a valid 90-day window — so it is
        // always the offending parameter.
        TrendError::FromAfterTo { from, to } => {
            ApiError::invalid_query(format!("from must not be after to (from {from}, to {to})"))
        }
        TrendError::WindowTooLong { days } => ApiError::invalid_query(format!(
            "from opens a window of {days} days; at most {MAX_TREND_WINDOW_DAYS} are allowed"
        )),
        TrendError::Repository(repository) => repo_error_response(repository),
    }
}

/// Status trend
///
/// How many goals and milestones sat under each status on each day of a window, read from the progress snapshots (roadmap 3.14): for every item and day only that day's latest snapshot counts, whatever its reason, so a manual change made during the day shows up in that day's point as manually set. Days without snapshots are omitted — there is no carry-forward — and deleted items leave no trace. `from` and `to` are inclusive UTC dates in `YYYY-MM-DD` form; `to` defaults to today and `from` to 89 days before it (a 90-day window), and the window may span at most 366 days. Milestone points carry the average percent complete of that day's rows, one decimal, or null when none have one. It reads only: it never changes anything. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/dashboard/trends",
    tags = ["dashboard"],
    security(("session_cookie" = [])),
    params(
        TrendQuery,
    ),
    responses(
        (
            status = 200,
            description = "The status trend series",
            body = TrendsResponse
        ),
        (status = 400, description = "A malformed or out-of-range `from`/`to`", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn trends(
    service: web::Data<DashboardService>,
    query: web::Query<TrendQuery>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let from = parse_target_date(query.from.as_deref(), "from")?;
    let to = parse_target_date(query.to.as_deref(), "to")?;
    let trends = service
        .trends(from, to)
        .await
        .map_err(trends_error_response)?;
    Ok(HttpResponse::Ok().json(TrendsResponse::from(&trends)))
}

/// One goal of the dashboard timeline (roadmap 3.15, D22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineGoalResponse {
    pub id: Uuid,
    pub title: String,
    /// The UTC calendar date it was created, `YYYY-MM-DD` (D19).
    pub start_date: NaiveDate,
    /// `YYYY-MM-DD`, or null when undated.
    pub target_date: Option<NaiveDate>,
    /// The effective status — the manual setting if one is active (D12).
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Where [`Self::status`] came from ("automatic" or "manual").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    /// The stored automatic status, reported alongside (D22).
    #[schema(value_type = StatusDoc)]
    pub automatic_status: Status,
    /// The owner's summary, or null when unowned (D17); a deactivated owner
    /// still appears, with `active` false.
    pub owner: Option<PersonSummary>,
    /// Every linked milestone id — including milestones outside the
    /// requested window (D22).
    pub milestone_ids: Vec<Uuid>,
}

/// One milestone of the dashboard timeline (roadmap 3.15, D22); see
/// [`TimelineGoalResponse`] for the shared fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineMilestoneResponse {
    pub id: Uuid,
    pub title: String,
    /// The UTC calendar date it was created, `YYYY-MM-DD` (D19).
    pub start_date: NaiveDate,
    /// `YYYY-MM-DD`, or null when undated.
    pub target_date: Option<NaiveDate>,
    /// The effective status — the manual setting if one is active (D12).
    #[schema(value_type = StatusDoc)]
    pub status: Status,
    /// Where [`Self::status`] came from ("automatic" or "manual").
    #[schema(value_type = StatusSourceDoc)]
    pub status_source: StatusSource,
    /// The stored automatic status, reported alongside (D22).
    #[schema(value_type = StatusDoc)]
    pub automatic_status: Status,
    /// The owner's summary, or null when unowned (D17); a deactivated owner
    /// still appears, with `active` false.
    pub owner: Option<PersonSummary>,
    /// The share of its tasks that are done, 0–100 (0 with no tasks).
    pub percent_complete: u8,
}

/// The dashboard timeline (roadmap 3.15, D22): goals and milestones against
/// their target dates, both in the shared default order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TimelineResponse {
    pub goals: Vec<TimelineGoalResponse>,
    pub milestones: Vec<TimelineMilestoneResponse>,
}

impl From<&TimelineGoalItem> for TimelineGoalResponse {
    fn from(item: &TimelineGoalItem) -> Self {
        Self {
            id: item.id.0,
            title: item.title.clone(),
            start_date: item.start_date,
            target_date: item.target_date,
            status: item.status,
            status_source: item.status_source,
            automatic_status: item.automatic_status,
            owner: item.owner.as_ref().map(PersonSummary::from),
            milestone_ids: item.milestone_ids.iter().map(|id| id.0).collect(),
        }
    }
}

impl From<&TimelineMilestoneItem> for TimelineMilestoneResponse {
    fn from(item: &TimelineMilestoneItem) -> Self {
        Self {
            id: item.id.0,
            title: item.title.clone(),
            start_date: item.start_date,
            target_date: item.target_date,
            status: item.status,
            status_source: item.status_source,
            automatic_status: item.automatic_status,
            owner: item.owner.as_ref().map(PersonSummary::from),
            percent_complete: item.percent_complete,
        }
    }
}

impl From<&Timeline> for TimelineResponse {
    fn from(timeline: &Timeline) -> Self {
        Self {
            goals: timeline
                .goals
                .iter()
                .map(TimelineGoalResponse::from)
                .collect(),
            milestones: timeline
                .milestones
                .iter()
                .map(TimelineMilestoneResponse::from)
                .collect(),
        }
    }
}

/// Query params for `GET /api/dashboard/timeline`. Both are optional and
/// plain strings so a 400 can name the offending parameter.
#[derive(IntoParams)]
pub struct TimelineQuery {
    /// Only rows whose target date is on or after this date, `YYYY-MM-DD`. When either bound is given, undated rows never match.
    pub target_after: Option<String>,
    /// Only rows whose target date is on or before this date, `YYYY-MM-DD`. When either bound is given, undated rows never match.
    pub target_before: Option<String>,
}

impl<'de> Deserialize<'de> for TimelineQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A derived impl would keep one of a repeated parameter silently;
        // collect the pairs so a duplicate is an error naming it (like the
        // list queries). Unknown parameters are ignored.
        let pairs: Vec<(String, String)> = Deserialize::deserialize(deserializer)?;
        let mut target_after: Option<String> = None;
        let mut target_before: Option<String> = None;
        for (key, value) in pairs {
            let slot = match key.as_str() {
                "target_after" => &mut target_after,
                "target_before" => &mut target_before,
                _ => continue,
            };
            if slot.is_some() {
                return Err(serde::de::Error::custom(format!(
                    "duplicate parameter {key}"
                )));
            }
            *slot = Some(value);
        }
        Ok(TimelineQuery {
            target_after,
            target_before,
        })
    }
}

/// Dashboard timeline
///
/// Goals and milestones against their target dates, both in the shared default order — target date ascending with undated rows last, then created, then id. Each row reports its effective status — the manual setting if one is active, otherwise the stored automatic value (D12) — with `status_source` and the automatic status alongside; goals list every linked milestone id, including milestones outside the requested window, and milestones carry their percent complete. `target_after` and `target_before` are optional inclusive target-date bounds in `YYYY-MM-DD` form: when either is given, undated rows never match, and with neither given undated rows are included (last); an inverted range matches nothing rather than erroring. It reads only: it never changes anything. Any signed-in user may read.
#[utoipa::path(
    get,
    path = "/api/dashboard/timeline",
    tags = ["dashboard"],
    security(("session_cookie" = [])),
    params(
        TimelineQuery,
    ),
    responses(
        (
            status = 200,
            description = "The dashboard timeline",
            body = TimelineResponse
        ),
        (status = 400, description = "A malformed `target_after`/`target_before` date", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn timeline(
    service: web::Data<DashboardService>,
    query: web::Query<TimelineQuery>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let target_after = parse_target_date(query.target_after.as_deref(), "target_after")?;
    let target_before = parse_target_date(query.target_before.as_deref(), "target_before")?;
    let timeline = service
        .timeline(TimelineFilter {
            target_after,
            target_before,
        })
        .await
        .map_err(dashboard_error_response)?;
    Ok(HttpResponse::Ok().json(TimelineResponse::from(&timeline)))
}
