//! Status types shared by goals and milestones.

use serde::Serialize;

/// The state of a goal or milestone with respect to its target date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// Progress is moving along as planned.
    OnTrack,
    /// Progress is slower than planned and the target date may be missed.
    AtRisk,
    /// The target date will be missed unless something changes.
    OffTrack,
    /// The work is finished.
    Complete,
}

/// Where a [`GoalStatus`] value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StatusSource {
    /// The status was worked out by the system from the underlying data.
    Computed,
    /// A person set the status by hand, overriding the system's view.
    ManualOverride,
}

/// A status together with where it came from, so callers can always tell a
/// system-computed value apart from one that was overridden by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GoalStatus {
    pub status: Status,
    pub source: StatusSource,
}

impl GoalStatus {
    /// A status produced by the system's own computation.
    pub fn computed(status: Status) -> Self {
        Self {
            status,
            source: StatusSource::Computed,
        }
    }

    /// A status set manually, overriding whatever the system computes.
    pub fn manual_override(status: Status) -> Self {
        Self {
            status,
            source: StatusSource::ManualOverride,
        }
    }
}
