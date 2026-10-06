//! Persistence port for [`Goal`]s.

use domain::{Goal, GoalId, Status};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait GoalRepository: Send + Sync {
    async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    /// Returns `None` if no goal has this id.
    async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError>;

    async fn list(&self) -> Result<Vec<Goal>, RepositoryError>;

    async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    /// Set or clear the goal's manual status override; `None` clears it.
    /// Never touches the automatic status (roadmap 3.2).
    async fn set_status_override(
        &self,
        id: GoalId,
        status_override: Option<Status>,
    ) -> Result<(), RepositoryError>;

    async fn delete(&self, id: GoalId) -> Result<(), RepositoryError>;
}
