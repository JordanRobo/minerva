//! Board-state transitions (roadmap 3.7, D6): moving a task between the
//! board columns (`backlog` / `to_do` / `in_progress` / `done`).
//!
//! The rules live here, not in the handler: any column may move to any other
//! (there are no workflow restrictions in v1), a blocked task is never
//! refused — blocking is a UI warning, not a hard stop — and setting the
//! column a task already sits in is an idempotent no-op that returns the task
//! without writing, so its `updated_at` is left alone. The write itself is a
//! single-column update (see [`TaskRepository::set_status`]), so it cannot
//! clobber a field another writer changed in between. Nothing here touches
//! goal or milestone status: board position is not health, and the snapshot
//! hook belongs to 3.14.

use std::sync::Arc;

use domain::{Task, TaskId, TaskStatus};

use crate::ports::{RepositoryError, TaskRepository};

/// An error from moving a task to another board column.
#[derive(Debug)]
pub enum TaskStatusError {
    /// No task with the given id exists.
    NotFound,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for TaskStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatusError::NotFound => write!(f, "requested resource was not found"),
            TaskStatusError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TaskStatusError {}

/// Moves a task between board columns (roadmap 3.7, D6). The service takes no
/// notion of blocking or of goal/milestone status: a card moves regardless of
/// what blocks it, and moving it changes nothing but the column.
pub struct TaskStatusService {
    tasks: Arc<dyn TaskRepository>,
}

impl TaskStatusService {
    pub fn new(tasks: Arc<dyn TaskRepository>) -> Self {
        Self { tasks }
    }

    /// Move the task to `status`. Any column may move to any other. Moving it
    /// to the column it already sits in is a no-op that still returns the
    /// task but writes nothing, so `updated_at` is unchanged.
    pub async fn set_status(
        &self,
        id: TaskId,
        status: TaskStatus,
    ) -> Result<Task, TaskStatusError> {
        let existing = self.find(id).await?;
        if existing.status == status {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.tasks
            .set_status(id, status)
            .await
            .map_err(TaskStatusError::Repository)
    }

    async fn find(&self, id: TaskId) -> Result<Task, TaskStatusError> {
        self.tasks
            .find_by_id(id)
            .await
            .map_err(TaskStatusError::Repository)?
            .ok_or(TaskStatusError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use chrono::Utc;

    use super::*;

    fn test_task(status: TaskStatus) -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            milestone_id: None,
            title: "Test task".to_owned(),
            description: None,
            status,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// An in-memory [`TaskRepository`] mirroring Postgres: `set_status` writes
    /// only the column and bumps `updated_at`, answers `NotFound` for an
    /// unknown id, and fails on demand. It counts real writes so a test can
    /// prove a no-op wrote nothing.
    #[derive(Default)]
    struct InMemoryTaskRepository {
        tasks: Mutex<HashMap<TaskId, Task>>,
        fail_set_status: AtomicBool,
        set_status_writes: AtomicUsize,
    }

    impl InMemoryTaskRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_status` fail (or succeed again).
        fn set_fail_set_status(&self, fail: bool) {
            self.fail_set_status.store(fail, Ordering::Relaxed);
        }

        /// How many times `set_status` actually wrote.
        fn writes(&self) -> usize {
            self.set_status_writes.load(Ordering::Relaxed)
        }

        fn locked(&self) -> MutexGuard<'_, HashMap<TaskId, Task>> {
            self.tasks.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl TaskRepository for InMemoryTaskRepository {
        async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
            self.locked().insert(task.id, task.clone());
            Ok(task)
        }

        async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
            Ok(self.locked().get(&id).cloned())
        }

        async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            let tasks = self.locked();
            let mut found: Vec<Task> = ids.iter().filter_map(|id| tasks.get(id).cloned()).collect();
            found.sort_by_key(|task| task.id.0);
            Ok(found)
        }

        async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
            if self.locked().contains_key(&task.id) {
                Ok(task)
            } else {
                Err(RepositoryError::NotFound)
            }
        }

        async fn set_status(
            &self,
            id: TaskId,
            status: TaskStatus,
        ) -> Result<Task, RepositoryError> {
            if self.fail_set_status.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected(
                    "faking a status write failure".to_owned(),
                ));
            }
            let mut tasks = self.locked();
            let Some(task) = tasks.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the column and the
            // updated_at timestamp.
            task.status = status;
            task.updated_at = Utc::now();
            self.set_status_writes.fetch_add(1, Ordering::Relaxed);
            Ok(task.clone())
        }

        async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
            if self.locked().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &crate::ports::TaskListFilter,
            page: &crate::pagination::PageRequest,
        ) -> Result<crate::pagination::Page<Task>, RepositoryError> {
            let matched = crate::ports::task_repository::fakes::apply_task_list_filter(
                self.locked().values(),
                filter,
            );
            Ok(crate::ports::task_repository::fakes::page_tasks(
                matched, page,
            ))
        }
    }

    fn setup() -> (Arc<InMemoryTaskRepository>, TaskStatusService) {
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let service = TaskStatusService::new(tasks.clone());
        (tasks, service)
    }

    /// Every column of the board, in a stable order.
    fn all_statuses() -> [TaskStatus; 4] {
        [
            TaskStatus::Backlog,
            TaskStatus::ToDo,
            TaskStatus::InProgress,
            TaskStatus::Done,
        ]
    }

    #[tokio::test]
    async fn any_column_may_move_to_any_other_column() {
        let (tasks, service) = setup();
        for from in all_statuses() {
            let task = test_task(from);
            tasks.create(task.clone()).await.unwrap();
            for to in all_statuses() {
                if to == from {
                    continue;
                }
                let moved = service.set_status(task.id, to).await.unwrap();
                assert_eq!(moved.status, to, "{from:?} -> {to:?}");
            }
        }
    }

    #[tokio::test]
    async fn moving_into_and_out_of_done_is_allowed() {
        let (tasks, service) = setup();
        let task = test_task(TaskStatus::Backlog);
        tasks.create(task.clone()).await.unwrap();

        let done = service.set_status(task.id, TaskStatus::Done).await.unwrap();
        assert_eq!(done.status, TaskStatus::Done);
        // Done is not a dead end: the card can be pulled back to the board.
        let reopened = service
            .set_status(task.id, TaskStatus::Backlog)
            .await
            .unwrap();
        assert_eq!(reopened.status, TaskStatus::Backlog);
    }

    #[tokio::test]
    async fn setting_the_column_a_task_already_has_is_a_noop_without_a_write() {
        let (tasks, service) = setup();
        let task = test_task(TaskStatus::InProgress);
        tasks.create(task.clone()).await.unwrap();
        let original_updated_at = task.updated_at;

        let unchanged = service
            .set_status(task.id, TaskStatus::InProgress)
            .await
            .unwrap();

        assert_eq!(unchanged.status, TaskStatus::InProgress);
        assert_eq!(unchanged.updated_at, original_updated_at);
        assert_eq!(tasks.writes(), 0, "a no-op must not write");
    }

    #[tokio::test]
    async fn a_real_move_writes_once_and_bumps_updated_at() {
        let (tasks, service) = setup();
        let task = test_task(TaskStatus::Backlog);
        tasks.create(task.clone()).await.unwrap();
        let original_updated_at = task.updated_at;

        let moved = service.set_status(task.id, TaskStatus::ToDo).await.unwrap();

        assert_eq!(moved.status, TaskStatus::ToDo);
        assert_ne!(moved.updated_at, original_updated_at);
        assert_eq!(tasks.writes(), 1);
    }

    #[tokio::test]
    async fn unknown_task_id_is_a_not_found() {
        let (_tasks, service) = setup();

        let result = service.set_status(TaskId::new(), TaskStatus::Done).await;
        assert!(matches!(result, Err(TaskStatusError::NotFound)));
    }

    #[tokio::test]
    async fn a_repository_failure_surfaces_as_the_typed_error() {
        let (tasks, service) = setup();
        tasks.set_fail_set_status(true);
        let task = test_task(TaskStatus::Backlog);
        tasks.create(task.clone()).await.unwrap();

        let error = service
            .set_status(task.id, TaskStatus::Done)
            .await
            .expect_err("the faked write failure");
        assert!(matches!(error, TaskStatusError::Repository(_)));
    }

    #[tokio::test]
    async fn a_task_that_is_blocked_can_still_be_moved() {
        // The service takes no notion of blocking (roadmap 3.7, D6): a card is
        // moved by its column alone, so one whose blocker is not Done moves
        // just like any other. The end-to-end proof — with a real `blocks`
        // relation in the database — lives in the handler tests.
        let (tasks, service) = setup();
        let blocked = test_task(TaskStatus::ToDo);
        tasks.create(blocked.clone()).await.unwrap();

        let moved = service
            .set_status(blocked.id, TaskStatus::Done)
            .await
            .unwrap();
        assert_eq!(moved.status, TaskStatus::Done);
    }
}
