//! Persistence port for [`Task`]s.

use domain::{MilestoneId, Task, TaskId, TaskStatus};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait TaskRepository: Send + Sync {
    async fn create(&self, task: Task) -> Result<Task, RepositoryError>;

    /// Returns `None` if no task has this id.
    async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError>;

    /// The tasks with the given ids, in id order; ids with no matching task
    /// are skipped. An empty slice yields an empty list.
    async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError>;

    async fn update(&self, task: Task) -> Result<Task, RepositoryError>;

    /// Change only the task's board column (and `updated_at`), in a single
    /// write that cannot clobber fields another writer changed in between;
    /// returns the updated task. An unknown id is a `NotFound`.
    async fn set_status(&self, id: TaskId, status: TaskStatus) -> Result<Task, RepositoryError>;

    async fn delete(&self, id: TaskId) -> Result<(), RepositoryError>;

    /// Tasks assigned to the given milestone.
    async fn list_by_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Task>, RepositoryError>;

    /// Tasks not assigned to any milestone.
    async fn list_unassigned(&self) -> Result<Vec<Task>, RepositoryError>;
}
