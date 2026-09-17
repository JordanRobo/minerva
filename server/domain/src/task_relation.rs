//! Relations between tasks: how one piece of day-to-day work connects to
//! another (for example, one task must finish before another can start).

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::task::TaskId;

/// Identifier for a [`TaskRelation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TaskRelationId(pub Uuid);

impl TaskRelationId {
    /// Create a fresh identifier for a relation that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskRelationId {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of connection a [`TaskRelation`] describes, read from the
/// relation's source task toward its target task: "the source task
/// *blocks* / *is blocked by* / *relates to* the target task".
///
/// More variants may be added over time (for example a "part of" or
/// "follows" connection). Code that matches on this enum should keep a
/// wildcard arm so that adding a variant is not a breaking change for it.
///
/// `#[non_exhaustive]` is used here deliberately: it makes the compiler
/// enforce that wildcard rule in every crate outside `domain`, and putting
/// it in place now — before the application and interface crates start using
/// this type — avoids a breaking change later. The tradeoff is that
/// downstream code can never match this enum exhaustively, even when it has
/// good reason to believe the set of variants is stable; within this crate
/// exhaustive matches are still allowed, and constructing the known variants
/// works as usual everywhere.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TaskRelationType {
    /// The source task must be finished before the target task can proceed.
    Blocks,
    /// The source task cannot proceed until the target task is finished.
    ///
    /// This is the logical inverse of [`TaskRelationType::Blocks`]: if A
    /// blocks B, then B is blocked by A. This type does not enforce or
    /// auto-generate that pairing — see the note on [`TaskRelation`].
    BlockedBy,
    /// The two tasks are connected, without saying which one comes first.
    RelatesTo,
}

/// One directed relation between two tasks.
///
/// `source_task_id` is the task the statement is made from and
/// `target_task_id` is the task it points at, so a relation reads as "the
/// source task <relation type> the target task".
///
/// [`TaskRelationType::Blocks`] and [`TaskRelationType::BlockedBy`] are
/// logical inverses of each other (if A blocks B, that implies B is blocked
/// by A), but this domain type does not enforce or auto-generate the inverse
/// relation.
// TODO(application): decide where and how to keep Blocks/BlockedBy pairs
// consistent when relations are created or removed; that orchestration
// belongs in the application layer, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRelation {
    pub id: TaskRelationId,
    pub source_task_id: TaskId,
    pub target_task_id: TaskId,
    pub relation_type: TaskRelationType,
    pub created_at: DateTime<Utc>,
}

impl TaskRelation {
    /// Link two tasks with a relation type.
    pub fn new(
        source_task_id: TaskId,
        target_task_id: TaskId,
        relation_type: TaskRelationType,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: TaskRelationId::new(),
            source_task_id,
            target_task_id,
            relation_type,
            created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn test_timestamp() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    /// Describe a relation type in plain words. The wildcard arm is what
    /// makes this future-proof: if a new variant is added to
    /// [`TaskRelationType`] later, this function still compiles — the new
    /// variant simply falls through to "some other kind of connection" until
    /// someone adds a word for it here. (Outside the domain crate the
    /// wildcard arm is required by `#[non_exhaustive]`; inside the crate it
    /// is just good practice.)
    ///
    /// The arm is unreachable *today* — this crate can see every current
    /// variant, so the compiler would otherwise warn about it. The allow is
    /// deliberate: the arm exists for variants that do not exist yet.
    #[allow(unreachable_patterns)]
    fn describe(relation_type: TaskRelationType) -> &'static str {
        match relation_type {
            TaskRelationType::Blocks => "blocks",
            TaskRelationType::BlockedBy => "is blocked by",
            TaskRelationType::RelatesTo => "relates to",
            _ => "some other kind of connection",
        }
    }

    #[test]
    fn known_relation_types_match_without_reaching_the_wildcard_arm() {
        assert_eq!(describe(TaskRelationType::Blocks), "blocks");
        assert_eq!(describe(TaskRelationType::BlockedBy), "is blocked by");
        assert_eq!(describe(TaskRelationType::RelatesTo), "relates to");
    }

    #[test]
    fn a_relation_links_two_tasks_with_a_type() {
        let source = TaskId::new();
        let target = TaskId::new();
        let relation = TaskRelation::new(
            source,
            target,
            TaskRelationType::Blocks,
            test_timestamp(),
        );
        assert_eq!(relation.source_task_id, source);
        assert_eq!(relation.target_task_id, target);
        assert_eq!(relation.relation_type, TaskRelationType::Blocks);
    }
}
