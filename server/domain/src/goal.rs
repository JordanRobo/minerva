//! Goals: the outcomes a school project is organized around.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::milestone::Milestone;
use crate::status::{Status, StatusSource};
use crate::user::UserId;

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
    /// The user responsible for this goal, if one has been chosen (D17). A
    /// goal may exist before anyone owns it; the domain does not check that
    /// the user exists — that is an application concern.
    pub owner_id: Option<UserId>,
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
    /// associated milestones (D19).
    ///
    /// The rule, in priority order:
    ///
    /// 1. `OnTrack` if there are no milestones at all: there is simply
    ///    nothing behind schedule to speak of.
    /// 2. `Complete` if every milestone is complete.
    /// 3. Over the incomplete milestones, `OffTrack` when the off-track ones
    ///    are half of them or more — a goal is off track when most of what
    ///    remains is off track, not because of a single straggler.
    /// 4. Otherwise `AtRisk` when any incomplete milestone is at risk or off
    ///    track (an off-track minority still puts the goal at risk).
    /// 5. Otherwise `OnTrack`.
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
        let mut incomplete = 0usize;
        let mut off_track = 0usize;
        let mut at_risk = 0usize;
        for milestone in milestones {
            match milestone.effective_status() {
                Status::Complete => {}
                Status::OffTrack => {
                    incomplete += 1;
                    off_track += 1;
                }
                Status::AtRisk => {
                    incomplete += 1;
                    at_risk += 1;
                }
                Status::OnTrack => incomplete += 1,
            }
        }
        if incomplete == 0 {
            return Status::Complete;
        }
        // Off track when the off-track milestones are half of the incomplete
        // ones or more; the doubled comparison keeps it exact in integers.
        if 2 * off_track >= incomplete {
            return Status::OffTrack;
        }
        if off_track > 0 || at_risk > 0 {
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
            owner_id: None,
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
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn milestone_with_effective_status(
        status: Status,
        override_status: Option<Status>,
    ) -> Milestone {
        let mut milestone = milestone_with_status(status);
        if let Some(override_status) = override_status {
            milestone.set_status_override(override_status);
        }
        milestone
    }

    #[test]
    fn rollup_follows_the_d19_rules() {
        let goal = test_goal();
        // Each case is (name, milestones as (automatic status, override), expected).
        let cases = [
            (
                "no milestones is on track",
                Vec::<(Status, Option<Status>)>::new(),
                Status::OnTrack,
            ),
            (
                "all on-track milestones are on track",
                vec![(Status::OnTrack, None), (Status::OnTrack, None)],
                Status::OnTrack,
            ),
            (
                "a single off-track straggler among three is at risk, not off track",
                vec![
                    (Status::OnTrack, None),
                    (Status::OffTrack, None),
                    (Status::OnTrack, None),
                ],
                Status::AtRisk,
            ),
            (
                "one off-track milestone out of two is half and makes the goal off track",
                vec![(Status::OffTrack, None), (Status::OnTrack, None)],
                Status::OffTrack,
            ),
            (
                "two off-track milestones out of three are more than half",
                vec![
                    (Status::OffTrack, None),
                    (Status::OffTrack, None),
                    (Status::OnTrack, None),
                ],
                Status::OffTrack,
            ),
            (
                "an at-risk milestone with no off-track ones is at risk",
                vec![
                    (Status::OnTrack, None),
                    (Status::AtRisk, None),
                    (Status::Complete, None),
                ],
                Status::AtRisk,
            ),
            (
                "an off-track minority alongside an at-risk milestone is at risk",
                vec![
                    (Status::OffTrack, None),
                    (Status::AtRisk, None),
                    (Status::OnTrack, None),
                    (Status::OnTrack, None),
                ],
                Status::AtRisk,
            ),
            (
                "off track wins over at risk at exactly half",
                vec![(Status::AtRisk, None), (Status::OffTrack, None)],
                Status::OffTrack,
            ),
            (
                "all complete milestones are complete",
                vec![(Status::Complete, None), (Status::Complete, None)],
                Status::Complete,
            ),
            (
                "complete milestones do not count until all are complete",
                vec![(Status::Complete, None), (Status::OnTrack, None)],
                Status::OnTrack,
            ),
            (
                "an overridden milestone counts by its effective status",
                vec![(Status::OnTrack, Some(Status::OffTrack))],
                Status::OffTrack,
            ),
            (
                "overrides can push the off-track count to half",
                vec![
                    (Status::OnTrack, Some(Status::OffTrack)),
                    (Status::OnTrack, Some(Status::OffTrack)),
                    (Status::OnTrack, None),
                ],
                Status::OffTrack,
            ),
        ];
        for (name, specs, expected) in cases {
            let milestones: Vec<Milestone> = specs
                .into_iter()
                .map(|(status, override_status)| {
                    milestone_with_effective_status(status, override_status)
                })
                .collect();
            assert_eq!(
                goal.compute_status_from_milestones(&milestones),
                expected,
                "{name}"
            );
        }
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
