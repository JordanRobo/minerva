//! The dashboard API (roadmap 3.15, D22): a read-only overview of the whole
//! plan — live counts of goals and milestones by effective status with the
//! manually set items flagged, the progress trend series from the snapshots,
//! and the timeline of goals and milestones against their target dates.
//! Read-only: nothing here writes, snapshots or recomputes (D5).

use chrono::{DateTime, NaiveDate, Utc};
use domain::{GoalId, MilestoneId, Status, StatusSource, UserId, UserSummary};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ports::{
    Clock, DashboardKind, DashboardRepository, ManuallySetItem, RepositoryError, StatusRollup,
    TaskCounts, TaskRepository, TimelineData, TimelineFilter, TrendRow, UserRepository,
};

/// An error from building the dashboard summary.
#[derive(Debug)]
pub enum DashboardError {
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for DashboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DashboardError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DashboardError {}

/// The longest inclusive trend window, in days (roadmap 3.15, D22): a year
/// plus one day, so a leap-year span is always coverable.
pub const MAX_TREND_WINDOW_DAYS: i64 = 366;

/// An error from building the dashboard trends (roadmap 3.15, D22). The
/// window-validation variants carry their values so a 400 can name the
/// offending parameter; nothing is clamped.
#[derive(Debug)]
pub enum TrendError {
    /// `from` is after `to`.
    FromAfterTo { from: NaiveDate, to: NaiveDate },
    /// The inclusive window is longer than [`MAX_TREND_WINDOW_DAYS`] days.
    WindowTooLong { days: i64 },
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for TrendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrendError::FromAfterTo { from, to } => {
                write!(f, "from must not be after to (from {from}, to {to})")
            }
            TrendError::WindowTooLong { days } => write!(
                f,
                "the window is {days} days; at most {} are allowed",
                MAX_TREND_WINDOW_DAYS
            ),
            TrendError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TrendError {}

/// The dashboard summary (roadmap 3.15, D22): taken at `as_of`, with the
/// goals' and milestones' rollups — every status present, zero when none —
/// and every manually set item, goals before milestones.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardSummary {
    /// When the summary was taken, from the injected clock.
    pub as_of: DateTime<Utc>,
    /// The goal rollup (see [`StatusRollup`]).
    pub goals: StatusRollup,
    /// The milestone rollup (see [`Self::goals`]).
    pub milestones: StatusRollup,
    /// Every manually set goal and milestone, goals before milestones.
    pub manually_set: Vec<ManuallySetItem>,
}

/// One UTC day of a status trend series (roadmap 3.15, D22): every item's
/// latest status of that day, from the progress snapshots only. All four
/// statuses are always present, zero when none. `average_percent_complete` is
/// set for milestones (None when no row that day carried a percent) and is
/// always None for goals.
#[derive(Debug, Clone, PartialEq)]
pub struct TrendPoint {
    pub date: NaiveDate,
    pub on_track: u64,
    pub at_risk: u64,
    pub off_track: u64,
    pub complete: u64,
    /// Every item counted that day.
    pub total: u64,
    /// The items whose latest snapshot of the day was manually set.
    pub manual: u64,
    /// Milestones only (see [`Self`]): one decimal, None when no row carried a
    /// percent; always None for goals.
    pub average_percent_complete: Option<f64>,
}

/// The dashboard trends (roadmap 3.15, D22): the resolved inclusive window and
/// one point per UTC date that has snapshots, goals and milestones as separate
/// series. A date with no snapshots appears in neither.
#[derive(Debug, Clone, PartialEq)]
pub struct Trends {
    /// The resolved start of the window (inclusive).
    pub from: NaiveDate,
    /// The resolved end of the window (inclusive).
    pub to: NaiveDate,
    pub goals: Vec<TrendPoint>,
    pub milestones: Vec<TrendPoint>,
}

/// One goal of the dashboard timeline (roadmap 3.15, D22): the row's data
/// with its owner resolved and its linked milestone ids attached.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineGoalItem {
    pub id: GoalId,
    pub title: String,
    /// The UTC calendar date of creation (D19).
    pub start_date: NaiveDate,
    pub target_date: Option<NaiveDate>,
    /// The effective status — the override when set, else the stored value
    /// (D12).
    pub status: Status,
    /// Where [`Self::status`] came from.
    pub status_source: StatusSource,
    /// The stored automatic status, reported alongside (D22).
    pub automatic_status: Status,
    /// The owner summary, or None when unowned (D17); a deactivated owner
    /// still resolves, flagged by `active`.
    pub owner: Option<UserSummary>,
    /// Every linked milestone id — including milestones outside the
    /// requested window (D22).
    pub milestone_ids: Vec<MilestoneId>,
}

/// One milestone of the dashboard timeline (roadmap 3.15, D22); see
/// [`TimelineGoalItem`] for the shared fields.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMilestoneItem {
    pub id: MilestoneId,
    pub title: String,
    /// The UTC calendar date of creation (D19 — a milestone's start is its
    /// creation).
    pub start_date: NaiveDate,
    pub target_date: Option<NaiveDate>,
    /// The effective status — the override when set, else the stored value
    /// (D12).
    pub status: Status,
    /// Where [`Self::status`] came from.
    pub status_source: StatusSource,
    /// The stored automatic status, reported alongside (D22).
    pub automatic_status: Status,
    /// The owner summary, or None when unowned (D17); a deactivated owner
    /// still resolves, flagged by `active`.
    pub owner: Option<UserSummary>,
    /// The share of the milestone's tasks that are done, 0–100 (0 with no
    /// tasks) — the same value the standard milestone responses report.
    pub percent_complete: u8,
}

/// The dashboard timeline (roadmap 3.15, D22): goals and milestones against
/// their target dates, both in the shared default order.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub goals: Vec<TimelineGoalItem>,
    pub milestones: Vec<TimelineMilestoneItem>,
}

/// Builds the dashboard summary over the read-model port (roadmap 3.15,
/// D22). Read-only: it never writes, snapshots or recomputes. The timeline
/// also resolves owners and milestone percents through the user and task
/// ports — one batched call of each per request, like the standard responses.
pub struct DashboardService {
    dashboard: Arc<dyn DashboardRepository>,
    users: Arc<dyn UserRepository>,
    tasks: Arc<dyn TaskRepository>,
    clock: Arc<dyn Clock>,
}

impl DashboardService {
    pub fn new(
        dashboard: Arc<dyn DashboardRepository>,
        users: Arc<dyn UserRepository>,
        tasks: Arc<dyn TaskRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            dashboard,
            users,
            tasks,
            clock,
        }
    }

    /// The live summary: both rollups and the manually-set list from the
    /// read model, stamped with the current time.
    pub async fn summary(&self) -> Result<DashboardSummary, DashboardError> {
        let goals = self
            .dashboard
            .status_rollup(DashboardKind::Goal)
            .await
            .map_err(DashboardError::Repository)?;
        let milestones = self
            .dashboard
            .status_rollup(DashboardKind::Milestone)
            .await
            .map_err(DashboardError::Repository)?;
        let manually_set = self
            .dashboard
            .manually_set_items()
            .await
            .map_err(DashboardError::Repository)?;
        Ok(DashboardSummary {
            as_of: self.clock.now(),
            goals,
            milestones,
            manually_set,
        })
    }

    /// The status trend series over an inclusive UTC date window (D22): each
    /// day shows every item's latest status of that day, read from the progress
    /// snapshots only — never live entity data. `to` defaults to today; `from`
    /// defaults to 89 days before `to` (a 90-day window). Read-only: it never
    /// writes, snapshots or recomputes.
    pub async fn trends(
        &self,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
    ) -> Result<Trends, TrendError> {
        let to = to.unwrap_or_else(|| self.clock.today());
        let from = from.unwrap_or(to - chrono::Duration::days(89));
        if from > to {
            return Err(TrendError::FromAfterTo { from, to });
        }
        let days = (to - from).num_days() + 1;
        if days > MAX_TREND_WINDOW_DAYS {
            return Err(TrendError::WindowTooLong { days });
        }
        let goals = self
            .dashboard
            .status_trend(DashboardKind::Goal, from, to)
            .await
            .map_err(TrendError::Repository)?;
        let milestones = self
            .dashboard
            .status_trend(DashboardKind::Milestone, from, to)
            .await
            .map_err(TrendError::Repository)?;
        Ok(Trends {
            from,
            to,
            goals: goals
                .into_iter()
                .map(|row| trend_point(row, None))
                .collect(),
            milestones: milestones
                .into_iter()
                .map(|row| trend_point(row, milestone_average(&row)))
                .collect(),
        })
    }

    /// The timeline over an optional inclusive target-date window (D22):
    /// every matching goal and milestone with its effective status, the
    /// automatic status alongside, its owner resolved and — for milestones —
    /// its percent complete. Owners are deduplicated into ONE
    /// `summaries_by_ids` call and percents come from ONE grouped task-counts
    /// lookup, the same batching the standard responses use (D17, roadmap
    /// 3.13). A goal's `milestone_ids` lists all its links, even milestones
    /// outside the window. Read-only: it never writes, snapshots or
    /// recomputes.
    pub async fn timeline(&self, filter: TimelineFilter) -> Result<Timeline, DashboardError> {
        let data: TimelineData = self
            .dashboard
            .timeline(filter)
            .await
            .map_err(DashboardError::Repository)?;

        // One grouped lookup for every milestone's task counts (roadmap 3.13).
        let milestone_ids: Vec<MilestoneId> = data.milestones.iter().map(|row| row.id).collect();
        let counts: HashMap<MilestoneId, TaskCounts> = self
            .tasks
            .task_counts_for_milestones(&milestone_ids)
            .await
            .map_err(DashboardError::Repository)?;

        // One user query for every owner, duplicates collapsed first (D17),
        // like `resolve_summaries` in the interface layer.
        let owner_ids: HashSet<UserId> = data
            .goals
            .iter()
            .map(|row| row.owner_id)
            .chain(data.milestones.iter().map(|row| row.owner_id))
            .flatten()
            .collect();
        let people: HashMap<UserId, UserSummary> = if owner_ids.is_empty() {
            HashMap::new()
        } else {
            let ids: Vec<UserId> = owner_ids.into_iter().collect();
            self.users
                .summaries_by_ids(&ids)
                .await
                .map_err(DashboardError::Repository)?
                .into_iter()
                .map(|summary| (summary.id, summary))
                .collect()
        };

        // Each goal's links, in the order the read model returned them —
        // already grouped per goal and stable.
        let mut milestone_ids_by_goal: HashMap<GoalId, Vec<MilestoneId>> = HashMap::new();
        for (goal_id, milestone_id) in data.links {
            milestone_ids_by_goal
                .entry(goal_id)
                .or_default()
                .push(milestone_id);
        }

        let goals = data
            .goals
            .into_iter()
            .map(|row| {
                let (status, status_source) =
                    status_and_source(row.automatic_status, row.status_override);
                TimelineGoalItem {
                    milestone_ids: milestone_ids_by_goal.remove(&row.id).unwrap_or_default(),
                    owner: row.owner_id.and_then(|id| people.get(&id).cloned()),
                    start_date: row.created_at.date_naive(),
                    id: row.id,
                    title: row.title,
                    status,
                    status_source,
                    automatic_status: row.automatic_status,
                    target_date: row.target_date,
                }
            })
            .collect();

        let milestones = data
            .milestones
            .into_iter()
            .map(|row| {
                let (status, status_source) =
                    status_and_source(row.automatic_status, row.status_override);
                // Every requested id is present in `counts` (zeros when
                // taskless); a missing entry degrades to zeros rather than
                // failing the response, like the milestone responses.
                let count = counts
                    .get(&row.id)
                    .copied()
                    .unwrap_or(TaskCounts { total: 0, done: 0 });
                TimelineMilestoneItem {
                    owner: row.owner_id.and_then(|id| people.get(&id).cloned()),
                    start_date: row.created_at.date_naive(),
                    id: row.id,
                    title: row.title,
                    status,
                    status_source,
                    automatic_status: row.automatic_status,
                    target_date: row.target_date,
                    percent_complete: (domain::percent_complete(count.total, count.done) * 100.0)
                        .round() as u8,
                }
            })
            .collect();

        Ok(Timeline { goals, milestones })
    }
}

/// The effective status and its source from the stored pair: the override
/// when set (D12), else the automatic value — the same rule the domain's
/// `effective_status()`/`status_source()` apply.
fn status_and_source(automatic: Status, override_status: Option<Status>) -> (Status, StatusSource) {
    match override_status {
        Some(status) => (status, StatusSource::ManualOverride),
        None => (automatic, StatusSource::Computed),
    }
}

/// Build one trend point from a day's row. `average_percent_complete` is passed
/// in so goals (always None) and milestones share the shape.
fn trend_point(row: TrendRow, average_percent_complete: Option<f64>) -> TrendPoint {
    TrendPoint {
        date: row.date,
        on_track: row.on_track,
        at_risk: row.at_risk,
        off_track: row.off_track,
        complete: row.complete,
        total: row.on_track + row.at_risk + row.off_track + row.complete,
        manual: row.manual,
        average_percent_complete,
    }
}

/// A milestone day's average percent complete across the rows that carry one,
/// or None when none did. Goals never reach this (their rows carry no percent).
fn milestone_average(row: &TrendRow) -> Option<f64> {
    (row.percent_count > 0).then(|| round_one_decimal(row.percent_sum, row.percent_count))
}

/// `sum / count` rounded to one decimal, half away from zero. Percents are
/// 0–100, so all values here are non-negative and "away from zero" is "up"; the
/// arithmetic stays in integers until the final division by 10, so no float
/// surprises accumulate.
fn round_one_decimal(sum: i64, count: u64) -> f64 {
    debug_assert!(count > 0);
    let den = count as i64;
    let tenths = sum * 10;
    let whole = tenths / den;
    let rem = tenths % den;
    let rounded = if 2 * rem >= den { whole + 1 } else { whole };
    rounded as f64 / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{DashboardEntityId, StatusCount, TimelineGoal, TimelineMilestone};
    use chrono::NaiveDate;
    use domain::{GoalId, MilestoneId, Status, Task, TaskId, TaskStatus, UserId, UserSummary};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    /// A canned read model: fixed rollups, manually-set rows and trend rows.
    struct FakeDashboard {
        goals: StatusRollup,
        milestones: StatusRollup,
        manually_set: Vec<ManuallySetItem>,
        goal_trend: Vec<TrendRow>,
        milestone_trend: Vec<TrendRow>,
        timeline: TimelineData,
    }

    #[async_trait::async_trait]
    impl DashboardRepository for FakeDashboard {
        async fn status_rollup(
            &self,
            kind: DashboardKind,
        ) -> Result<StatusRollup, RepositoryError> {
            Ok(match kind {
                DashboardKind::Goal => self.goals,
                DashboardKind::Milestone => self.milestones,
            })
        }

        async fn manually_set_items(&self) -> Result<Vec<ManuallySetItem>, RepositoryError> {
            Ok(self.manually_set.clone())
        }

        async fn status_trend(
            &self,
            kind: DashboardKind,
            _from: NaiveDate,
            _to: NaiveDate,
        ) -> Result<Vec<TrendRow>, RepositoryError> {
            Ok(match kind {
                DashboardKind::Goal => self.goal_trend.clone(),
                DashboardKind::Milestone => self.milestone_trend.clone(),
            })
        }

        async fn timeline(&self, _filter: TimelineFilter) -> Result<TimelineData, RepositoryError> {
            Ok(self.timeline.clone())
        }
    }

    /// A [`TrendRow`] for one date, most fields zero unless overridden.
    fn trend_row(date: NaiveDate) -> TrendRow {
        TrendRow {
            date,
            on_track: 0,
            at_risk: 0,
            off_track: 0,
            complete: 0,
            manual: 0,
            percent_sum: 0,
            percent_count: 0,
        }
    }

    /// A fixed clock: the summary's `as_of` is whatever the test says it is.
    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// Empty counting fakes for tests that never resolve owners or percents.
    fn empty_people_tasks() -> (Arc<CountingUserRepository>, Arc<CountingTaskRepository>) {
        (
            Arc::new(CountingUserRepository {
                summaries: HashMap::new(),
                calls: AtomicUsize::new(0),
                last_ids: std::sync::Mutex::new(Vec::new()),
            }),
            Arc::new(CountingTaskRepository {
                counts: HashMap::new(),
                calls: AtomicUsize::new(0),
                last_ids: std::sync::Mutex::new(Vec::new()),
            }),
        )
    }

    #[tokio::test]
    async fn the_summary_carries_both_rollups_the_items_and_the_clock_time() {
        let at = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap()
            .and_utc();
        let (users, tasks) = empty_people_tasks();
        let service = DashboardService::new(
            Arc::new(FakeDashboard {
                goals: StatusRollup {
                    on_track: StatusCount {
                        count: 3,
                        manual: 1,
                    },
                    ..Default::default()
                },
                milestones: StatusRollup {
                    complete: StatusCount {
                        count: 2,
                        manual: 0,
                    },
                    ..Default::default()
                },
                manually_set: vec![ManuallySetItem {
                    entity: DashboardEntityId::Goal(GoalId::new()),
                    title: "Pinned".into(),
                    status: Status::Complete,
                    automatic_status: Status::OnTrack,
                    target_date: None,
                }],
                goal_trend: Vec::new(),
                milestone_trend: Vec::new(),
                timeline: Default::default(),
            }),
            users,
            tasks,
            Arc::new(FixedClock(at)),
        );

        let summary = service.summary().await.expect("summary");

        assert_eq!(summary.as_of, at);
        // All four statuses are present in both rollups, zero when none.
        assert_eq!(summary.goals.on_track.count, 3);
        assert_eq!(summary.goals.on_track.manual, 1);
        assert_eq!(summary.goals.at_risk, StatusCount::default());
        assert_eq!(summary.goals.off_track, StatusCount::default());
        assert_eq!(summary.goals.complete, StatusCount::default());
        assert_eq!(summary.milestones.complete.count, 2);
        assert_eq!(summary.milestones.on_track, StatusCount::default());
        // The totals add up across the four statuses.
        assert_eq!(summary.goals.total(), 3);
        assert_eq!(summary.goals.manual(), 1);
        assert_eq!(summary.milestones.total(), 2);
        assert_eq!(summary.milestones.manual(), 0);
        // The manually set items come through with both statuses.
        let item = &summary.manually_set[0];
        assert_eq!(item.title, "Pinned");
        assert_eq!(item.status, Status::Complete);
        assert_eq!(item.automatic_status, Status::OnTrack);
    }

    #[tokio::test]
    async fn an_empty_read_model_yields_an_all_zero_summary() {
        let (users, tasks) = empty_people_tasks();
        let service = DashboardService::new(
            Arc::new(FakeDashboard {
                goals: StatusRollup::default(),
                milestones: StatusRollup::default(),
                manually_set: Vec::new(),
                goal_trend: Vec::new(),
                milestone_trend: Vec::new(),
                timeline: Default::default(),
            }),
            users,
            tasks,
            Arc::new(FixedClock(
                NaiveDate::from_ymd_opt(2026, 11, 20)
                    .unwrap()
                    .and_hms_opt(9, 30, 0)
                    .unwrap()
                    .and_utc(),
            )),
        );

        let summary = service.summary().await.expect("summary");

        assert_eq!(summary.goals, StatusRollup::default());
        assert_eq!(summary.milestones, StatusRollup::default());
        assert_eq!(summary.goals.total(), 0);
        assert!(summary.manually_set.is_empty());
    }

    /// A trends service over a fixed clock (today 2026-11-20) and the given
    /// canned trend rows.
    fn trends_service(
        goal_trend: Vec<TrendRow>,
        milestone_trend: Vec<TrendRow>,
    ) -> DashboardService {
        let at = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap()
            .and_utc();
        let (users, tasks) = empty_people_tasks();
        DashboardService::new(
            Arc::new(FakeDashboard {
                goals: StatusRollup::default(),
                milestones: StatusRollup::default(),
                manually_set: Vec::new(),
                goal_trend,
                milestone_trend,
                timeline: Default::default(),
            }),
            users,
            tasks,
            Arc::new(FixedClock(at)),
        )
    }

    #[tokio::test]
    async fn the_default_trend_window_is_90_days_ending_today() {
        let service = trends_service(Vec::new(), Vec::new());

        let trends = service.trends(None, None).await.expect("trends");

        assert_eq!(trends.to, NaiveDate::from_ymd_opt(2026, 11, 20).unwrap());
        // `from` is 89 days before `to`: a 90-day inclusive window.
        assert_eq!(trends.from, trends.to - chrono::Duration::days(89));
        assert_eq!((trends.to - trends.from).num_days() + 1, 90);
    }

    #[tokio::test]
    async fn an_explicit_trend_window_is_honoured_and_echoed() {
        let service = trends_service(Vec::new(), Vec::new());
        let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();

        let trends = service.trends(Some(from), Some(to)).await.expect("trends");

        assert_eq!(trends.from, from);
        assert_eq!(trends.to, to);
    }

    #[tokio::test]
    async fn a_trend_window_with_from_after_to_is_rejected() {
        let service = trends_service(Vec::new(), Vec::new());

        let err = service
            .trends(
                Some(NaiveDate::from_ymd_opt(2026, 5, 1).unwrap()),
                Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, TrendError::FromAfterTo { .. }));
    }

    #[tokio::test]
    async fn a_trend_window_longer_than_366_days_is_rejected_but_366_is_not() {
        let service = trends_service(Vec::new(), Vec::new());
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();

        // 367 inclusive days (from = to - 366) is too long.
        let err = service
            .trends(Some(to - chrono::Duration::days(366)), Some(to))
            .await
            .unwrap_err();
        assert!(matches!(err, TrendError::WindowTooLong { days: 367 }));

        // Exactly 366 inclusive days (from = to - 365) is allowed.
        let trends = service
            .trends(Some(to - chrono::Duration::days(365)), Some(to))
            .await
            .expect("trends");
        assert_eq!((trends.to - trends.from).num_days() + 1, 366);
    }

    #[tokio::test]
    async fn trend_points_fill_zero_statuses_total_the_counts_and_omit_goal_percents() {
        let mut row = trend_row(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        row.on_track = 3;
        row.complete = 2;
        row.manual = 1;
        let service = trends_service(vec![row], Vec::new());

        let trends = service.trends(None, None).await.expect("trends");

        let point = &trends.goals[0];
        assert_eq!(point.on_track, 3);
        // The absent statuses are present as zeros, not missing.
        assert_eq!(point.at_risk, 0);
        assert_eq!(point.off_track, 0);
        assert_eq!(point.complete, 2);
        assert_eq!(point.total, 5);
        assert_eq!(point.manual, 1);
        // Goals never carry a percent.
        assert_eq!(point.average_percent_complete, None);
    }

    #[tokio::test]
    async fn the_milestone_average_percent_is_rounded_to_one_decimal_half_away_from_zero() {
        let mut a = trend_row(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        a.on_track = 1;
        a.percent_sum = 100; // 100 / 3 = 33.333... -> 33.3
        a.percent_count = 3;
        let mut b = trend_row(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        b.complete = 1;
        b.percent_sum = 9; // 9 / 4 = 2.25 -> 2.3 (half away from zero)
        b.percent_count = 4;
        let mut c = trend_row(NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        c.at_risk = 1; // no percent recorded -> None
        let service = trends_service(Vec::new(), vec![a, b, c]);

        let trends = service.trends(None, None).await.expect("trends");

        assert_eq!(trends.milestones.len(), 3);
        assert_eq!(trends.milestones[0].average_percent_complete, Some(33.3));
        assert_eq!(trends.milestones[1].average_percent_complete, Some(2.3));
        assert_eq!(trends.milestones[2].average_percent_complete, None);
    }

    // --- Timeline (step 3) -------------------------------------------------

    /// A task repository that answers `task_counts_for_milestones` from a
    /// canned map, counting its calls and remembering the ids asked for; no
    /// other method is reachable from the dashboard service.
    struct CountingTaskRepository {
        counts: HashMap<MilestoneId, TaskCounts>,
        calls: AtomicUsize,
        last_ids: std::sync::Mutex<Vec<MilestoneId>>,
    }

    #[async_trait::async_trait]
    impl TaskRepository for CountingTaskRepository {
        async fn create(&self, _task: Task) -> Result<Task, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn find_by_id(&self, _id: TaskId) -> Result<Option<Task>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn find_by_ids(&self, _ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn task_counts_for_milestones(
            &self,
            milestone_ids: &[MilestoneId],
        ) -> Result<HashMap<MilestoneId, TaskCounts>, RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_ids.lock().unwrap() = milestone_ids.to_vec();
            Ok(self.counts.clone())
        }

        async fn update(&self, _task: Task) -> Result<Task, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn set_status(
            &self,
            _id: TaskId,
            _status: TaskStatus,
        ) -> Result<Task, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn set_assignee(
            &self,
            _id: TaskId,
            _assignee_id: Option<UserId>,
        ) -> Result<Task, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn delete(&self, _id: TaskId) -> Result<(), RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn list_page(
            &self,
            _filter: &crate::ports::TaskListFilter,
            _page: &crate::pagination::PageRequest,
        ) -> Result<crate::pagination::Page<Task>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }
    }

    /// A user repository that answers `summaries_by_ids` from a canned map,
    /// counting its calls and remembering the ids asked for; no other method
    /// is reachable from the dashboard service.
    struct CountingUserRepository {
        summaries: HashMap<UserId, UserSummary>,
        calls: AtomicUsize,
        last_ids: std::sync::Mutex<Vec<UserId>>,
    }

    #[async_trait::async_trait]
    impl UserRepository for CountingUserRepository {
        async fn create(&self, _user: domain::User) -> Result<domain::User, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn create_if_no_users(
            &self,
            _user: domain::User,
        ) -> Result<Option<domain::User>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn find_by_id(&self, _id: UserId) -> Result<Option<domain::User>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn find_by_email(
            &self,
            _email: String,
        ) -> Result<Option<domain::User>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn list(&self) -> Result<Vec<domain::User>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn summaries_by_ids(
            &self,
            ids: &[UserId],
        ) -> Result<Vec<UserSummary>, RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_ids.lock().unwrap() = ids.to_vec();
            Ok(ids
                .iter()
                .filter_map(|id| self.summaries.get(id))
                .cloned()
                .collect())
        }

        async fn list_active_summaries(&self) -> Result<Vec<UserSummary>, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn update(&self, _user: domain::User) -> Result<domain::User, RepositoryError> {
            unimplemented!("not reachable from the dashboard service")
        }

        async fn apply_access_change(
            &self,
            _target: UserId,
            _change: crate::ports::AccessChange,
        ) -> Result<domain::User, crate::ports::AccessChangeError> {
            unimplemented!("not reachable from the dashboard service")
        }
    }

    /// One canned goal row of the timeline read model.
    fn goal_row(
        id: u128,
        automatic: Status,
        status_override: Option<Status>,
        created_at: DateTime<Utc>,
    ) -> TimelineGoal {
        TimelineGoal {
            id: GoalId(Uuid::from_u128(id)),
            title: "goal".into(),
            automatic_status: automatic,
            status_override,
            target_date: None,
            owner_id: None,
            created_at,
        }
    }

    /// One canned milestone row of the timeline read model.
    fn milestone_row(
        id: u128,
        automatic: Status,
        status_override: Option<Status>,
        created_at: DateTime<Utc>,
    ) -> TimelineMilestone {
        TimelineMilestone {
            id: MilestoneId(Uuid::from_u128(id)),
            title: "milestone".into(),
            automatic_status: automatic,
            status_override,
            target_date: None,
            owner_id: None,
            created_at,
        }
    }

    /// A timeline service over canned rows and the counting user/task fakes.
    fn timeline_service(
        data: TimelineData,
        summaries: Vec<UserSummary>,
        counts: Vec<(MilestoneId, TaskCounts)>,
    ) -> (
        DashboardService,
        Arc<CountingUserRepository>,
        Arc<CountingTaskRepository>,
    ) {
        let at = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap()
            .and_utc();
        let users = Arc::new(CountingUserRepository {
            summaries: summaries
                .into_iter()
                .map(|summary| (summary.id, summary))
                .collect(),
            calls: AtomicUsize::new(0),
            last_ids: std::sync::Mutex::new(Vec::new()),
        });
        let tasks = Arc::new(CountingTaskRepository {
            counts: counts.into_iter().collect(),
            calls: AtomicUsize::new(0),
            last_ids: std::sync::Mutex::new(Vec::new()),
        });
        (
            DashboardService::new(
                Arc::new(FakeDashboard {
                    goals: StatusRollup::default(),
                    milestones: StatusRollup::default(),
                    manually_set: Vec::new(),
                    goal_trend: Vec::new(),
                    milestone_trend: Vec::new(),
                    timeline: data,
                }),
                users.clone(),
                tasks.clone(),
                Arc::new(FixedClock(at)),
            ),
            users,
            tasks,
        )
    }

    #[tokio::test]
    async fn the_timeline_reports_effective_and_automatic_status_and_utc_start_dates() {
        // One row created a second before UTC midnight, one a second after:
        // the start date is the UTC calendar date of creation (D19).
        let before_midnight = NaiveDate::from_ymd_opt(2026, 11, 19)
            .unwrap()
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc();
        let after_midnight = NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(0, 0, 1)
            .unwrap()
            .and_utc();
        let (service, _, _) = timeline_service(
            TimelineData {
                goals: vec![goal_row(
                    1,
                    Status::OnTrack,
                    Some(Status::Complete),
                    before_midnight,
                )],
                milestones: vec![milestone_row(2, Status::AtRisk, None, after_midnight)],
                links: Vec::new(),
            },
            Vec::new(),
            Vec::new(),
        );

        let timeline = service
            .timeline(TimelineFilter::default())
            .await
            .expect("timeline");

        // The override wins for the effective status (D12); the automatic
        // value is reported alongside.
        let goal = &timeline.goals[0];
        assert_eq!(goal.status, Status::Complete);
        assert_eq!(goal.status_source, StatusSource::ManualOverride);
        assert_eq!(goal.automatic_status, Status::OnTrack);
        assert_eq!(
            goal.start_date,
            NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
        );

        // No override: the stored value is the effective one.
        let milestone = &timeline.milestones[0];
        assert_eq!(milestone.status, Status::AtRisk);
        assert_eq!(milestone.status_source, StatusSource::Computed);
        assert_eq!(milestone.automatic_status, Status::AtRisk);
        assert_eq!(
            milestone.start_date,
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn the_timeline_percent_complete_covers_zero_partial_and_done_milestones() {
        let (service, _, _) = timeline_service(
            TimelineData {
                goals: Vec::new(),
                milestones: vec![
                    milestone_row(1, Status::OnTrack, None, Utc::now()),
                    milestone_row(2, Status::OnTrack, None, Utc::now()),
                    milestone_row(3, Status::Complete, None, Utc::now()),
                ],
                links: Vec::new(),
            },
            Vec::new(),
            vec![
                (
                    MilestoneId(Uuid::from_u128(1)),
                    TaskCounts { total: 0, done: 0 },
                ),
                (
                    MilestoneId(Uuid::from_u128(2)),
                    TaskCounts { total: 3, done: 1 },
                ),
                (
                    MilestoneId(Uuid::from_u128(3)),
                    TaskCounts { total: 2, done: 2 },
                ),
            ],
        );

        let timeline = service
            .timeline(TimelineFilter::default())
            .await
            .expect("timeline");

        // No tasks is 0, one of three rounds to 33, all done is 100 — the
        // same values the standard milestone responses report.
        let percents: Vec<u8> = timeline
            .milestones
            .iter()
            .map(|milestone| milestone.percent_complete)
            .collect();
        assert_eq!(percents, vec![0, 33, 100]);
    }

    #[tokio::test]
    async fn the_timeline_batches_owners_and_counts_into_one_call_each() {
        // Two goals and three milestones; two of them share one owner, one
        // has a second, one is unowned. Milestone 3 links to both goals.
        let owner_a = UserId(Uuid::from_u128(90));
        let owner_b = UserId(Uuid::from_u128(91));
        let now = Utc::now();
        let mut goal_1 = goal_row(1, Status::OnTrack, None, now);
        goal_1.owner_id = Some(owner_a);
        let mut goal_2 = goal_row(2, Status::OnTrack, None, now);
        goal_2.owner_id = Some(owner_b);
        let mut milestone_1 = milestone_row(10, Status::OnTrack, None, now);
        milestone_1.owner_id = Some(owner_a);
        let mut milestone_2 = milestone_row(11, Status::OnTrack, None, now);
        milestone_2.owner_id = Some(owner_b);
        let (service, users, tasks) = timeline_service(
            TimelineData {
                goals: vec![goal_1, goal_2],
                milestones: vec![
                    milestone_1,
                    milestone_2,
                    milestone_row(12, Status::OnTrack, None, now),
                ],
                links: vec![
                    (GoalId(Uuid::from_u128(1)), MilestoneId(Uuid::from_u128(10))),
                    (GoalId(Uuid::from_u128(1)), MilestoneId(Uuid::from_u128(12))),
                    (GoalId(Uuid::from_u128(2)), MilestoneId(Uuid::from_u128(12))),
                ],
            },
            vec![
                UserSummary::new(owner_a, "Owner A", "a@example.com", true),
                UserSummary::new(owner_b, "Owner B", "b@example.com", false),
            ],
            vec![
                (
                    MilestoneId(Uuid::from_u128(10)),
                    TaskCounts { total: 1, done: 1 },
                ),
                (
                    MilestoneId(Uuid::from_u128(11)),
                    TaskCounts { total: 0, done: 0 },
                ),
                (
                    MilestoneId(Uuid::from_u128(12)),
                    TaskCounts { total: 4, done: 2 },
                ),
            ],
        );

        let timeline = service
            .timeline(TimelineFilter::default())
            .await
            .expect("timeline");

        // One grouped counts call and one summaries call for the whole
        // response, whatever the row count.
        assert_eq!(tasks.calls.load(Ordering::SeqCst), 1);
        assert_eq!(users.calls.load(Ordering::SeqCst), 1);
        // The owner ids were deduplicated before the lookup...
        let asked = users.last_ids.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(asked.contains(&owner_a) && asked.contains(&owner_b));
        drop(asked);

        // ...and the summaries land on every row that names the owner — a
        // deactivated one flagged, the unowned row null.
        let goal_1 = &timeline.goals[0];
        assert_eq!(goal_1.owner.as_ref().map(|owner| owner.id), Some(owner_a));
        assert_eq!(goal_1.owner.as_ref().map(|owner| owner.active), Some(true));
        let goal_2 = &timeline.goals[1];
        assert_eq!(goal_2.owner.as_ref().map(|owner| owner.active), Some(false));
        assert_eq!(
            timeline.milestones[0].owner.as_ref().map(|owner| owner.id),
            Some(owner_a)
        );
        assert_eq!(timeline.milestones[2].owner, None);

        // A milestone linked to two goals appears in both goals' ids; each
        // goal's list keeps the read model's order.
        assert_eq!(
            goal_1.milestone_ids,
            vec![
                MilestoneId(Uuid::from_u128(10)),
                MilestoneId(Uuid::from_u128(12))
            ]
        );
        assert_eq!(goal_2.milestone_ids, vec![MilestoneId(Uuid::from_u128(12))]);
    }
}
