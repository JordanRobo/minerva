//! Tasks: the day-to-day work that moves toward a milestone.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::milestone::MilestoneId;

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
// JSON uses the same snake_case strings as the `tasks.status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// `blockers` are the tasks that block this one — the source tasks of
    /// the [`crate::task_relation::TaskRelationType::Blocks`] relations
    /// pointing at it (see [`crate::task_relation::TaskRelation::as_seen_by`]).
    /// A blocker that is already [`TaskStatus::Done`] no longer blocks:
    /// finished work holds nothing up.
    pub fn is_blocked(&self, blockers: &[Task]) -> bool {
        blockers
            .iter()
            .any(|blocker| blocker.status != TaskStatus::Done)
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

    fn task_with_status(status: TaskStatus) -> Task {
        let mut task = test_task(None);
        task.status = status;
        task
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
    fn task_with_no_blockers_is_not_blocked() {
        let task = test_task(None);
        assert!(!task.is_blocked(&[]));
    }

    #[test]
    fn task_whose_blockers_are_all_done_is_not_blocked() {
        // Finished work holds nothing up: a Done blocker no longer blocks.
        let task = test_task(None);
        let blockers = [
            task_with_status(TaskStatus::Done),
            task_with_status(TaskStatus::Done),
        ];
        assert!(!task.is_blocked(&blockers));
    }

    #[test]
    fn task_is_blocked_when_any_blocker_is_not_done() {
        let task = test_task(None);
        let blockers = [
            task_with_status(TaskStatus::Done),
            task_with_status(TaskStatus::InProgress),
            task_with_status(TaskStatus::Done),
        ];
        assert!(task.is_blocked(&blockers));
    }
}
