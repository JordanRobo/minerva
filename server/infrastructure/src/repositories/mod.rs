//! Postgres implementations of the application layer's repository ports.
//!
//! Each repository wraps a [`crate::db::PgPool`] and runs its Diesel queries
//! on the blocking thread pool via `tokio::task::spawn_blocking`, so async
//! callers never block the runtime.
//!
//! Note for future work: `task_relations` has two foreign keys to `tasks`
//! (`source_task_id` and `target_task_id`). Diesel's `joinable!` cannot
//! declare a default join column for such a table — two declarations for the
//! same pair are conflicting implementations, and `print-schema` omits the
//! ambiguous pair — so joins between `task_relations` and `tasks` must
//! always spell out their `ON` clause explicitly.

pub mod goal_repository;
pub mod mapping;
pub mod milestone_repository;

pub use goal_repository::PostgresGoalRepository;
pub use milestone_repository::PostgresMilestoneRepository;
