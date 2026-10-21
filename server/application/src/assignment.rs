//! Assigning tasks to people, and goals and milestones to owners (roadmap
//! 3.11, D17).
//!
//! The rules live here, not in the handler: the target entity is checked
//! before the user, so an unknown task is a `NotFound` even when the user id
//! is also unknown; a user that does not exist is a `UserNotFound`, and one
//! that is deactivated is a `UserDeactivated`. Setting the value already set,
//! or clearing a value that is not set, is an idempotent no-op that returns
//! the entity without writing, so its `updated_at` is left alone. Clearing
//! never looks the user up: it works even when the current assignee or owner
//! has since been deactivated. The writes are single-column (see
//! [`TaskRepository::set_assignee`] and the goal/milestone `set_owner`
//! ports), so they cannot clobber a field another writer changed in between.
//! No snapshot trigger, no audit (D5, D9).

use std::sync::Arc;

use domain::{Goal, GoalId, Milestone, MilestoneId, Task, TaskId, UserId};

use crate::ports::{
    GoalRepository, MilestoneRepository, RepositoryError, TaskRepository, UserRepository,
};

/// An error from assigning a task or setting an owner.
#[derive(Debug)]
pub enum AssignmentError {
    /// No task, goal, or milestone with the given id exists.
    NotFound,
    /// No user with the given id exists.
    UserNotFound,
    /// The user exists but is deactivated.
    UserDeactivated,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for AssignmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssignmentError::NotFound => write!(f, "requested resource was not found"),
            AssignmentError::UserNotFound => write!(f, "no user with the given id exists"),
            AssignmentError::UserDeactivated => write!(f, "the user is deactivated"),
            AssignmentError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AssignmentError {}

/// Assigns tasks to people and goals and milestones to owners (roadmap 3.11,
/// D17).
pub struct AssignmentService {
    tasks: Arc<dyn TaskRepository>,
    goals: Arc<dyn GoalRepository>,
    milestones: Arc<dyn MilestoneRepository>,
    users: Arc<dyn UserRepository>,
}

impl AssignmentService {
    pub fn new(
        tasks: Arc<dyn TaskRepository>,
        goals: Arc<dyn GoalRepository>,
        milestones: Arc<dyn MilestoneRepository>,
        users: Arc<dyn UserRepository>,
    ) -> Self {
        Self {
            tasks,
            goals,
            milestones,
            users,
        }
    }

    /// Assign the task to `assignee_id`. Setting the user it is already
    /// assigned to is a no-op that returns the task without writing.
    pub async fn set_task_assignee(
        &self,
        id: TaskId,
        assignee_id: UserId,
    ) -> Result<Task, AssignmentError> {
        let existing = self.find_task(id).await?;
        if existing.assignee_id == Some(assignee_id) {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.ensure_active_user(assignee_id).await?;
        self.tasks
            .set_assignee(id, Some(assignee_id))
            .await
            .map_err(AssignmentError::Repository)
    }

    /// Remove the task's assignee. No user lookup: it works even when the
    /// current assignee has since been deactivated. Clearing a task with no
    /// assignee is a no-op that returns the task without writing.
    pub async fn clear_task_assignee(&self, id: TaskId) -> Result<Task, AssignmentError> {
        let existing = self.find_task(id).await?;
        if existing.assignee_id.is_none() {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.tasks
            .set_assignee(id, None)
            .await
            .map_err(AssignmentError::Repository)
    }

    /// Set the goal's owner to `owner_id`. Setting the user it already has is
    /// a no-op that returns the goal without writing.
    pub async fn set_goal_owner(
        &self,
        id: GoalId,
        owner_id: UserId,
    ) -> Result<Goal, AssignmentError> {
        let existing = self.find_goal(id).await?;
        if existing.owner_id == Some(owner_id) {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.ensure_active_user(owner_id).await?;
        self.goals
            .set_owner(id, Some(owner_id))
            .await
            .map_err(AssignmentError::Repository)
    }

    /// Remove the goal's owner. No user lookup: it works even when the current
    /// owner has since been deactivated. Clearing a goal with no owner is a
    /// no-op that returns the goal without writing.
    pub async fn clear_goal_owner(&self, id: GoalId) -> Result<Goal, AssignmentError> {
        let existing = self.find_goal(id).await?;
        if existing.owner_id.is_none() {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.goals
            .set_owner(id, None)
            .await
            .map_err(AssignmentError::Repository)
    }

    /// Set the milestone's owner to `owner_id`. Setting the user it already
    /// has is a no-op that returns the milestone without writing.
    pub async fn set_milestone_owner(
        &self,
        id: MilestoneId,
        owner_id: UserId,
    ) -> Result<Milestone, AssignmentError> {
        let existing = self.find_milestone(id).await?;
        if existing.owner_id == Some(owner_id) {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.ensure_active_user(owner_id).await?;
        self.milestones
            .set_owner(id, Some(owner_id))
            .await
            .map_err(AssignmentError::Repository)
    }

    /// Remove the milestone's owner. No user lookup: it works even when the
    /// current owner has since been deactivated. Clearing a milestone with no
    /// owner is a no-op that returns the milestone without writing.
    pub async fn clear_milestone_owner(
        &self,
        id: MilestoneId,
    ) -> Result<Milestone, AssignmentError> {
        let existing = self.find_milestone(id).await?;
        if existing.owner_id.is_none() {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(existing);
        }
        self.milestones
            .set_owner(id, None)
            .await
            .map_err(AssignmentError::Repository)
    }

    async fn find_task(&self, id: TaskId) -> Result<Task, AssignmentError> {
        self.tasks
            .find_by_id(id)
            .await
            .map_err(AssignmentError::Repository)?
            .ok_or(AssignmentError::NotFound)
    }

    async fn find_goal(&self, id: GoalId) -> Result<Goal, AssignmentError> {
        self.goals
            .find_by_id(id)
            .await
            .map_err(AssignmentError::Repository)?
            .ok_or(AssignmentError::NotFound)
    }

    async fn find_milestone(&self, id: MilestoneId) -> Result<Milestone, AssignmentError> {
        self.milestones
            .find_by_id(id)
            .await
            .map_err(AssignmentError::Repository)?
            .ok_or(AssignmentError::NotFound)
    }

    /// The user must exist and be active before an assignment is written.
    /// Public so task creation can validate its `assignee_id` with exactly
    /// the same check as the set operations.
    pub async fn ensure_active_user(&self, id: UserId) -> Result<(), AssignmentError> {
        match self
            .users
            .find_by_id(id)
            .await
            .map_err(AssignmentError::Repository)?
        {
            Some(user) if user.is_active() => Ok(()),
            Some(_) => Err(AssignmentError::UserDeactivated),
            None => Err(AssignmentError::UserNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use chrono::Utc;
    use domain::{Role, Status, TaskStatus, User};

    use crate::auth::fakes::{FailingUserRepository, InMemoryUserRepository};
    use crate::pagination::{Page, PageRequest};
    use crate::ports::goal_repository::fakes::{apply_goal_list_filter, page_goals};
    use crate::ports::milestone_repository::fakes::{apply_milestone_list_filter, page_milestones};
    use crate::ports::task_repository::fakes::{apply_task_list_filter, page_tasks};
    use crate::ports::{
        AccessChange, GoalListFilter, MilestoneListFilter, TaskCounts, TaskListFilter,
        UserRepository,
    };

    use super::*;

    fn test_task() -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            milestone_id: None,
            title: "Test task".to_owned(),
            description: None,
            status: TaskStatus::Backlog,
            target_date: None,
            assignee_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_goal() -> Goal {
        let now = Utc::now();
        Goal {
            id: GoalId::new(),
            title: "Test goal".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_milestone() -> Milestone {
        let now = Utc::now();
        Milestone {
            id: MilestoneId::new(),
            title: "Test milestone".to_owned(),
            description: None,
            status: Status::OnTrack,
            status_override: None,
            target_date: None,
            owner_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_user(name: &str) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: format!("{name}@example.com"),
            password_hash: None,
            display_name: name.to_owned(),
            role: Role::Staff,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// An in-memory [`TaskRepository`] mirroring Postgres: `set_assignee`
    /// writes only the column and bumps `updated_at`, answers `NotFound` for
    /// an unknown id, and fails on demand. It counts real writes so a test can
    /// prove a no-op wrote nothing.
    #[derive(Default)]
    struct InMemoryTaskRepository {
        tasks: Mutex<HashMap<TaskId, Task>>,
        fail_set_assignee: AtomicBool,
        set_assignee_writes: AtomicUsize,
    }

    impl InMemoryTaskRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_assignee` fail (or succeed again).
        fn set_fail_set_assignee(&self, fail: bool) {
            self.fail_set_assignee.store(fail, Ordering::Relaxed);
        }

        /// How many times `set_assignee` actually wrote.
        fn writes(&self) -> usize {
            self.set_assignee_writes.load(Ordering::Relaxed)
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

        async fn task_counts_for_milestones(
            &self,
            milestone_ids: &[MilestoneId],
        ) -> Result<HashMap<MilestoneId, TaskCounts>, RepositoryError> {
            let tasks = self.locked();
            // Every requested id comes back, with zeros when the milestone
            // has no tasks (mirroring the Postgres implementation).
            let mut counts: HashMap<MilestoneId, TaskCounts> = milestone_ids
                .iter()
                .map(|id| (*id, TaskCounts { total: 0, done: 0 }))
                .collect();
            for task in tasks.values() {
                let Some(milestone_id) = task.milestone_id else {
                    continue;
                };
                let Some(counts) = counts.get_mut(&milestone_id) else {
                    continue;
                };
                counts.total += 1;
                if task.status == TaskStatus::Done {
                    counts.done += 1;
                }
            }
            Ok(counts)
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
            let mut tasks = self.locked();
            let Some(task) = tasks.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            task.status = status;
            task.updated_at = Utc::now();
            Ok(task.clone())
        }

        async fn set_assignee(
            &self,
            id: TaskId,
            assignee_id: Option<UserId>,
        ) -> Result<Task, RepositoryError> {
            if self.fail_set_assignee.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected(
                    "faking an assignee write failure".to_owned(),
                ));
            }
            let mut tasks = self.locked();
            let Some(task) = tasks.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the column and the
            // updated_at timestamp.
            task.assignee_id = assignee_id;
            task.updated_at = Utc::now();
            self.set_assignee_writes.fetch_add(1, Ordering::Relaxed);
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
            filter: &TaskListFilter,
            page: &PageRequest,
        ) -> Result<Page<Task>, RepositoryError> {
            let matched = apply_task_list_filter(self.locked().values(), filter);
            Ok(page_tasks(matched, page))
        }
    }

    /// An in-memory [`GoalRepository`] mirroring Postgres: `set_owner` writes
    /// only the column and bumps `updated_at`, answers `NotFound` for an
    /// unknown id, and fails on demand. It counts real writes so a test can
    /// prove a no-op wrote nothing.
    #[derive(Default)]
    struct InMemoryGoalRepository {
        goals: Mutex<HashMap<GoalId, Goal>>,
        fail_set_owner: AtomicBool,
        set_owner_writes: AtomicUsize,
    }

    impl InMemoryGoalRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_owner` fail (or succeed again).
        fn set_fail_set_owner(&self, fail: bool) {
            self.fail_set_owner.store(fail, Ordering::Relaxed);
        }

        /// How many times `set_owner` actually wrote.
        fn writes(&self) -> usize {
            self.set_owner_writes.load(Ordering::Relaxed)
        }

        fn locked(&self) -> MutexGuard<'_, HashMap<GoalId, Goal>> {
            self.goals.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl GoalRepository for InMemoryGoalRepository {
        async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            self.locked().insert(goal.id, goal.clone());
            Ok(goal)
        }

        async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError> {
            Ok(self.locked().get(&id).cloned())
        }

        async fn list(&self) -> Result<Vec<Goal>, RepositoryError> {
            Ok(self.locked().values().cloned().collect())
        }

        async fn list_all_ids(&self) -> Result<Vec<GoalId>, RepositoryError> {
            let mut ids: Vec<GoalId> = self.locked().keys().copied().collect();
            ids.sort_by_key(|id| id.0);
            Ok(ids)
        }

        async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError> {
            let mut goals = self.locked();
            let Some(existing) = goals.get(&goal.id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: an ordinary update never writes status_override
            // (roadmap 3.2) or owner_id (D17).
            let updated = Goal {
                status_override: existing.status_override,
                owner_id: existing.owner_id,
                ..goal
            };
            goals.insert(updated.id, updated.clone());
            Ok(updated)
        }

        async fn set_status_override(
            &self,
            id: GoalId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut goals = self.locked();
            let Some(goal) = goals.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            goal.status_override = status_override;
            Ok(())
        }

        async fn set_automatic_status(
            &self,
            id: GoalId,
            status: Status,
        ) -> Result<bool, RepositoryError> {
            let mut goals = self.locked();
            let Some(goal) = goals.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the automatic status;
            // updated_at and the override are untouched, and an unchanged
            // value writes nothing.
            if goal.status == status {
                return Ok(false);
            }
            goal.status = status;
            Ok(true)
        }

        async fn set_owner(
            &self,
            id: GoalId,
            owner_id: Option<UserId>,
        ) -> Result<Goal, RepositoryError> {
            if self.fail_set_owner.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected(
                    "faking an owner write failure".to_owned(),
                ));
            }
            let mut goals = self.locked();
            let Some(goal) = goals.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the column and the
            // updated_at timestamp.
            goal.owner_id = owner_id;
            goal.updated_at = Utc::now();
            self.set_owner_writes.fetch_add(1, Ordering::Relaxed);
            Ok(goal.clone())
        }

        async fn delete(&self, id: GoalId) -> Result<(), RepositoryError> {
            if self.locked().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &GoalListFilter,
            page: &PageRequest,
        ) -> Result<Page<Goal>, RepositoryError> {
            let matched = apply_goal_list_filter(self.locked().values(), filter);
            Ok(page_goals(matched, page))
        }
    }

    /// An in-memory [`MilestoneRepository`] mirroring Postgres: `set_owner`
    /// writes only the column and bumps `updated_at`, answers `NotFound` for
    /// an unknown id, and fails on demand. It counts real writes so a test can
    /// prove a no-op wrote nothing.
    #[derive(Default)]
    struct InMemoryMilestoneRepository {
        milestones: Mutex<HashMap<MilestoneId, Milestone>>,
        fail_set_owner: AtomicBool,
        set_owner_writes: AtomicUsize,
    }

    impl InMemoryMilestoneRepository {
        fn new() -> Self {
            Self::default()
        }

        /// Make `set_owner` fail (or succeed again).
        fn set_fail_set_owner(&self, fail: bool) {
            self.fail_set_owner.store(fail, Ordering::Relaxed);
        }

        /// How many times `set_owner` actually wrote.
        fn writes(&self) -> usize {
            self.set_owner_writes.load(Ordering::Relaxed)
        }

        fn locked(&self) -> MutexGuard<'_, HashMap<MilestoneId, Milestone>> {
            self.milestones.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl MilestoneRepository for InMemoryMilestoneRepository {
        async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            self.locked().insert(milestone.id, milestone.clone());
            Ok(milestone)
        }

        async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError> {
            Ok(self.locked().get(&id).cloned())
        }

        async fn find_by_ids(
            &self,
            ids: &[MilestoneId],
        ) -> Result<Vec<Milestone>, RepositoryError> {
            let milestones = self.locked();
            let mut found: Vec<Milestone> = ids
                .iter()
                .filter_map(|id| milestones.get(id).cloned())
                .collect();
            found.sort_by_key(|milestone| milestone.id.0);
            Ok(found)
        }

        async fn list(&self) -> Result<Vec<Milestone>, RepositoryError> {
            Ok(self.locked().values().cloned().collect())
        }

        async fn list_all_ids(&self) -> Result<Vec<MilestoneId>, RepositoryError> {
            let mut ids: Vec<MilestoneId> = self.locked().keys().copied().collect();
            ids.sort_by_key(|id| id.0);
            Ok(ids)
        }

        async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
            let mut milestones = self.locked();
            let Some(existing) = milestones.get(&milestone.id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: an ordinary update never writes status_override
            // (roadmap 3.2) or owner_id (D17).
            let updated = Milestone {
                status_override: existing.status_override,
                owner_id: existing.owner_id,
                ..milestone
            };
            milestones.insert(updated.id, updated.clone());
            Ok(updated)
        }

        async fn set_status_override(
            &self,
            id: MilestoneId,
            status_override: Option<Status>,
        ) -> Result<(), RepositoryError> {
            let mut milestones = self.locked();
            let Some(milestone) = milestones.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            milestone.status_override = status_override;
            Ok(())
        }

        async fn set_automatic_status(
            &self,
            id: MilestoneId,
            status: Status,
        ) -> Result<bool, RepositoryError> {
            let mut milestones = self.locked();
            let Some(milestone) = milestones.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the automatic status;
            // updated_at and the override are untouched, and an unchanged
            // value writes nothing.
            if milestone.status == status {
                return Ok(false);
            }
            milestone.status = status;
            Ok(true)
        }

        async fn set_owner(
            &self,
            id: MilestoneId,
            owner_id: Option<UserId>,
        ) -> Result<Milestone, RepositoryError> {
            if self.fail_set_owner.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected(
                    "faking an owner write failure".to_owned(),
                ));
            }
            let mut milestones = self.locked();
            let Some(milestone) = milestones.get_mut(&id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: the write touches only the column and the
            // updated_at timestamp.
            milestone.owner_id = owner_id;
            milestone.updated_at = Utc::now();
            self.set_owner_writes.fetch_add(1, Ordering::Relaxed);
            Ok(milestone.clone())
        }

        async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError> {
            if self.locked().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_page(
            &self,
            filter: &MilestoneListFilter,
            page: &PageRequest,
        ) -> Result<Page<Milestone>, RepositoryError> {
            let matched = apply_milestone_list_filter(self.locked().values(), filter);
            Ok(page_milestones(matched, page))
        }
    }

    fn setup() -> (
        Arc<InMemoryTaskRepository>,
        Arc<InMemoryGoalRepository>,
        Arc<InMemoryMilestoneRepository>,
        Arc<InMemoryUserRepository>,
        AssignmentService,
    ) {
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let users = Arc::new(InMemoryUserRepository::new());
        let service = AssignmentService::new(
            tasks.clone(),
            goals.clone(),
            milestones.clone(),
            users.clone(),
        );
        (tasks, goals, milestones, users, service)
    }

    // --- Tasks ---------------------------------------------------------

    #[tokio::test]
    async fn setting_an_assignee_writes_the_user_id() {
        let (tasks, _goals, _milestones, users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        let user = test_user("assignee");
        users.create(user.clone()).await.unwrap();

        let updated = service.set_task_assignee(task.id, user.id).await.unwrap();

        assert_eq!(updated.assignee_id, Some(user.id));
        assert_ne!(updated.updated_at, task.updated_at);
        assert_eq!(tasks.writes(), 1);
        assert_eq!(
            tasks
                .find_by_id(task.id)
                .await
                .unwrap()
                .unwrap()
                .assignee_id,
            Some(user.id)
        );
    }

    #[tokio::test]
    async fn setting_twice_and_clearing_twice_write_once() {
        let (tasks, _goals, _milestones, users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        let user = test_user("assignee");
        users.create(user.clone()).await.unwrap();

        let first = service.set_task_assignee(task.id, user.id).await.unwrap();
        let repeated = service.set_task_assignee(task.id, user.id).await.unwrap();
        assert_eq!(repeated.updated_at, first.updated_at);
        assert_eq!(tasks.writes(), 1, "the second set must not write");

        let cleared = service.clear_task_assignee(task.id).await.unwrap();
        assert_eq!(cleared.assignee_id, None);
        assert_ne!(cleared.updated_at, first.updated_at);
        assert_eq!(tasks.writes(), 2);

        let cleared_again = service.clear_task_assignee(task.id).await.unwrap();
        assert_eq!(cleared_again.updated_at, cleared.updated_at);
        assert_eq!(tasks.writes(), 2, "the second clear must not write");
    }

    #[tokio::test]
    async fn an_unknown_task_is_a_not_found_for_set_and_clear() {
        let (_tasks, _goals, _milestones, users, service) = setup();
        let user = test_user("assignee");
        users.create(user.clone()).await.unwrap();

        let set = service.set_task_assignee(TaskId::new(), user.id).await;
        assert!(matches!(set, Err(AssignmentError::NotFound)));
        let clear = service.clear_task_assignee(TaskId::new()).await;
        assert!(matches!(clear, Err(AssignmentError::NotFound)));
    }

    #[tokio::test]
    async fn setting_an_unknown_user_is_a_user_not_found() {
        let (tasks, _goals, _milestones, _users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();

        let result = service.set_task_assignee(task.id, UserId::new()).await;
        assert!(matches!(result, Err(AssignmentError::UserNotFound)));
        assert_eq!(
            tasks
                .find_by_id(task.id)
                .await
                .unwrap()
                .unwrap()
                .assignee_id,
            None
        );
    }

    #[tokio::test]
    async fn setting_a_deactivated_user_is_rejected_and_the_task_is_unchanged() {
        let (tasks, _goals, _milestones, users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let result = service.set_task_assignee(task.id, user.id).await;
        assert!(matches!(result, Err(AssignmentError::UserDeactivated)));
        assert_eq!(
            tasks
                .find_by_id(task.id)
                .await
                .unwrap()
                .unwrap()
                .assignee_id,
            None
        );
    }

    #[tokio::test]
    async fn clearing_works_when_the_current_assignee_is_deactivated() {
        let (tasks, _goals, _milestones, users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        service.set_task_assignee(task.id, user.id).await.unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let cleared = service.clear_task_assignee(task.id).await.unwrap();

        assert_eq!(cleared.assignee_id, None);
        assert_eq!(tasks.writes(), 2);
    }

    #[tokio::test]
    async fn a_failed_assignee_write_surfaces_as_the_typed_error() {
        let (tasks, _goals, _milestones, users, service) = setup();
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        let user = test_user("assignee");
        users.create(user.clone()).await.unwrap();
        tasks.set_fail_set_assignee(true);

        let error = service
            .set_task_assignee(task.id, user.id)
            .await
            .expect_err("the faked write failure");
        assert!(matches!(error, AssignmentError::Repository(_)));
    }

    #[tokio::test]
    async fn a_failed_user_lookup_surfaces_as_the_typed_error() {
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let goals = Arc::new(InMemoryGoalRepository::new());
        let milestones = Arc::new(InMemoryMilestoneRepository::new());
        let users: Arc<dyn UserRepository> = Arc::new(FailingUserRepository);
        let service = AssignmentService::new(tasks.clone(), goals, milestones, users);
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();

        let error = service
            .set_task_assignee(task.id, UserId::new())
            .await
            .expect_err("the faked lookup failure");
        assert!(matches!(error, AssignmentError::Repository(_)));
    }

    // --- Goals ---------------------------------------------------------

    #[tokio::test]
    async fn setting_an_owner_writes_the_user_id() {
        let (_tasks, goals, _milestones, users, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let updated = service.set_goal_owner(goal.id, user.id).await.unwrap();

        assert_eq!(updated.owner_id, Some(user.id));
        assert_ne!(updated.updated_at, goal.updated_at);
        assert_eq!(goals.writes(), 1);
        assert_eq!(
            goals.find_by_id(goal.id).await.unwrap().unwrap().owner_id,
            Some(user.id)
        );
    }

    #[tokio::test]
    async fn setting_an_owner_twice_and_clearing_twice_write_once() {
        let (_tasks, goals, _milestones, users, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let first = service.set_goal_owner(goal.id, user.id).await.unwrap();
        let repeated = service.set_goal_owner(goal.id, user.id).await.unwrap();
        assert_eq!(repeated.updated_at, first.updated_at);
        assert_eq!(goals.writes(), 1, "the second set must not write");

        let cleared = service.clear_goal_owner(goal.id).await.unwrap();
        assert_eq!(cleared.owner_id, None);
        assert_ne!(cleared.updated_at, first.updated_at);
        assert_eq!(goals.writes(), 2);

        let cleared_again = service.clear_goal_owner(goal.id).await.unwrap();
        assert_eq!(cleared_again.updated_at, cleared.updated_at);
        assert_eq!(goals.writes(), 2, "the second clear must not write");
    }

    #[tokio::test]
    async fn an_unknown_goal_is_a_not_found_for_set_and_clear() {
        let (_tasks, _goals, _milestones, users, service) = setup();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let set = service.set_goal_owner(GoalId::new(), user.id).await;
        assert!(matches!(set, Err(AssignmentError::NotFound)));
        let clear = service.clear_goal_owner(GoalId::new()).await;
        assert!(matches!(clear, Err(AssignmentError::NotFound)));
    }

    #[tokio::test]
    async fn setting_a_deactivated_owner_is_rejected_and_the_goal_is_unchanged() {
        let (_tasks, goals, _milestones, users, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let result = service.set_goal_owner(goal.id, user.id).await;
        assert!(matches!(result, Err(AssignmentError::UserDeactivated)));
        assert_eq!(
            goals.find_by_id(goal.id).await.unwrap().unwrap().owner_id,
            None
        );
    }

    #[tokio::test]
    async fn clearing_works_when_the_current_owner_is_deactivated() {
        let (_tasks, goals, _milestones, users, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        service.set_goal_owner(goal.id, user.id).await.unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let cleared = service.clear_goal_owner(goal.id).await.unwrap();

        assert_eq!(cleared.owner_id, None);
        assert_eq!(goals.writes(), 2);
    }

    #[tokio::test]
    async fn a_failed_owner_write_surfaces_as_the_typed_error() {
        let (_tasks, goals, _milestones, users, service) = setup();
        let goal = test_goal();
        goals.create(goal.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();
        goals.set_fail_set_owner(true);

        let error = service
            .set_goal_owner(goal.id, user.id)
            .await
            .expect_err("the faked write failure");
        assert!(matches!(error, AssignmentError::Repository(_)));
    }

    // --- Milestones ----------------------------------------------------

    #[tokio::test]
    async fn setting_a_milestone_owner_writes_the_user_id() {
        let (_tasks, _goals, milestones, users, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let updated = service
            .set_milestone_owner(milestone.id, user.id)
            .await
            .unwrap();

        assert_eq!(updated.owner_id, Some(user.id));
        assert_ne!(updated.updated_at, milestone.updated_at);
        assert_eq!(milestones.writes(), 1);
        assert_eq!(
            milestones
                .find_by_id(milestone.id)
                .await
                .unwrap()
                .unwrap()
                .owner_id,
            Some(user.id)
        );
    }

    #[tokio::test]
    async fn setting_a_milestone_owner_twice_and_clearing_twice_write_once() {
        let (_tasks, _goals, milestones, users, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let first = service
            .set_milestone_owner(milestone.id, user.id)
            .await
            .unwrap();
        let repeated = service
            .set_milestone_owner(milestone.id, user.id)
            .await
            .unwrap();
        assert_eq!(repeated.updated_at, first.updated_at);
        assert_eq!(milestones.writes(), 1, "the second set must not write");

        let cleared = service.clear_milestone_owner(milestone.id).await.unwrap();
        assert_eq!(cleared.owner_id, None);
        assert_ne!(cleared.updated_at, first.updated_at);
        assert_eq!(milestones.writes(), 2);

        let cleared_again = service.clear_milestone_owner(milestone.id).await.unwrap();
        assert_eq!(cleared_again.updated_at, cleared.updated_at);
        assert_eq!(milestones.writes(), 2, "the second clear must not write");
    }

    #[tokio::test]
    async fn an_unknown_milestone_is_a_not_found_for_set_and_clear() {
        let (_tasks, _goals, _milestones, users, service) = setup();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();

        let set = service
            .set_milestone_owner(MilestoneId::new(), user.id)
            .await;
        assert!(matches!(set, Err(AssignmentError::NotFound)));
        let clear = service.clear_milestone_owner(MilestoneId::new()).await;
        assert!(matches!(clear, Err(AssignmentError::NotFound)));
    }

    #[tokio::test]
    async fn setting_a_deactivated_milestone_owner_is_rejected_and_the_milestone_is_unchanged() {
        let (_tasks, _goals, milestones, users, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let result = service.set_milestone_owner(milestone.id, user.id).await;
        assert!(matches!(result, Err(AssignmentError::UserDeactivated)));
        assert_eq!(
            milestones
                .find_by_id(milestone.id)
                .await
                .unwrap()
                .unwrap()
                .owner_id,
            None
        );
    }

    #[tokio::test]
    async fn clearing_works_when_the_current_milestone_owner_is_deactivated() {
        let (_tasks, _goals, milestones, users, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let user = test_user("departed");
        users.create(user.clone()).await.unwrap();
        service
            .set_milestone_owner(milestone.id, user.id)
            .await
            .unwrap();
        users
            .apply_access_change(user.id, AccessChange::Deactivate)
            .await
            .unwrap();

        let cleared = service.clear_milestone_owner(milestone.id).await.unwrap();

        assert_eq!(cleared.owner_id, None);
        assert_eq!(milestones.writes(), 2);
    }

    #[tokio::test]
    async fn a_failed_milestone_owner_write_surfaces_as_the_typed_error() {
        let (_tasks, _goals, milestones, users, service) = setup();
        let milestone = test_milestone();
        milestones.create(milestone.clone()).await.unwrap();
        let user = test_user("owner");
        users.create(user.clone()).await.unwrap();
        milestones.set_fail_set_owner(true);

        let error = service
            .set_milestone_owner(milestone.id, user.id)
            .await
            .expect_err("the faked write failure");
        assert!(matches!(error, AssignmentError::Repository(_)));
    }
}
