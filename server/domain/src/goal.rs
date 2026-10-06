//! Goals: the outcomes a school project is organized around.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::milestone::Milestone;
use crate::status::{Status, StatusSource};

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
    /// The automatic status, as worked out from the goal's milestones. A
    /// manual override, when set, takes its place for display and rollups.
    pub status: Status,
    /// A manual override of the automatic status. It is sticky: held until
    /// explicitly cleared, never touched by recomputation.
    pub status_override: Option<Status>,
    /// The date this goal is expected to be finished by, if one was set.
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Goal {
    /// The status to show and roll up: the manual override when one is set,
    /// otherwise the automatic status.
    pub fn effective_status(&self) -> Status {
        self.status_override.unwrap_or(self.status)
    }

    /// Where [`Goal::effective_status`] came from: a manual override when
    /// one is set, the system's computation otherwise.
    pub fn status_source(&self) -> StatusSource {
        match self.status_override {
            Some(_) => StatusSource::ManualOverride,
            None => StatusSource::Computed,
        }
    }

    /// Set a manual override. It holds until [`Goal::clear_status_override`];
    /// recomputation never removes it.
    pub fn set_status_override(&mut self, status: Status) {
        self.status_override = Some(status);
    }

    /// Clear the manual override, returning to the automatic status.
    pub fn clear_status_override(&mut self) {
        self.status_override = None;
    }

    /// Record a freshly computed automatic status. Never touches the
    /// override, which is why an override survives recomputation.
    pub fn apply_computed_status(&mut self, status: Status) {
        self.status = status;
    }

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
    /// The rule looks only at each milestone's effective status (including
    /// any manual override), not at dates: a milestone that has passed its
    /// target date without being finished is expected to already carry an
    /// `OffTrack` status of its own. Keeping the rollup free of "today" also
    /// keeps it a pure function, which makes it easy to test and safe to run
    /// anywhere. The exact rule may be refined as the product matures; that
    /// is why it lives in one clearly named place.
    pub fn compute_status_from_milestones(&self, milestones: &[Milestone]) -> Status {
        if milestones.is_empty() {
            return Status::OnTrack;
        }
        if milestones
            .iter()
            .all(|m| m.effective_status() == Status::Complete)
        {
            return Status::Complete;
        }
        if milestones
            .iter()
            .any(|m| m.effective_status() == Status::OffTrack)
        {
            return Status::OffTrack;
        }
        if milestones
            .iter()
            .any(|m| m.effective_status() == Status::AtRisk)
        {
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
            status,
            status_override: None,
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
            status: Status::OnTrack,
            status_override: None,
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

    #[test]
    fn rollup_uses_the_milestones_effective_status() {
        let goal = test_goal();
        let mut overridden = milestone_with_status(Status::OnTrack);
        overridden.set_status_override(Status::OffTrack);
        let milestones = [overridden];
        assert_eq!(
            goal.compute_status_from_milestones(&milestones),
            Status::OffTrack
        );
    }

    #[test]
    fn effective_status_without_an_override_is_the_stored_status() {
        let goal = test_goal();
        assert_eq!(goal.effective_status(), Status::OnTrack);
        assert_eq!(goal.status_source(), StatusSource::Computed);
    }

    #[test]
    fn setting_an_override_flips_the_effective_status_and_source() {
        let mut goal = test_goal();
        goal.set_status_override(Status::AtRisk);
        assert_eq!(goal.effective_status(), Status::AtRisk);
        assert_eq!(goal.status_source(), StatusSource::ManualOverride);
        // The automatic status is untouched by the override.
        assert_eq!(goal.status, Status::OnTrack);
    }

    #[test]
    fn apply_computed_status_leaves_an_active_override_untouched() {
        let mut goal = test_goal();
        goal.set_status_override(Status::AtRisk);
        goal.apply_computed_status(Status::OffTrack);
        assert_eq!(goal.status, Status::OffTrack);
        assert_eq!(goal.status_override, Some(Status::AtRisk));
        assert_eq!(goal.effective_status(), Status::AtRisk);
        assert_eq!(goal.status_source(), StatusSource::ManualOverride);
    }

    #[test]
    fn clearing_an_override_returns_to_the_computed_status() {
        let mut goal = test_goal();
        goal.set_status_override(Status::Complete);
        goal.apply_computed_status(Status::AtRisk);
        goal.clear_status_override();
        assert_eq!(goal.status_override, None);
        assert_eq!(goal.effective_status(), Status::AtRisk);
        assert_eq!(goal.status_source(), StatusSource::Computed);
    }
}
