//! Goals: the outcomes a school project is organized around.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::milestone::Milestone;
use crate::status::{GoalStatus, Status};

/// Identifier for a [`Goal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct GoalId(pub Uuid);

impl GoalId {
    /// Create a fresh identifier for a goal that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GoalId {
    fn default() -> Self {
        Self::new()
    }
}

/// An outcome the project is organized around, broken into milestones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Goal {
    pub id: GoalId,
    pub title: String,
    pub description: Option<String>,
    pub status: GoalStatus,
    /// The date this goal is expected to be finished by, if one was set.
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Goal {
    /// Work out this goal's overall status from the statuses of its
    /// associated milestones.
    ///
    /// The rule, in priority order:
    ///
    /// 1. `Complete` if every milestone is complete (and there is at least
    ///    one).
    /// 2. `OffTrack` if any milestone is off track.
    /// 3. `AtRisk` if any milestone is at risk.
    /// 4. Otherwise `OnTrack`.
    ///
    /// A goal with no milestones yet is reported as `OnTrack`: there is
    /// simply nothing behind schedule to speak of.
    ///
    /// The rule looks only at each milestone's status, not at dates: a
    /// milestone that has passed its target date without being finished is
    /// expected to already carry an `OffTrack` status of its own. Keeping
    /// the rollup free of "today" also keeps it a pure function, which makes
    /// it easy to test and safe to run anywhere. The exact rule may be
    /// refined as the product matures; that is why it lives in one clearly
    /// named place.
    pub fn compute_status_from_milestones(&self, milestones: &[Milestone]) -> Status {
        if milestones.is_empty() {
            return Status::OnTrack;
        }
        if milestones
            .iter()
            .all(|m| m.status.status == Status::Complete)
        {
            return Status::Complete;
        }
        if milestones
            .iter()
            .any(|m| m.status.status == Status::OffTrack)
        {
            return Status::OffTrack;
        }
        if milestones.iter().any(|m| m.status.status == Status::AtRisk) {
            return Status::AtRisk;
        }
        Status::OnTrack
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::milestone::MilestoneId;

    fn test_timestamp() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    fn milestone_with_status(status: Status) -> Milestone {
        let now = test_timestamp();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: GoalStatus::computed(status),
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_goal() -> Goal {
        let now = test_timestamp();
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status: GoalStatus::computed(Status::OnTrack),
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn rollup_with_no_milestones_is_on_track() {
        let goal = test_goal();
        assert_eq!(goal.compute_status_from_milestones(&[]), Status::OnTrack);
    }

    #[test]
    fn rollup_of_all_on_track_milestones_is_on_track() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::OnTrack),
            milestone_with_status(Status::OnTrack),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::OnTrack
        );
    }

    #[test]
    fn rollup_with_one_off_track_milestone_is_off_track() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::OnTrack),
            milestone_with_status(Status::OffTrack),
            milestone_with_status(Status::OnTrack),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::OffTrack
        );
    }

    #[test]
    fn rollup_with_one_at_risk_milestone_and_no_off_track_is_at_risk() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::OnTrack),
            milestone_with_status(Status::AtRisk),
            milestone_with_status(Status::Complete),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::AtRisk
        );
    }

    #[test]
    fn rollup_of_all_complete_milestones_is_complete() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::Complete),
            milestone_with_status(Status::Complete),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::Complete
        );
    }

    #[test]
    fn off_track_wins_over_at_risk() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::AtRisk),
            milestone_with_status(Status::OffTrack),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::OffTrack
        );
    }

    #[test]
    fn complete_milestones_do_not_count_as_done_until_all_are_complete() {
        let goal = test_goal();
        let milestones = [
            milestone_with_status(Status::Complete),
            milestone_with_status(Status::OnTrack),
        ];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::OnTrack
        );
    }
}
