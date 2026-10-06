//! Snapshot hook for manual status overrides (roadmap 3.2).
//!
//! The override service calls this after a goal's or milestone's manual
//! status override was set or cleared successfully, so a progress snapshot
//! can be taken at the moment the status changed (roadmap 3.14). Until then
//! only the no-op implementation exists. The call is best-effort, like email
//! delivery: a hook failure is logged by the caller and never fails the
//! override itself.

use async_trait::async_trait;
use domain::{GoalId, MilestoneId};

/// An error from taking a status snapshot.
#[derive(Debug)]
pub struct StatusSnapshotError(pub String);

impl std::fmt::Display for StatusSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StatusSnapshotError {}

/// Which entity's manual status override changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusChangeTarget {
    Goal(GoalId),
    Milestone(MilestoneId),
}

/// Called after a goal's or milestone's manual status override was set or
/// cleared successfully (roadmap 3.2). The call is best-effort: an error is
/// logged by the caller and does not fail the override.
#[async_trait]
pub trait StatusSnapshotTrigger: Send + Sync {
    async fn snapshot(&self, target: StatusChangeTarget) -> Result<(), StatusSnapshotError>;
}

/// A [`StatusSnapshotTrigger`] that does nothing: snapshots are not built
/// yet (roadmap 3.14), so the override service has its seam without any side
/// effect.
pub struct NoopStatusSnapshotTrigger;

#[async_trait]
impl StatusSnapshotTrigger for NoopStatusSnapshotTrigger {
    async fn snapshot(&self, _target: StatusChangeTarget) -> Result<(), StatusSnapshotError> {
        Ok(())
    }
}
