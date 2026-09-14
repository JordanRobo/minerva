//! Point-in-time records of how far along a goal or milestone is.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::goal::GoalId;
use crate::milestone::MilestoneId;
use crate::status::Status;

/// Identifier for a [`ProgressSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}
