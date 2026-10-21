//! Point-in-time records of how far along a goal or milestone is, and the
//! rules that turn such progress into an automatic status.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::goal::GoalId;
use crate::milestone::MilestoneId;
use crate::status::Status;

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

/// A point-in-time record of how far along a goal or milestone was.
///
/// Snapshots are what let the platform answer "how has progress changed over
/// time?" without re-deriving history from other data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgressSnapshot {
    pub id: ProgressSnapshotId,
    pub target: ProgressTarget,
    /// When the snapshot was taken.
    pub recorded_at: DateTime<Utc>,
    /// The status computed for the target at the time of recording.
    pub status: Status,
    /// Completion percentage, always in the range 0..=100. Kept private so
    /// that every snapshot goes through [`ProgressSnapshot::new`], which is
    /// the only place the range is enforced.
    percent_complete: u8,
    pub note: Option<String>,
}

impl ProgressSnapshot {
    /// Record a new snapshot for `target`.
    ///
    /// `percent_complete` must be at most 100. Values above that are
    /// rejected with [`ProgressError::PercentOutOfRange`] rather than
    /// clamped to 100: a percentage over 100 almost always signals a bug
    /// upstream, and silently "fixing" it would hide that bug. (A `u8`
    /// cannot be negative, so the lower bound needs no checking.)
    pub fn new(
        target: ProgressTarget,
        recorded_at: DateTime<Utc>,
        status: Status,
        percent_complete: u8,
        note: Option<String>,
    ) -> Result<Self, ProgressError> {
        if percent_complete > 100 {
            return Err(ProgressError::PercentOutOfRange(percent_complete));
        }
        Ok(Self {
            id: ProgressSnapshotId::new(),
            target,
            recorded_at,
            status,
            percent_complete,
            note,
        })
    }

    /// The completion percentage at the time of recording. Always 0..=100.
    pub fn percent_complete(&self) -> u8 {
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

    #[test]
    fn snapshot_accepts_zero_percent() {
        let snapshot = ProgressSnapshot::new(
            ProgressTarget::Goal(GoalId::new()),
            test_timestamp(),
            Status::OnTrack,
            0,
            None,
        )
        .expect("0 is a valid percentage");
        assert_eq!(snapshot.percent_complete(), 0);
    }

    #[test]
    fn snapshot_accepts_hundred_percent() {
        let snapshot = ProgressSnapshot::new(
            ProgressTarget::Milestone(MilestoneId::new()),
            test_timestamp(),
            Status::Complete,
            100,
            Some("finished".to_owned()),
        )
        .expect("100 is a valid percentage");
        assert_eq!(snapshot.percent_complete(), 100);
        assert_eq!(snapshot.note.as_deref(), Some("finished"));
    }

    #[test]
    fn snapshot_accepts_mid_range_percentages() {
        let snapshot = ProgressSnapshot::new(
            ProgressTarget::Goal(GoalId::new()),
            test_timestamp(),
            Status::AtRisk,
            57,
            None,
        )
        .expect("57 is a valid percentage");
        assert_eq!(snapshot.percent_complete(), 57);
        assert_eq!(snapshot.status, Status::AtRisk);
    }

    #[test]
    fn snapshot_rejects_percentages_above_hundred() {
        let result = ProgressSnapshot::new(
            ProgressTarget::Goal(GoalId::new()),
            test_timestamp(),
            Status::OnTrack,
            101,
            None,
        );
        assert_eq!(result.err(), Some(ProgressError::PercentOutOfRange(101)));
    }

    #[test]
    fn snapshot_rejects_the_largest_u8() {
        let result = ProgressSnapshot::new(
            ProgressTarget::Milestone(MilestoneId::new()),
            test_timestamp(),
            Status::OnTrack,
            u8::MAX,
            None,
        );
        assert_eq!(
            result.err(),
            Some(ProgressError::PercentOutOfRange(u8::MAX))
        );
    }

    #[test]
    fn snapshot_keeps_its_target_and_recording_time() {
        let goal_id = GoalId::new();
        let recorded_at = test_timestamp();
        let snapshot = ProgressSnapshot::new(
            ProgressTarget::Goal(goal_id),
            recorded_at,
            Status::OnTrack,
            42,
            None,
        )
        .expect("42 is a valid percentage");
        assert_eq!(snapshot.target, ProgressTarget::Goal(goal_id));
        assert_eq!(snapshot.recorded_at, recorded_at);
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
