//! Minerva domain layer.
//!
//! Core business concepts and invariants for the school project-management
//! platform: goals, milestones, how they relate, and progress over time.
//! This crate is pure logic with no I/O. Per `docs/architecture.md` it keeps
//! zero external dependencies beyond two value-type exceptions — `uuid` and
//! `chrono` — which contribute data types but no behavior that touches the
//! outside world.

pub mod goal;
pub mod goal_milestone;
pub mod milestone;
pub mod progress;
pub mod status;

pub use goal::{Goal, GoalId};
pub use goal_milestone::GoalMilestone;
pub use milestone::{Milestone, MilestoneId};
pub use progress::{ProgressError, ProgressSnapshot, ProgressSnapshotId, ProgressTarget};
pub use status::{GoalStatus, Status, StatusSource};
