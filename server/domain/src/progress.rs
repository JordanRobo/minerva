//! Point-in-time records of how far along a goal or milestone is, and the
//! rules that turn such progress into an automatic status.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::goal::{Goal, GoalId};
use crate::milestone::{Milestone, MilestoneId};
use crate::status::{Status, StatusSource};

/// Identifier for a [`ProgressSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ProgressSnapshotId(pub Uuid);

impl ProgressSnapshotId {
    /// Create a fresh identifier for a snapshot that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProgressSnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

/// The thing a progress snapshot was recorded for: either a goal or a
/// milestone, never both and never neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ProgressTarget {
    Goal(GoalId),
    Milestone(MilestoneId),
}

/// An error from trying to build a [`ProgressSnapshot`] with an invalid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressError {
    /// `percent_complete` was above 100.
    PercentOutOfRange(u8),
}

impl std::fmt::Display for ProgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressError::PercentOutOfRange(value) => {
                write!(f, "percent_complete must be between 0 and 100, got {value}")
            }
        }
    }
}

impl std::error::Error for ProgressError {}

/// Why a snapshot was taken (D5): the daily scheduled sweep, or staff
/// manually changing the entity's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// The daily scheduled sweep recorded it.
    Daily,
    /// Staff set or cleared a manual status override.
    ManualChange,
}

/// A snapshot not yet stored: everything a [`ProgressSnapshot`] records,
/// with its id already minted. The `snapshot_date` is derived from
/// `taken_at` here so the two can never disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshot {
    pub id: ProgressSnapshotId,
    pub target: ProgressTarget,
    /// The effective status to record: the manual override when set,
    /// otherwise the automatic status.
    pub status: Status,
    /// Where `status` came from.
    pub source: StatusSource,
    /// When the snapshot was taken.
    pub taken_at: DateTime<Utc>,
    /// The UTC calendar day of `taken_at`. At most one daily snapshot exists
    /// per entity and day on this column.
    pub snapshot_date: NaiveDate,
    /// Why it was taken.
    pub reason: SnapshotReason,
    /// Completion percentage at the moment, 0..=100 — milestones only; goals
    /// have none. Kept private so every snapshot goes through
    /// [`NewSnapshot::new`], the only place the range is enforced.
    percent_complete: Option<u8>,
}

impl NewSnapshot {
    /// Record a new snapshot for `target`.
    ///
    /// `percent_complete` is the 0-100 value the API reports for a
    /// milestone; goals pass `None`. A percentage above 100 is rejected with
    /// [`ProgressError::PercentOutOfRange`] rather than clamped to 100: it
    /// almost always signals a bug upstream, and silently "fixing" it would
    /// hide that bug. (A `u8` cannot be negative, so the lower bound needs
    /// no checking.)
    pub fn new(
        target: ProgressTarget,
        status: Status,
        source: StatusSource,
        taken_at: DateTime<Utc>,
        reason: SnapshotReason,
        percent_complete: Option<u8>,
    ) -> Result<Self, ProgressError> {
        if let Some(percent) = percent_complete
            && percent > 100
        {
            return Err(ProgressError::PercentOutOfRange(percent));
        }
        Ok(Self {
            id: ProgressSnapshotId::new(),
            target,
            status,
            source,
            taken_at,
            snapshot_date: taken_at.date_naive(),
            reason,
            percent_complete,
        })
    }

    /// Snapshot a goal's current effective status. Goals have no completion
    /// percentage.
    pub fn from_goal(goal: &Goal, taken_at: DateTime<Utc>, reason: SnapshotReason) -> Self {
        Self::new(
            ProgressTarget::Goal(goal.id),
            goal.effective_status(),
            goal.status_source(),
            taken_at,
            reason,
            None,
        )
        .expect("a goal snapshot carries no percentage to validate")
    }

    /// Snapshot a milestone's current effective status. `percent_complete` is
    /// the same 0-100 value the API reports for the milestone.
    pub fn from_milestone(
        milestone: &Milestone,
        taken_at: DateTime<Utc>,
        reason: SnapshotReason,
        percent_complete: u8,
    ) -> Result<Self, ProgressError> {
        Self::new(
            ProgressTarget::Milestone(milestone.id),
            milestone.effective_status(),
            milestone.status_source(),
            taken_at,
            reason,
            Some(percent_complete),
        )
    }

    /// The completion percentage to record: milestones only; `None` for
    /// goals. Always 0..=100 when present.
    pub fn percent_complete(&self) -> Option<u8> {
        self.percent_complete
    }
}

/// A stored point-in-time record of how far along a goal or milestone was.
///
/// Snapshots are what let the platform answer "how has progress changed over
/// time?" without re-deriving history from other data. The recorded status
/// is the *effective* one — the manual override when set, else the automatic
/// value — with `source` saying which of the two it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgressSnapshot {
    pub id: ProgressSnapshotId,
    pub target: ProgressTarget,
    /// The effective status at the moment: override when set, else automatic.
    pub status: Status,
    /// Where [`ProgressSnapshot::status`] came from.
    pub source: StatusSource,
    /// When the snapshot was taken.
    pub taken_at: DateTime<Utc>,
    /// The UTC calendar day of `taken_at`.
    pub snapshot_date: NaiveDate,
    /// Why it was taken.
    pub reason: SnapshotReason,
    /// Completion percentage at the moment, 0..=100 — milestones only; goals
    /// have none (`None`).
    percent_complete: Option<u8>,
}

impl ProgressSnapshot {
    /// The stored form of a [`NewSnapshot`].
    pub fn from_new(new: NewSnapshot) -> Self {
        Self {
            id: new.id,
            target: new.target,
            status: new.status,
            source: new.source,
            taken_at: new.taken_at,
            snapshot_date: new.snapshot_date,
            reason: new.reason,
            percent_complete: new.percent_complete,
        }
    }

    /// The completion percentage at the moment, 0..=100; `None` for goals.
    pub fn percent_complete(&self) -> Option<u8> {
        self.percent_complete
    }
}

/// How far behind schedule (as a fraction of the plan) a milestone may be
/// before its automatic status leaves `OnTrack`, and before it becomes
/// `OffTrack` outright. Tunable knobs for [`compute_milestone_status`] (D19).
pub const SCHEDULE_GAP_AT_RISK: f64 = 0.20;
pub const SCHEDULE_GAP_OFF_TRACK: f64 = 0.40;

/// The inputs to a milestone status computation: how many tasks the
/// milestone has, how many are done, and the window the work was planned
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilestoneProgress {
    pub total_tasks: u32,
    pub done_tasks: u32,
    /// When the milestone's work started — its creation date in v1 (D19), as
    /// milestones carry no dedicated start-date field.
    pub start_date: NaiveDate,
    pub target_date: Option<NaiveDate>,
}

/// The fraction of `total_tasks` that are done, in 0..=1. A milestone with
/// no tasks is 0.0 complete, not an error.
pub fn percent_complete(total_tasks: u32, done_tasks: u32) -> f64 {
    if total_tasks == 0 {
        0.0
    } else {
        done_tasks as f64 / total_tasks as f64
    }
}

/// Work out a milestone's automatic status from its progress and today's
/// date (D19). Pure: the same inputs always give the same status, which is
/// what makes it safe to run in the daily sweep or on demand.
///
/// The rules, in order:
///
/// 1. `Complete` when every task is done (and there is at least one).
/// 2. `OnTrack` when no target date was set — nothing can be late against.
/// 3. `OffTrack` when the target date has passed without completion.
/// 4. Otherwise compare the work done with what the schedule expected by
///    today: `expected` is the fraction of the start-to-target window that
///    has elapsed, clamped to 0..=1 (a target no later than the start counts
///    as fully expected). A gap below [`SCHEDULE_GAP_AT_RISK`] stays
///    `OnTrack`, a gap below [`SCHEDULE_GAP_OFF_TRACK`] is `AtRisk`, and a
///    gap at or beyond it is `OffTrack`.
pub fn compute_milestone_status(progress: &MilestoneProgress, today: NaiveDate) -> Status {
    if progress.total_tasks > 0 && progress.done_tasks == progress.total_tasks {
        return Status::Complete;
    }
    let Some(target_date) = progress.target_date else {
        return Status::OnTrack;
    };
    if target_date < today {
        return Status::OffTrack;
    }
    let expected = if target_date <= progress.start_date {
        1.0
    } else {
        let elapsed = (today - progress.start_date).num_days() as f64;
        let window = (target_date - progress.start_date).num_days() as f64;
        (elapsed / window).clamp(0.0, 1.0)
    };
    let gap = expected - percent_complete(progress.total_tasks, progress.done_tasks);
    if gap < SCHEDULE_GAP_AT_RISK {
        Status::OnTrack
    } else if gap < SCHEDULE_GAP_OFF_TRACK {
        Status::AtRisk
    } else {
        Status::OffTrack
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn test_timestamp() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    fn test_goal() -> Goal {
        let now = test_timestamp();
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_milestone() -> Milestone {
        let now = test_timestamp();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn snapshot_reason_strings_round_trip() {
        let cases = [
            (SnapshotReason::Daily, "daily"),
            (SnapshotReason::ManualChange, "manual_change"),
        ];
        for (reason, wire) in cases {
            let json = serde_json::to_string(&reason).expect("reason serializes");
            assert_eq!(json, format!("\"{wire}\""));
            let back: SnapshotReason = serde_json::from_str(&json).expect("reason deserializes");
            assert_eq!(back, reason);
        }
    }

    #[test]
    fn snapshot_from_a_goal_records_its_effective_status_and_source() {
        let goal = test_goal();
        let taken_at = test_timestamp();
        let snapshot = NewSnapshot::from_goal(&goal, taken_at, SnapshotReason::Daily);
        assert_eq!(snapshot.target, ProgressTarget::Goal(goal.id));
        assert_eq!(snapshot.status, Status::OnTrack);
        assert_eq!(snapshot.source, StatusSource::Computed);
        assert_eq!(snapshot.reason, SnapshotReason::Daily);
        assert_eq!(snapshot.taken_at, taken_at);
        assert_eq!(snapshot.snapshot_date, taken_at.date_naive());
        assert_eq!(snapshot.percent_complete(), None);
    }

    #[test]
    fn snapshot_from_an_overridden_goal_records_the_override() {
        let mut goal = test_goal();
        goal.set_status_override(Status::OffTrack);
        let snapshot =
            NewSnapshot::from_goal(&goal, test_timestamp(), SnapshotReason::ManualChange);
        assert_eq!(snapshot.status, Status::OffTrack);
        assert_eq!(snapshot.source, StatusSource::ManualOverride);
    }

    #[test]
    fn snapshot_from_a_milestone_records_its_effective_status_source_and_percent() {
        let milestone = test_milestone();
        let taken_at = test_timestamp();
        let snapshot = NewSnapshot::from_milestone(&milestone, taken_at, SnapshotReason::Daily, 57)
            .expect("57 is a valid percentage");
        assert_eq!(snapshot.target, ProgressTarget::Milestone(milestone.id));
        assert_eq!(snapshot.status, Status::OnTrack);
        assert_eq!(snapshot.source, StatusSource::Computed);
        assert_eq!(snapshot.percent_complete(), Some(57));
    }

    #[test]
    fn snapshot_from_an_overridden_milestone_records_the_override() {
        let mut milestone = test_milestone();
        milestone.set_status_override(Status::AtRisk);
        let snapshot = NewSnapshot::from_milestone(
            &milestone,
            test_timestamp(),
            SnapshotReason::ManualChange,
            0,
        )
        .expect("0 is a valid percentage");
        assert_eq!(snapshot.status, Status::AtRisk);
        assert_eq!(snapshot.source, StatusSource::ManualOverride);
    }

    #[test]
    fn snapshot_rejects_percentages_above_hundred() {
        let milestone = test_milestone();
        let result =
            NewSnapshot::from_milestone(&milestone, test_timestamp(), SnapshotReason::Daily, 101);
        assert_eq!(result.err(), Some(ProgressError::PercentOutOfRange(101)));
    }

    #[test]
    fn snapshot_rejects_the_largest_u8() {
        let milestone = test_milestone();
        let result = NewSnapshot::from_milestone(
            &milestone,
            test_timestamp(),
            SnapshotReason::Daily,
            u8::MAX,
        );
        assert_eq!(
            result.err(),
            Some(ProgressError::PercentOutOfRange(u8::MAX))
        );
    }

    #[test]
    fn snapshot_accepts_hundred_percent() {
        let milestone = test_milestone();
        let snapshot =
            NewSnapshot::from_milestone(&milestone, test_timestamp(), SnapshotReason::Daily, 100)
                .expect("100 is a valid percentage");
        assert_eq!(snapshot.percent_complete(), Some(100));
    }

    #[test]
    fn stored_snapshot_keeps_every_field_of_the_new_one() {
        let goal = test_goal();
        let new = NewSnapshot::from_goal(&goal, test_timestamp(), SnapshotReason::Daily);
        let stored = ProgressSnapshot::from_new(new.clone());
        assert_eq!(stored.id, new.id);
        assert_eq!(stored.target, new.target);
        assert_eq!(stored.status, new.status);
        assert_eq!(stored.source, new.source);
        assert_eq!(stored.taken_at, new.taken_at);
        assert_eq!(stored.snapshot_date, new.snapshot_date);
        assert_eq!(stored.reason, new.reason);
        assert_eq!(stored.percent_complete(), new.percent_complete());
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn progress(
        total_tasks: u32,
        done_tasks: u32,
        start_date: NaiveDate,
        target_date: Option<NaiveDate>,
    ) -> MilestoneProgress {
        MilestoneProgress {
            total_tasks,
            done_tasks,
            start_date,
            target_date,
        }
    }

    #[test]
    fn percent_complete_divides_done_by_total() {
        assert_eq!(percent_complete(4, 0), 0.0);
        assert_eq!(percent_complete(4, 2), 0.5);
        assert_eq!(percent_complete(4, 4), 1.0);
        assert_eq!(percent_complete(10, 3), 0.3);
    }

    #[test]
    fn percent_complete_with_no_tasks_is_zero() {
        assert_eq!(percent_complete(0, 0), 0.0);
    }

    #[test]
    fn milestone_status_follows_the_d19_rules() {
        let cases = [
            (
                "every task done is complete, even past the target date",
                progress(5, 5, day(2026, 1, 1), Some(day(2026, 1, 10))),
                day(2026, 1, 15),
                Status::Complete,
            ),
            (
                "no tasks and no target date is on track",
                progress(0, 0, day(2026, 1, 1), None),
                day(2026, 1, 15),
                Status::OnTrack,
            ),
            (
                "no target date means nothing can be late",
                progress(4, 1, day(2026, 1, 1), None),
                day(2026, 1, 15),
                Status::OnTrack,
            ),
            (
                "a passed target date with work left is off track",
                progress(4, 1, day(2026, 1, 1), Some(day(2026, 1, 10))),
                day(2026, 1, 15),
                Status::OffTrack,
            ),
            (
                "zero tasks after the target date is off track",
                progress(0, 0, day(2026, 1, 1), Some(day(2026, 1, 10))),
                day(2026, 1, 15),
                Status::OffTrack,
            ),
            (
                "zero tasks early in the window is on track",
                progress(0, 0, day(2026, 1, 1), Some(day(2026, 1, 31))),
                day(2026, 1, 5),
                Status::OnTrack,
            ),
            (
                "zero tasks a third into the window is at risk",
                progress(0, 0, day(2026, 1, 1), Some(day(2026, 1, 31))),
                day(2026, 1, 12),
                Status::AtRisk,
            ),
            (
                "zero tasks halfway through the window is off track",
                progress(0, 0, day(2026, 1, 1), Some(day(2026, 1, 31))),
                day(2026, 1, 16),
                Status::OffTrack,
            ),
            (
                "the target date itself with no work done is off track",
                progress(4, 0, day(2026, 1, 1), Some(day(2026, 1, 15))),
                day(2026, 1, 15),
                Status::OffTrack,
            ),
            (
                "on schedule the day before the target stays on track",
                progress(15, 14, day(2026, 1, 1), Some(day(2026, 1, 16))),
                day(2026, 1, 15),
                Status::OnTrack,
            ),
            (
                "behind schedule the day before the target is at risk",
                progress(15, 10, day(2026, 1, 1), Some(day(2026, 1, 16))),
                day(2026, 1, 15),
                Status::AtRisk,
            ),
            (
                "ahead of schedule is on track",
                progress(14, 8, day(2026, 1, 1), Some(day(2026, 1, 31))),
                day(2026, 1, 15),
                Status::OnTrack,
            ),
            (
                "a gap exactly at the at-risk threshold is at risk",
                progress(10, 3, day(2026, 1, 1), Some(day(2026, 1, 11))),
                day(2026, 1, 6),
                Status::AtRisk,
            ),
            (
                "a gap just under the at-risk threshold stays on track",
                progress(10, 4, day(2026, 1, 1), Some(day(2026, 1, 11))),
                day(2026, 1, 6),
                Status::OnTrack,
            ),
            (
                "a gap exactly at the off-track threshold is off track",
                progress(5, 3, day(2026, 1, 1), Some(day(2026, 1, 11))),
                day(2026, 1, 11),
                Status::OffTrack,
            ),
            (
                "a gap beyond the off-track threshold is off track",
                progress(10, 2, day(2026, 1, 1), Some(day(2026, 1, 11))),
                day(2026, 1, 8),
                Status::OffTrack,
            ),
            (
                "a target before the start counts as fully expected",
                progress(10, 7, day(2026, 1, 10), Some(day(2026, 1, 5))),
                day(2026, 1, 3),
                Status::AtRisk,
            ),
            (
                "a target before the start and already passed is off track by date",
                progress(10, 7, day(2026, 1, 10), Some(day(2026, 1, 5))),
                day(2026, 1, 6),
                Status::OffTrack,
            ),
            (
                "work starting in the future clamps the expected progress to zero",
                progress(4, 1, day(2026, 1, 20), Some(day(2026, 1, 30))),
                day(2026, 1, 15),
                Status::OnTrack,
            ),
        ];
        for (name, p, today, expected) in cases {
            assert_eq!(compute_milestone_status(&p, today), expected, "{name}");
        }
    }
}
