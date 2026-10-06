//! Persistence port for the goal–milestone relation.

use domain::{Goal, GoalId, GoalMilestone, Milestone, MilestoneId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait GoalMilestoneRepository: Send + Sync {
    /// Idempotent: linking a pair that is already linked is a no-op. Returns
    /// whether a new link was created (`false` when the pair was already
    /// linked, including when a concurrent insert won the race).
    async fn link(&self, link: GoalMilestone) -> Result<bool, RepositoryError>;

    /// Idempotent: unlinking a pair that is not linked is a no-op. Returns
    /// whether a link was removed.
    async fn unlink(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<bool, RepositoryError>;

    /// The milestones linked to the goal, ordered by target date (nulls
    /// last), then created_at, then id.
    async fn milestones_for_goal(&self, goal_id: GoalId)
    -> Result<Vec<Milestone>, RepositoryError>;

    /// The goals linked to the milestone, in the same order.
    async fn goals_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Goal>, RepositoryError>;
}
