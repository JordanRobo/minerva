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
    /// blocks B, then B is blocked by A. It is accepted as a *submitted*
    /// relation type but never stored: writes are normalised to a
    /// [`TaskRelationType::Blocks`] row with the endpoints swapped (see
    /// [`TaskRelation::canonical_form`]), and the blocked-by view is derived
    /// when reading (see [`TaskRelation::as_seen_by`]).
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
/// Storage keeps one canonical row per relationship (roadmap 3.6, D4): a
/// submitted [`TaskRelationType::BlockedBy`] is stored as a
/// [`TaskRelationType::Blocks`] row with the endpoints swapped, and a
/// [`TaskRelationType::RelatesTo`] row stores the lower task id in
/// `source_task_id`. See [`TaskRelation::canonical_form`]; the blocked-by
/// view is derived when reading, see [`TaskRelation::as_seen_by`].
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

    /// The canonical storage form of a submitted relation (roadmap 3.6, D4):
    /// one row per relationship, so the same relationship submitted from
    /// either task — or as the inverse type — normalises to the same
    /// `(source, target, stored type)`.
    ///
    /// `task_id` is the task the relation is submitted from and
    /// `related_task_id` the one it names. A submitted `blocked_by` becomes
    /// a `blocks` row with the endpoints swapped; a `relates_to` row stores
    /// the lower task id in `source`. Self-relations normalise like any
    /// other pair and are rejected by the repository, not here.
    pub fn canonical_form(
        task_id: TaskId,
        related_task_id: TaskId,
        relation_type: TaskRelationType,
    ) -> (TaskId, TaskId, TaskRelationType) {
        match relation_type {
            TaskRelationType::Blocks => (task_id, related_task_id, TaskRelationType::Blocks),
            TaskRelationType::BlockedBy => (related_task_id, task_id, TaskRelationType::Blocks),
            TaskRelationType::RelatesTo => {
                if task_id.0 <= related_task_id.0 {
                    (task_id, related_task_id, TaskRelationType::RelatesTo)
                } else {
                    (related_task_id, task_id, TaskRelationType::RelatesTo)
                }
            }
        }
    }

    /// How `task_id` sees this stored relation: the type from its own
    /// perspective plus the id of the other task. `None` when `task_id` is
    /// neither endpoint.
    ///
    /// The blocked-by view is derived, not stored: the target of a `blocks`
    /// row reads it as `blocked_by`, and both endpoints of a `relates_to`
    /// row read it as `relates_to`. A stored `blocked_by` row is legacy
    /// pre-canonicalisation data that new writes never produce; from its
    /// source it reads as `blocked_by` and from its target as `blocks`.
    pub fn as_seen_by(&self, task_id: TaskId) -> Option<(TaskRelationType, TaskId)> {
        if self.source_task_id == task_id {
            Some((self.relation_type, self.target_task_id))
        } else if self.target_task_id == task_id {
            match self.relation_type {
                TaskRelationType::Blocks => {
                    Some((TaskRelationType::BlockedBy, self.source_task_id))
                }
                TaskRelationType::BlockedBy => {
                    Some((TaskRelationType::Blocks, self.source_task_id))
                }
                TaskRelationType::RelatesTo => {
                    Some((TaskRelationType::RelatesTo, self.source_task_id))
                }
            }
        } else {
            None
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
        let relation =
            TaskRelation::new(source, target, TaskRelationType::Blocks, test_timestamp());
        assert_eq!(relation.source_task_id, source);
        assert_eq!(relation.target_task_id, target);
        assert_eq!(relation.relation_type, TaskRelationType::Blocks);
    }

    /// A deterministic task id for ordering-sensitive tests.
    fn task_id(n: u128) -> TaskId {
        TaskId(Uuid::from_u128(n))
    }

    #[test]
    fn blocks_normalisation_keeps_the_submitted_direction() {
        let (a, b) = (task_id(1), task_id(2));
        assert_eq!(
            TaskRelation::canonical_form(a, b, TaskRelationType::Blocks),
            (a, b, TaskRelationType::Blocks)
        );
        assert_eq!(
            TaskRelation::canonical_form(b, a, TaskRelationType::Blocks),
            (b, a, TaskRelationType::Blocks)
        );
    }

    #[test]
    fn blocked_by_normalises_to_a_blocks_row_with_swapped_endpoints() {
        let (a, b) = (task_id(1), task_id(2));
        // "a is blocked by b" stores as "b blocks a".
        assert_eq!(
            TaskRelation::canonical_form(a, b, TaskRelationType::BlockedBy),
            (b, a, TaskRelationType::Blocks)
        );
        assert_eq!(
            TaskRelation::canonical_form(b, a, TaskRelationType::BlockedBy),
            (a, b, TaskRelationType::Blocks)
        );
        // Both submissions describe the same relationship.
        assert_eq!(
            TaskRelation::canonical_form(a, b, TaskRelationType::BlockedBy),
            TaskRelation::canonical_form(b, a, TaskRelationType::Blocks)
        );
    }

    #[test]
    fn relates_to_normalisation_orders_the_pair_by_task_id() {
        let (a, b) = (task_id(1), task_id(2));
        // Either submission order yields the same canonical row...
        assert_eq!(
            TaskRelation::canonical_form(a, b, TaskRelationType::RelatesTo),
            TaskRelation::canonical_form(b, a, TaskRelationType::RelatesTo)
        );
        // ...with the lower task id in source.
        assert_eq!(
            TaskRelation::canonical_form(b, a, TaskRelationType::RelatesTo),
            (a, b, TaskRelationType::RelatesTo)
        );
    }

    #[test]
    fn canonical_form_is_idempotent() {
        let (a, b) = (task_id(1), task_id(2));
        for submitted in [
            TaskRelationType::Blocks,
            TaskRelationType::BlockedBy,
            TaskRelationType::RelatesTo,
        ] {
            let once = TaskRelation::canonical_form(a, b, submitted);
            let twice = TaskRelation::canonical_form(once.0, once.1, once.2);
            assert_eq!(once, twice);
        }
    }

    #[test]
    fn a_stored_blocks_row_reads_as_blocks_from_source_and_blocked_by_from_target() {
        let (a, b) = (task_id(1), task_id(2));
        let relation = TaskRelation::new(a, b, TaskRelationType::Blocks, test_timestamp());
        assert_eq!(relation.as_seen_by(a), Some((TaskRelationType::Blocks, b)));
        assert_eq!(
            relation.as_seen_by(b),
            Some((TaskRelationType::BlockedBy, a))
        );
    }

    #[test]
    fn a_stored_relates_to_row_reads_the_same_from_both_endpoints() {
        let (a, b) = (task_id(1), task_id(2));
        let relation = TaskRelation::new(a, b, TaskRelationType::RelatesTo, test_timestamp());
        assert_eq!(
            relation.as_seen_by(a),
            Some((TaskRelationType::RelatesTo, b))
        );
        assert_eq!(
            relation.as_seen_by(b),
            Some((TaskRelationType::RelatesTo, a))
        );
    }

    #[test]
    fn as_seen_by_is_none_for_a_task_that_is_not_an_endpoint() {
        let (a, b) = (task_id(1), task_id(2));
        let other = task_id(3);
        let relation = TaskRelation::new(a, b, TaskRelationType::Blocks, test_timestamp());
        assert_eq!(relation.as_seen_by(other), None);
    }

    #[test]
    fn a_legacy_stored_blocked_by_row_reads_as_its_literal_direction() {
        // Pre-canonicalisation rows are converted away by the 3.6 migration;
        // this pins the read semantics in case one is ever met.
        let (a, b) = (task_id(1), task_id(2));
        let relation = TaskRelation::new(a, b, TaskRelationType::BlockedBy, test_timestamp());
        assert_eq!(
            relation.as_seen_by(a),
            Some((TaskRelationType::BlockedBy, b))
        );
        assert_eq!(relation.as_seen_by(b), Some((TaskRelationType::Blocks, a)));
    }
}
