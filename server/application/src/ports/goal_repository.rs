//! Persistence port for [`Goal`]s.

use domain::{Goal, GoalId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait GoalRepository: Send + Sync {
    async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    /// Returns `None` if no goal has this id.
    async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError>;

    async fn list(&self) -> Result<Vec<Goal>, RepositoryError>;

    async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError>;

    async fn delete(&self, id: GoalId) -> Result<(), RepositoryError>;
}
