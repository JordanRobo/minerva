//! Milestones: the measurable steps a goal is broken into.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::status::{Status, StatusSource};

/// Identifier for a [`Milestone`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct MilestoneId(pub Uuid);

impl MilestoneId {
    /// Create a fresh identifier for a milestone that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MilestoneId {
    fn default() -> Self {
        Self::new()
    }
}

/// A single measurable step on the way to a goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Milestone {
    pub id: MilestoneId,
    pub title: String,
    pub description: Option<String>,
    /// The automatic status, as worked out from the milestone's tasks and
    /// target date. A manual override, when set, takes its place for display
    /// and rollups.
    pub status: Status,
    /// A manual override of the automatic status. It is sticky: held until
    /// explicitly cleared, never touched by recomputation.
    pub status_override: Option<Status>,
    /// The date this milestone is expected to be finished by, if one was set.
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Milestone {
    /// The status to show and roll up: the manual override when one is set,
    /// otherwise the automatic status.
    pub fn effective_status(&self) -> Status {
        self.status_override.unwrap_or(self.status)
    }

    /// Where [`Milestone::effective_status`] came from: a manual override
    /// when one is set, the system's computation otherwise.
    pub fn status_source(&self) -> StatusSource {
        match self.status_override {
            Some(_) => StatusSource::ManualOverride,
            None => StatusSource::Computed,
        }
    }

    /// Set a manual override. It holds until
    /// [`Milestone::clear_status_override`]; recomputation never removes it.
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

    fn test_milestone() -> Milestone {
        let now = test_timestamp();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn effective_status_without_an_override_is_the_stored_status() {
        let milestone = test_milestone();
        assert_eq!(milestone.effective_status(), Status::OnTrack);
        assert_eq!(milestone.status_source(), StatusSource::Computed);
    }

    #[test]
    fn setting_an_override_flips_the_effective_status_and_source() {
        let mut milestone = test_milestone();
        milestone.set_status_override(Status::AtRisk);
        assert_eq!(milestone.effective_status(), Status::AtRisk);
        assert_eq!(milestone.status_source(), StatusSource::ManualOverride);
        // The automatic status is untouched by the override.
        assert_eq!(milestone.status, Status::OnTrack);
    }

    #[test]
    fn apply_computed_status_leaves_an_active_override_untouched() {
        let mut milestone = test_milestone();
        milestone.set_status_override(Status::AtRisk);
        milestone.apply_computed_status(Status::OffTrack);
        assert_eq!(milestone.status, Status::OffTrack);
        assert_eq!(milestone.status_override, Some(Status::AtRisk));
        assert_eq!(milestone.effective_status(), Status::AtRisk);
        assert_eq!(milestone.status_source(), StatusSource::ManualOverride);
    }

    #[test]
    fn clearing_an_override_returns_to_the_computed_status() {
        let mut milestone = test_milestone();
        milestone.set_status_override(Status::Complete);
        milestone.apply_computed_status(Status::AtRisk);
        milestone.clear_status_override();
        assert_eq!(milestone.status_override, None);
        assert_eq!(milestone.effective_status(), Status::AtRisk);
        assert_eq!(milestone.status_source(), StatusSource::Computed);
    }
}
