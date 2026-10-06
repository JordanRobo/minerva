//! Status types shared by goals and milestones.

use serde::{Deserialize, Serialize};

/// The state of a goal or milestone with respect to its target date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Where an effective status came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StatusSource {
    /// The status was worked out by the system from the underlying data.
    #[serde(rename = "automatic")]
    Computed,
    /// A person set the status by hand, overriding the system's view.
    #[serde(rename = "manual")]
    ManualOverride,
}
