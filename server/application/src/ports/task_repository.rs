//! Persistence port for [`Task`]s.

use domain::{MilestoneId, Task, TaskId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait TaskRepository: Send + Sync {
    async fn create(&self, task: Task) -> Result<Task, RepositoryError>;

    /// Returns `None` if no task has this id.
    async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError>;

    async fn update(&self, task: Task) -> Result<Task, RepositoryError>;

    async fn delete(&self, id: TaskId) -> Result<(), RepositoryError>;

    /// Tasks assigned to the given milestone.
    async fn list_by_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Task>, RepositoryError>;

    /// Tasks not assigned to any milestone.
    async fn list_unassigned(&self) -> Result<Vec<Task>, RepositoryError>;
}
