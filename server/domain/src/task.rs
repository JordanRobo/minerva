//! Tasks: the day-to-day work that moves toward a milestone.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::milestone::MilestoneId;
use crate::task_relation::{TaskRelation, TaskRelationType};

/// Identifier for a [`Task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TaskId(pub Uuid);

impl TaskId {
    /// Create a fresh identifier for a task that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a task sits on the day-to-day board.
///
/// This tracks *position* (waiting, ready, underway, finished) and is
/// deliberately separate from [`crate::status::Status`], which describes how
/// healthy a goal or milestone is against its target date. The two answer
/// different questions and neither is derived from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TaskStatus {
    /// Not planned to start yet.
    Backlog,
    /// Ready to be started.
    ToDo,
    /// Currently being worked on.
    InProgress,
    /// Finished.
    Done,
}

/// A unit of day-to-day work, optionally attached to a milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: TaskId,
    /// The milestone this task contributes to, if one has been chosen. A
    /// task may exist before it is assigned anywhere.
    pub milestone_id: Option<MilestoneId>,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    /// The date this task is expected to be finished by, if one was set.
    pub target_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    /// Whether this task is currently blocked by something else.
    ///
    /// A task is blocked when another task has a
    /// [`TaskRelationType::Blocks`] relation pointing at it — that other
    /// task's id is the relation's `source_task_id` and this task's id is its
    /// `target_task_id`. A [`TaskRelationType::BlockedBy`] relation pointing
    /// *at* this task does not count: it says the *other* task is blocked by
    /// this one, so direction matters.
    pub fn is_blocked(&self, relations: &[TaskRelation]) -> bool {
        relations.iter().any(|relation| {
            relation.target_task_id == self.id
                && relation.relation_type == TaskRelationType::Blocks
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::milestone::MilestoneId;

    fn test_timestamp() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    fn test_task(milestone_id: Option<MilestoneId>) -> Task {
        let now = test_timestamp();
        Task {
            id: TaskId::new(),
            milestone_id,
            title: "Test task".to_owned(),
            description: None,
            status: TaskStatus::ToDo,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn relation_between(
        source: TaskId,
        target: TaskId,
        relation_type: TaskRelationType,
    ) -> TaskRelation {
        TaskRelation::new(source, target, relation_type, test_timestamp())
    }

    #[test]
    fn task_can_be_assigned_to_a_milestone() {
        let milestone_id = MilestoneId::new();
        let task = test_task(Some(milestone_id));
        assert_eq!(task.milestone_id, Some(milestone_id));
        assert_eq!(task.status, TaskStatus::ToDo);
    }

    #[test]
    fn task_can_exist_without_a_milestone() {
        let task = test_task(None);
        assert_eq!(task.milestone_id, None);
    }

    #[test]
    fn task_with_no_relations_is_not_blocked() {
        let task = test_task(None);
        assert!(!task.is_blocked(&[]));
    }

    #[test]
    fn task_with_a_blocks_relation_pointing_at_it_is_blocked() {
        let task = test_task(None);
        let other = TaskId::new();
        let relations = [relation_between(other, task.id, TaskRelationType::Blocks)];
        assert!(task.is_blocked(&relations));
    }

    #[test]
    fn task_with_a_blocked_by_relation_pointing_at_it_is_not_blocked() {
        // A BlockedBy relation pointing at the task says the *other* task is
        // blocked by this one, so it does not block this task.
        let task = test_task(None);
        let other = TaskId::new();
        let relations = [relation_between(other, task.id, TaskRelationType::BlockedBy)];
        assert!(!task.is_blocked(&relations));
    }

    #[test]
    fn task_that_blocks_another_task_is_not_blocked_itself() {
        // Direction matters: an outgoing Blocks relation means this task
        // blocks someone else, not that it is blocked.
        let task = test_task(None);
        let other = TaskId::new();
        let relations = [relation_between(task.id, other, TaskRelationType::Blocks)];
        assert!(!task.is_blocked(&relations));
    }

    #[test]
    fn task_among_several_relations_is_blocked_if_any_blocks_relation_points_at_it() {
        let task = test_task(None);
        let first_other = TaskId::new();
        let second_other = TaskId::new();
        let relations = [
            relation_between(task.id, first_other, TaskRelationType::Blocks),
            relation_between(second_other, task.id, TaskRelationType::RelatesTo),
            relation_between(first_other, task.id, TaskRelationType::Blocks),
        ];
        assert!(task.is_blocked(&relations));
    }
}
