//! The many-to-many association between goals and milestones.

use crate::goal::GoalId;
use crate::milestone::MilestoneId;

/// A goal–milestone association, modeled as a first-class entity rather than
/// a bare pair of ids so that fields describing *how* a milestone contributes
/// to a goal (for example a weight or an expected share of the work) can be
/// added later without changing the shape callers already use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalMilestone {
    pub goal_id: GoalId,
    pub milestone_id: MilestoneId,
}

impl GoalMilestone {
    /// Link a milestone to a goal.
    pub fn new(goal_id: GoalId, milestone_id: MilestoneId) -> Self {
        Self {
            goal_id,
            milestone_id,
        }
    }
}
