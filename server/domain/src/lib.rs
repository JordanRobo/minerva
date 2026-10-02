//! Minerva domain layer.
//!
//! Core business concepts and invariants for the school project-management
//! platform: goals, milestones, tasks, how they relate, and progress over
//! time.
//! This crate is pure logic with no I/O. Per `docs/architecture.md` it keeps
//! no I/O or framework dependencies beyond three value-type exceptions —
//! `uuid`, `chrono`, and `serde` — which contribute data types but no
//! behavior that touches the outside world.

pub mod goal;
pub mod goal_milestone;
pub mod milestone;
pub mod progress;
pub mod role;
pub mod session;
pub mod status;
pub mod task;
pub mod task_relation;
pub mod user;
pub mod user_identity;

pub use goal::{Goal, GoalId};
pub use goal_milestone::GoalMilestone;
pub use milestone::{Milestone, MilestoneId};
pub use progress::{ProgressError, ProgressSnapshot, ProgressSnapshotId, ProgressTarget};
pub use role::{DEFAULT_NEW_USER_ROLE, Permission, Role};
pub use session::{Session, SessionId};
pub use status::{GoalStatus, Status, StatusSource};
pub use task::{Task, TaskId, TaskStatus};
pub use task_relation::{TaskRelation, TaskRelationId, TaskRelationType};
pub use user::{User, UserId};
pub use user_identity::{UserIdentity, UserIdentityId};
