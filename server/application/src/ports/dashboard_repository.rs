//! Read-model port for the dashboard summary (roadmap 3.15, D22).
//!
//! The dashboard reads plain read types — not goal or milestone entities:
//! the summary needs only statuses and a few fields, so counting happens in
//! one place instead of materializing every row.

use chrono::{DateTime, NaiveDate, Utc};
use domain::{GoalId, MilestoneId, Status, UserId};

use crate::ports::RepositoryError;

/// The kind of entity a dashboard row counts or lists. Goals and milestones
/// are always separate series (D22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardKind {
    Goal,
    Milestone,
}

/// One status's share of a rollup: how many items carry that effective
/// status, and how many of those are manually set (D12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCount {
    pub count: u64,
    pub manual: u64,
}

/// The effective-status rollup for one kind (roadmap 3.15, D22): all four
/// statuses are always present, zero when none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusRollup {
    pub on_track: StatusCount,
    pub at_risk: StatusCount,
    pub off_track: StatusCount,
    pub complete: StatusCount,
}

impl StatusRollup {
    /// Count one item with the given effective status, manually set or not.
    pub fn record(&mut self, status: Status, manual: bool) {
        let entry = match status {
            Status::OnTrack => &mut self.on_track,
            Status::AtRisk => &mut self.at_risk,
            Status::OffTrack => &mut self.off_track,
            Status::Complete => &mut self.complete,
        };
        entry.count += 1;
        if manual {
            entry.manual += 1;
        }
    }

    /// Every item counted, across all statuses.
    pub fn total(&self) -> u64 {
        self.on_track.count + self.at_risk.count + self.off_track.count + self.complete.count
    }

    /// The manually set items, across all statuses.
    pub fn manual(&self) -> u64 {
        self.on_track.manual + self.at_risk.manual + self.off_track.manual + self.complete.manual
    }
}

/// The entity a manually-set row refers to: its kind and id together, so the
/// two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardEntityId {
    Goal(GoalId),
    Milestone(MilestoneId),
}

/// A goal or milestone whose status is manually set (roadmap 3.15, D22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManuallySetItem {
    /// The entity this row describes; carries its kind and id.
    pub entity: DashboardEntityId,
    pub title: String,
    /// The effective status — the manual override itself, since one is set.
    pub status: Status,
    /// The stored automatic status, reported alongside (D22).
    pub automatic_status: Status,
    pub target_date: Option<NaiveDate>,
}

/// One UTC day of a status trend series (roadmap 3.15, D22): how many items
/// sat under each status that day, how many of those were manually set, and —
/// for milestones — the sum and count of percents so the average is divided
/// out once, in Rust, without float surprises. A date with no snapshots never
/// produces a row (no carry-forward).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrendRow {
    pub date: NaiveDate,
    pub on_track: u64,
    pub at_risk: u64,
    pub off_track: u64,
    pub complete: u64,
    /// The items whose latest snapshot of the day was manually set.
    pub manual: u64,
    /// Sum of `percent_complete` over the day's rows that carry one (0 for goals).
    pub percent_sum: i64,
    /// How many of the day's rows carry a percent (0 for goals).
    pub percent_count: u64,
}

/// The read model behind the dashboard summary (roadmap 3.15, D22): plain
/// counts and rows over the stored statuses, never the entities themselves.
#[async_trait::async_trait]
pub trait DashboardRepository: Send + Sync {
    /// The effective-status rollup for one kind (D22): every row counted by
    /// its effective status — the manual override when set, else the stored
    /// automatic value — with how many of them are manually set. All four
    /// statuses present, zero when none.
    async fn status_rollup(&self, kind: DashboardKind) -> Result<StatusRollup, RepositoryError>;

    /// Every goal and milestone that has a manual override (D22): goals
    /// before milestones, each in the shared default order — target date
    /// ascending NULLS LAST, then created_at, then id.
    async fn manually_set_items(&self) -> Result<Vec<ManuallySetItem>, RepositoryError>;

    /// The status trend for one kind over an inclusive UTC date window (D22):
    /// for each (entity, date) the entity's LATEST snapshot of that day is
    /// taken — `taken_at` descending, then `id` descending — regardless of
    /// reason (`daily` or `manual_change`), then grouped by date. A date with
    /// no snapshots produces no row (no carry-forward); a deleted entity's
    /// history is already gone (cascade). Ordered by date ascending. Reads
    /// only the progress snapshots, never the entities.
    async fn status_trend(
        &self,
        kind: DashboardKind,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<TrendRow>, RepositoryError>;

    /// The timeline read model (D22): every goal and milestone matching the
    /// filter, each in the shared default order — target date ascending
    /// NULLS LAST, then created_at, then id — plus every goal↔milestone link
    /// of the returned goals. The date bounds keep the list endpoints'
    /// semantics (roadmap 3.10): inclusive, and when either is given undated
    /// rows never match; with no bound undated rows are included (last).
    async fn timeline(&self, filter: TimelineFilter) -> Result<TimelineData, RepositoryError>;
}

/// The timeline's date filter (roadmap 3.15, D22): optional inclusive target
/// date bounds with the list endpoints' semantics — when either bound is
/// given, undated rows never match; with no bound, undated rows are included
/// (last). An inverted range matches nothing and is not an error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimelineFilter {
    /// Only rows whose target date is on or after this date.
    pub target_after: Option<NaiveDate>,
    /// Only rows whose target date is on or before this date.
    pub target_before: Option<NaiveDate>,
}

/// One goal row of the timeline read model (roadmap 3.15, D22): the stored
/// automatic status plus the optional override — the effective status and its
/// source are derived from these two, exactly like the domain does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineGoal {
    pub id: GoalId,
    pub title: String,
    /// The stored automatic status (D19).
    pub automatic_status: Status,
    /// The manual override when set (roadmap 3.2).
    pub status_override: Option<Status>,
    pub target_date: Option<NaiveDate>,
    pub owner_id: Option<UserId>,
    /// When the goal was created; its UTC calendar date is the timeline's
    /// start date (D19).
    pub created_at: DateTime<Utc>,
}

/// One milestone row of the timeline read model (roadmap 3.15, D22); see
/// [`TimelineGoal`] for the status fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineMilestone {
    pub id: MilestoneId,
    pub title: String,
    /// The stored automatic status (D19).
    pub automatic_status: Status,
    /// The manual override when set (roadmap 3.2).
    pub status_override: Option<Status>,
    pub target_date: Option<NaiveDate>,
    pub owner_id: Option<UserId>,
    /// When the milestone was created; its UTC calendar date is the
    /// timeline's start date (D19 — a milestone's start is its creation).
    pub created_at: DateTime<Utc>,
}

/// The read model behind the dashboard timeline (roadmap 3.15, D22): goals
/// and milestones in the shared default order, plus the links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineData {
    pub goals: Vec<TimelineGoal>,
    pub milestones: Vec<TimelineMilestone>,
    /// Every goal↔milestone link of the returned goals, as (goal_id,
    /// milestone_id) pairs — grouped per goal (goal id ascending), each
    /// group in the linked milestone's shared default order, so a goal's
    /// `milestone_ids` are stable. A link to a milestone outside the
    /// requested window is included on purpose (D22); a milestone linked to
    /// two goals appears under both.
    pub links: Vec<(GoalId, MilestoneId)>,
}
