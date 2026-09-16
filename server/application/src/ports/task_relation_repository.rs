//! Persistence port for [`TaskRelation`]s.

use domain::{TaskId, TaskRelation, TaskRelationId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait TaskRelationRepository: Send + Sync {
    async fn create(&self, relation: TaskRelation) -> Result<TaskRelation, RepositoryError>;

    async fn delete(&self, id: TaskRelationId) -> Result<(), RepositoryError>;

    /// Relations in which the task participates as either source or target.
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<TaskRelation>, RepositoryError>;
}
