//! Task create/update/delete (roadmap 3.13, step 4): the handlers' writes go
//! through this service so the best-effort status recompute after each
//! mutation lives with the mutation, not in the handler.
//!
//! The recompute is best-effort, like the snapshot hook on overrides: the
//! write already committed, so a failure only warns (the scheduled sweep in
//! roadmap 3.13 heals any miss) and never fails the request.

use std::sync::Arc;

use domain::{MilestoneId, Task, TaskId};

use crate::ports::{RepositoryError, TaskRepository};
use crate::status_computation::StatusRecomputation;

/// An error from creating, updating or deleting a task.
#[derive(Debug)]
pub enum TaskServiceError {
    /// No task with the given id exists.
    NotFound,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for TaskServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskServiceError::NotFound => write!(f, "requested resource was not found"),
            TaskServiceError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TaskServiceError {}

/// Creates, updates and deletes tasks (roadmap 3.13, step 4), recomputing
/// the affected milestones' statuses — and the statuses of the goals linked
/// to them — after each successful write. Milestones first, then goals: a
/// goal's rollup reads its linked milestones' effective statuses.
pub struct TaskService {
    tasks: Arc<dyn TaskRepository>,
    computation: Arc<dyn StatusRecomputation>,
}

impl TaskService {
    pub fn new(tasks: Arc<dyn TaskRepository>, computation: Arc<dyn StatusRecomputation>) -> Self {
        Self { tasks, computation }
    }

    /// Create the task and recompute its milestone (when it has one) plus
    /// the goals linked to it.
    pub async fn create(&self, task: Task) -> Result<Task, TaskServiceError> {
        let task = self
            .tasks
            .create(task)
            .await
            .map_err(TaskServiceError::Repository)?;
        if let Some(milestone_id) = task.milestone_id {
            self.recompute_milestones(&[milestone_id]).await;
        }
        Ok(task)
    }

    /// Update the task and recompute every milestone it touched: both the
    /// old and the new one when it moved between them.
    pub async fn update(&self, task: Task) -> Result<Task, TaskServiceError> {
        let existing = self.find(task.id).await?;
        // The affected milestones are recomputed after the write, so the old
        // id must come from storage — the caller's copy may be stale.
        let mut affected = Vec::new();
        if let Some(old) = existing.milestone_id {
            affected.push(old);
        }
        if let Some(new) = task.milestone_id
            && !affected.contains(&new)
        {
            affected.push(new);
        }
        let task = self
            .tasks
            .update(task)
            .await
            .map_err(TaskServiceError::Repository)?;
        if !affected.is_empty() {
            self.recompute_milestones(&affected).await;
        }
        Ok(task)
    }

    /// Delete the task and recompute its milestone's status. The milestone
    /// id is captured before the delete: after it, the task is gone.
    pub async fn delete(&self, id: TaskId) -> Result<(), TaskServiceError> {
        let existing = self.find(id).await?;
        self.tasks
            .delete(id)
            .await
            .map_err(TaskServiceError::Repository)?;
        if let Some(milestone_id) = existing.milestone_id {
            self.recompute_milestones(&[milestone_id]).await;
        }
        Ok(())
    }

    async fn find(&self, id: TaskId) -> Result<Task, TaskServiceError> {
        self.tasks
            .find_by_id(id)
            .await
            .map_err(TaskServiceError::Repository)?
            .ok_or(TaskServiceError::NotFound)
    }

    /// Best-effort: the mutation already committed, so a recompute failure
    /// only warns (roadmap 3.13's sweep heals any miss).
    async fn recompute_milestones(&self, milestone_ids: &[MilestoneId]) {
        if let Err(err) = self.computation.recompute_milestones(milestone_ids).await {
            eprintln!("warning: status recompute after a task change failed: {err}");
        }
        if let Err(err) = self
            .computation
            .recompute_goals_for_milestones(milestone_ids)
            .await
        {
            eprintln!("warning: status recompute after a task change failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use domain::{MilestoneId, TaskStatus};

    use crate::ports::task_repository::fakes::InMemoryTaskRepository;
    use crate::status_computation::fakes::{RecomputeCall, RecordingRecomputation};

    use super::*;

    fn test_task(milestone_id: Option<MilestoneId>) -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            milestone_id,
            title: "Test task".to_owned(),
            description: None,
            status: TaskStatus::Backlog,
            target_date: None,
            assignee_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn setup() -> (
        Arc<InMemoryTaskRepository>,
        Arc<RecordingRecomputation>,
        TaskService,
    ) {
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let computation = Arc::new(RecordingRecomputation::new());
        let service = TaskService::new(tasks.clone(), computation.clone());
        (tasks, computation, service)
    }

    #[tokio::test]
    async fn create_recomputes_the_task_milestone_and_its_goals() {
        let (tasks, computation, service) = setup();
        let milestone_id = MilestoneId::new();
        tasks.create(test_task(Some(milestone_id))).await.unwrap();

        let created = service.create(test_task(Some(milestone_id))).await.unwrap();

        assert_eq!(created.milestone_id, Some(milestone_id));
        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![milestone_id]),
                RecomputeCall::GoalsForMilestones(vec![milestone_id])
            ]
        );
    }

    #[tokio::test]
    async fn create_without_a_milestone_recomputes_nothing() {
        let (_tasks, computation, service) = setup();

        service.create(test_task(None)).await.unwrap();

        assert!(computation.calls().is_empty());
    }

    #[tokio::test]
    async fn update_in_place_recomputes_only_that_milestone() {
        let (tasks, computation, service) = setup();
        let milestone_id = MilestoneId::new();
        let task = test_task(Some(milestone_id));
        tasks.create(task.clone()).await.unwrap();

        service.update(task).await.unwrap();

        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![milestone_id]),
                RecomputeCall::GoalsForMilestones(vec![milestone_id])
            ]
        );
    }

    #[tokio::test]
    async fn update_moving_between_milestones_recomputes_both() {
        let (tasks, computation, service) = setup();
        let old_id = MilestoneId::new();
        let new_id = MilestoneId::new();
        let mut task = test_task(Some(old_id));
        tasks.create(task.clone()).await.unwrap();
        task.milestone_id = Some(new_id);

        service.update(task).await.unwrap();

        // Both the old and the new milestone, in one batch.
        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![old_id, new_id]),
                RecomputeCall::GoalsForMilestones(vec![old_id, new_id])
            ]
        );
    }

    #[tokio::test]
    async fn update_unassigning_from_a_milestone_recomputes_it() {
        let (tasks, computation, service) = setup();
        let milestone_id = MilestoneId::new();
        let mut task = test_task(Some(milestone_id));
        tasks.create(task.clone()).await.unwrap();
        task.milestone_id = None;

        service.update(task).await.unwrap();

        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![milestone_id]),
                RecomputeCall::GoalsForMilestones(vec![milestone_id])
            ]
        );
    }

    #[tokio::test]
    async fn update_of_an_unassigned_task_recomputes_nothing() {
        let (tasks, computation, service) = setup();
        let task = test_task(None);
        tasks.create(task.clone()).await.unwrap();

        service.update(task).await.unwrap();

        assert!(computation.calls().is_empty());
    }

    #[tokio::test]
    async fn delete_recomputes_the_milestone_captured_before_the_delete() {
        let (tasks, computation, service) = setup();
        let milestone_id = MilestoneId::new();
        let task = test_task(Some(milestone_id));
        tasks.create(task.clone()).await.unwrap();

        service.delete(task.id).await.unwrap();

        assert!(tasks.find_by_id(task.id).await.unwrap().is_none());
        assert_eq!(
            computation.calls(),
            vec![
                RecomputeCall::Milestones(vec![milestone_id]),
                RecomputeCall::GoalsForMilestones(vec![milestone_id])
            ]
        );
    }

    #[tokio::test]
    async fn unknown_ids_are_not_found() {
        let (_tasks, _computation, service) = setup();

        assert!(matches!(
            service.update(test_task(None)).await,
            Err(TaskServiceError::NotFound)
        ));
        assert!(matches!(
            service.delete(TaskId::new()).await,
            Err(TaskServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_failing_recompute_does_not_fail_the_mutation() {
        let (tasks, computation, service) = setup();
        computation.set_fail(true);
        let milestone_id = MilestoneId::new();
        let task = test_task(Some(milestone_id));
        tasks.create(task.clone()).await.unwrap();

        // The write committed; the recompute failure only warns.
        let created = service.create(test_task(Some(milestone_id))).await.unwrap();
        assert_eq!(created.milestone_id, Some(milestone_id));

        let moved = service.update(created.clone()).await.unwrap();
        assert_eq!(moved.id, created.id);

        service.delete(created.id).await.unwrap();
        assert!(tasks.find_by_id(created.id).await.unwrap().is_none());
    }
}
