//! Persistence port for the goal–milestone relation.

use domain::{GoalId, GoalMilestone, MilestoneId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait GoalMilestoneRepository: Send + Sync {
    async fn link(&self, link: GoalMilestone) -> Result<(), RepositoryError>;

    async fn unlink(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<(), RepositoryError>;

    async fn milestones_for_goal(&self, goal_id: GoalId) -> Result<Vec<MilestoneId>, RepositoryError>;

    async fn goals_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<GoalId>, RepositoryError>;
}
