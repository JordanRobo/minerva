//! Persistence port for [`Milestone`]s.

use domain::{Milestone, MilestoneId, Status};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait MilestoneRepository: Send + Sync {
    async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError>;

    /// Returns `None` if no milestone has this id.
    async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError>;

    async fn list(&self) -> Result<Vec<Milestone>, RepositoryError>;

    async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError>;

    /// Set or clear the milestone's manual status override; `None` clears
    /// it. Never touches the automatic status (roadmap 3.2).
    async fn set_status_override(
        &self,
        id: MilestoneId,
        status_override: Option<Status>,
    ) -> Result<(), RepositoryError>;

    async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError>;
}
