//! Milestones: the measurable steps a goal is broken into.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::status::GoalStatus;

/// Identifier for a [`Milestone`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Milestone {
    pub id: MilestoneId,
    pub title: String,
    pub description: Option<String>,
    pub status: GoalStatus,
    /// The date this milestone is expected to be finished by, if one was set.
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
