//! Persistence port for [`ProgressSnapshot`]s.
//!
//! Snapshots are an append-only historical record, so this port has no
//! update or delete methods.

use domain::{ProgressSnapshot, ProgressTarget};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait ProgressSnapshotRepository: Send + Sync {
    async fn create(&self, snapshot: ProgressSnapshot) -> Result<ProgressSnapshot, RepositoryError>;

    /// Snapshots for the target, ordered oldest-to-newest by `recorded_at`.
    async fn list_for_target(
        &self,
        target: ProgressTarget,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError>;
}
