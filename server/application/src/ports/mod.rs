//! Storage ports for the application layer.
//!
//! Traits describing how use cases persist and retrieve domain objects, in
//! terms of `domain` types only. The infrastructure layer implements them
//! (e.g. with Diesel/Postgres) and the interface composition root supplies
//! concrete implementations at startup, so the application layer stays
//! decoupled from any specific persistence technology.

pub mod goal_milestone_repository;
pub mod goal_repository;
pub mod milestone_repository;
pub mod progress_snapshot_repository;
pub mod task_relation_repository;
pub mod task_repository;

pub use goal_milestone_repository::GoalMilestoneRepository;
pub use goal_repository::GoalRepository;
pub use milestone_repository::MilestoneRepository;
pub use progress_snapshot_repository::ProgressSnapshotRepository;
pub use task_relation_repository::TaskRelationRepository;
pub use task_repository::TaskRepository;

/// An error from a repository operation.
#[derive(Debug)]
pub enum RepositoryError {
    /// The requested entity does not exist.
    NotFound,
    /// The operation conflicts with the current state (e.g. a duplicate).
    Conflict(String),
    /// The operation referenced an entity that does not exist (e.g. a foreign
    /// key pointing at a missing row).
    InvalidReference(String),
    /// Something unexpected went wrong (I/O failure, constraint violation, ...).
    Unexpected(String),
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepositoryError::NotFound => write!(f, "requested resource was not found"),
            RepositoryError::Conflict(detail) => write!(f, "conflict: {detail}"),
            RepositoryError::InvalidReference(detail) => write!(f, "invalid reference: {detail}"),
            RepositoryError::Unexpected(detail) => {
                write!(f, "unexpected repository error: {detail}")
            }
        }
    }
}

impl std::error::Error for RepositoryError {}
