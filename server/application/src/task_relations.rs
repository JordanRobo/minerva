//! Task relations (roadmap 3.6, D4): creating a relation between two tasks,
//! deleting it again, and listing everything a task is connected to.
//!
//! The rules live here, not in the handlers: a self-relation is rejected
//! before any lookup, both tasks must exist before a write (the path task is
//! checked first), the submitted type is normalised to one canonical row per
//! relationship via [`TaskRelation::canonical_form`], and the duplicate /
//! reverse rejections rest on the repository's unique indexes rather than a
//! service-level pre-check. Every result is reported from the path task's
//! perspective: a stored `blocks` row reads as `blocked_by` from its target,
//! and both ends of a `relates_to` row read the same (see
//! [`TaskRelation::as_seen_by`]). Nothing here touches statuses — relations
//! do not affect goal or milestone status in v1.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use domain::{Task, TaskId, TaskRelation, TaskRelationId, TaskRelationType, TaskStatus};

use crate::ports::{
    RepositoryError, TaskRelationCreateError, TaskRelationRepository, TaskRepository,
};

/// Which of the two tasks named in a `create` call was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingTask {
    /// The path task — the task the relation is being created from.
    Path,
    /// The related task — the one the relation points at.
    Related,
}

/// An error from creating, deleting or listing task relations.
#[derive(Debug)]
pub enum TaskRelationError {
    /// No task with the given id exists; which of the two was missing.
    TaskNotFound(MissingTask),
    /// No relation with the given id exists, or it does not involve the path
    /// task (reported the same way so its existence is not leaked).
    RelationNotFound,
    /// The relation links a task to itself.
    SelfRelation,
    /// A relation of the same type already exists between the two tasks.
    RelationExists,
    /// A `blocks` relation in the opposite direction already exists: the two
    /// tasks would block each other.
    ReverseRelationExists,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for TaskRelationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskRelationError::TaskNotFound(MissingTask::Path) => write!(f, "task was not found"),
            TaskRelationError::TaskNotFound(MissingTask::Related) => {
                write!(f, "related task was not found")
            }
            TaskRelationError::RelationNotFound => write!(f, "relation was not found"),
            TaskRelationError::SelfRelation => write!(f, "a task cannot relate to itself"),
            TaskRelationError::RelationExists => write!(f, "the relation already exists"),
            TaskRelationError::ReverseRelationExists => {
                write!(f, "a relation in the opposite direction already exists")
            }
            TaskRelationError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TaskRelationError {}

/// The minimum of a task needed to display it as the other end of a relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
}

/// One relation as seen from one of its endpoint tasks: the type from that
/// task's perspective plus a summary of the other task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRelationView {
    pub relation_id: TaskRelationId,
    /// The relation type read from the path task's perspective (a stored
    /// `blocks` row reads as `blocked_by` from its target).
    pub relation_type: TaskRelationType,
    pub related_task: TaskSummary,
    pub created_at: DateTime<Utc>,
}

/// Creates, deletes and lists task relations (roadmap 3.6, D4). Nothing here
/// touches statuses.
pub struct TaskRelationService {
    tasks: Arc<dyn TaskRepository>,
    relations: Arc<dyn TaskRelationRepository>,
}

impl TaskRelationService {
    pub fn new(tasks: Arc<dyn TaskRepository>, relations: Arc<dyn TaskRelationRepository>) -> Self {
        Self { tasks, relations }
    }

    /// Link `related_task_id` to `task_id` with the submitted relation type,
    /// returning the new relation as `task_id` sees it. A self-relation is
    /// rejected before any lookup; both tasks must exist (path task first).
    pub async fn create(
        &self,
        task_id: TaskId,
        submitted_type: TaskRelationType,
        related_task_id: TaskId,
    ) -> Result<TaskRelationView, TaskRelationError> {
        if task_id == related_task_id {
            return Err(TaskRelationError::SelfRelation);
        }
        self.require_task(task_id, MissingTask::Path).await?;
        let related = self
            .require_task(related_task_id, MissingTask::Related)
            .await?;
        // One canonical row per relationship (D4). The duplicate / reverse /
        // self invariants are enforced by the repository's unique indexes,
        // not a pre-check here, so concurrent racers are classified against
        // committed state.
        let (source, target, stored_type) =
            TaskRelation::canonical_form(task_id, related_task_id, submitted_type);
        let relation = TaskRelation::new(source, target, stored_type, Utc::now());
        let stored = self
            .relations
            .create(relation)
            .await
            .map_err(map_create_error)?;
        // `task_id` is always an endpoint of its own canonical row.
        let (seen_type, _) = stored
            .as_seen_by(task_id)
            .expect("the path task is an endpoint of its own canonical relation");
        Ok(TaskRelationView {
            relation_id: stored.id,
            relation_type: seen_type,
            related_task: TaskSummary {
                id: related.id,
                title: related.title,
                status: related.status,
            },
            created_at: stored.created_at,
        })
    }

    /// Remove the relation. The path task must exist, and the relation must
    /// exist *and* involve it; a relation that exists for other tasks is
    /// reported the same way as a missing one.
    pub async fn delete(
        &self,
        task_id: TaskId,
        relation_id: TaskRelationId,
    ) -> Result<(), TaskRelationError> {
        self.require_task(task_id, MissingTask::Path).await?;
        let Some(relation) = self
            .relations
            .find_by_id(relation_id)
            .await
            .map_err(TaskRelationError::Repository)?
        else {
            return Err(TaskRelationError::RelationNotFound);
        };
        if relation.as_seen_by(task_id).is_none() {
            return Err(TaskRelationError::RelationNotFound);
        }
        let removed = self
            .relations
            .delete(relation_id)
            .await
            .map_err(TaskRelationError::Repository)?;
        if !removed {
            return Err(TaskRelationError::RelationNotFound);
        }
        Ok(())
    }

    /// Every relation in which `task_id` participates, from its perspective,
    /// in the repository's deterministic order (created_at, then id). Related
    /// task summaries are fetched in one batched lookup, not one query per
    /// row. An unknown task is a typed not-found.
    pub async fn list_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskRelationView>, TaskRelationError> {
        self.require_task(task_id, MissingTask::Path).await?;
        let relations = self
            .relations
            .list_for_task(task_id)
            .await
            .map_err(TaskRelationError::Repository)?;
        let related_ids: Vec<TaskId> = relations
            .iter()
            .filter_map(|relation| relation.as_seen_by(task_id).map(|(_, other)| other))
            .collect();
        let summaries = self.task_summaries(&related_ids).await?;
        Ok(relations
            .into_iter()
            .filter_map(|relation| {
                let (seen_type, other_id) = relation.as_seen_by(task_id)?;
                let related_task = summaries.get(&other_id)?.clone();
                Some(TaskRelationView {
                    relation_id: relation.id,
                    relation_type: seen_type,
                    related_task,
                    created_at: relation.created_at,
                })
            })
            .collect())
    }

    /// The path task must exist; which id was missing is reported by `which`.
    async fn require_task(
        &self,
        id: TaskId,
        which: MissingTask,
    ) -> Result<Task, TaskRelationError> {
        match self
            .tasks
            .find_by_id(id)
            .await
            .map_err(TaskRelationError::Repository)?
        {
            Some(task) => Ok(task),
            None => Err(TaskRelationError::TaskNotFound(which)),
        }
    }

    /// Summaries for a batch of task ids in a single lookup.
    async fn task_summaries(
        &self,
        task_ids: &[TaskId],
    ) -> Result<HashMap<TaskId, TaskSummary>, TaskRelationError> {
        let tasks = self
            .tasks
            .find_by_ids(task_ids)
            .await
            .map_err(TaskRelationError::Repository)?;
        Ok(tasks
            .into_iter()
            .map(|task| {
                (
                    task.id,
                    TaskSummary {
                        id: task.id,
                        title: task.title,
                        status: task.status,
                    },
                )
            })
            .collect())
    }
}

/// The repository's typed create errors map one-to-one onto the request-level
/// ones; the storage invariants they encode are unchanged.
fn map_create_error(error: TaskRelationCreateError) -> TaskRelationError {
    match error {
        TaskRelationCreateError::SelfRelation => TaskRelationError::SelfRelation,
        TaskRelationCreateError::Duplicate => TaskRelationError::RelationExists,
        TaskRelationCreateError::ReverseExists => TaskRelationError::ReverseRelationExists,
        TaskRelationCreateError::Repository(err) => TaskRelationError::Repository(err),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, NaiveDate};

    use super::*;
    use crate::ports::task_relation_repository::fakes::InMemoryTaskRelationRepository;

    /// A fixed timestamp in 2026, exact to the second.
    fn at(month: u32, day: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, month, day)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    fn test_task(title: &str) -> Task {
        let now = at(1, 1);
        Task {
            id: TaskId::new(),
            milestone_id: None,
            title: title.to_owned(),
            description: None,
            status: TaskStatus::ToDo,
            target_date: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// An in-memory [`TaskRepository`] that counts lookups, so a test can
    /// assert the service batches related-task summaries instead of issuing
    /// one query per row. `fail_find_by_id` forces the repository error path.
    #[derive(Default)]
    struct CountingTaskRepository {
        tasks: Mutex<HashMap<TaskId, Task>>,
        find_by_id_calls: AtomicUsize,
        find_by_ids_calls: AtomicUsize,
        fail_find_by_id: AtomicBool,
    }

    #[async_trait::async_trait]
    impl TaskRepository for CountingTaskRepository {
        async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
            self.tasks.lock().unwrap().insert(task.id, task.clone());
            Ok(task)
        }

        async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
            self.find_by_id_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_find_by_id.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected("injected failure".to_owned()));
            }
            Ok(self.tasks.lock().unwrap().get(&id).cloned())
        }

        async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
            self.find_by_ids_calls.fetch_add(1, Ordering::Relaxed);
            let tasks = self.tasks.lock().unwrap();
            Ok(ids.iter().filter_map(|id| tasks.get(id).cloned()).collect())
        }

        async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
            if self.tasks.lock().unwrap().contains_key(&task.id) {
                Ok(task)
            } else {
                Err(RepositoryError::NotFound)
            }
        }

        async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
            if self.tasks.lock().unwrap().remove(&id).is_none() {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }

        async fn list_by_milestone(
            &self,
            milestone_id: domain::MilestoneId,
        ) -> Result<Vec<Task>, RepositoryError> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .values()
                .filter(|t| t.milestone_id == Some(milestone_id))
                .cloned()
                .collect())
        }

        async fn list_unassigned(&self) -> Result<Vec<Task>, RepositoryError> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .values()
                .filter(|t| t.milestone_id.is_none())
                .cloned()
                .collect())
        }
    }

    /// A [`TaskRelationRepository`] whose every call fails, to prove the
    /// service surfaces repository errors as its typed error.
    struct FailingTaskRelationRepository;

    #[async_trait::async_trait]
    impl TaskRelationRepository for FailingTaskRelationRepository {
        async fn create(
            &self,
            _relation: TaskRelation,
        ) -> Result<TaskRelation, TaskRelationCreateError> {
            Err(TaskRelationCreateError::Repository(
                RepositoryError::Unexpected("injected failure".to_owned()),
            ))
        }

        async fn find_by_id(
            &self,
            _id: TaskRelationId,
        ) -> Result<Option<TaskRelation>, RepositoryError> {
            Err(RepositoryError::Unexpected("injected failure".to_owned()))
        }

        async fn delete(&self, _id: TaskRelationId) -> Result<bool, RepositoryError> {
            Err(RepositoryError::Unexpected("injected failure".to_owned()))
        }

        async fn list_for_task(
            &self,
            _task_id: TaskId,
        ) -> Result<Vec<TaskRelation>, RepositoryError> {
            Err(RepositoryError::Unexpected("injected failure".to_owned()))
        }
    }

    fn setup() -> (
        Arc<CountingTaskRepository>,
        Arc<InMemoryTaskRelationRepository>,
        TaskRelationService,
    ) {
        let tasks = Arc::new(CountingTaskRepository::default());
        let relations = Arc::new(InMemoryTaskRelationRepository::new());
        let service = TaskRelationService::new(tasks.clone(), relations.clone());
        (tasks, relations, service)
    }

    /// Seed a task into the repository and return it.
    async fn seed_task(tasks: &CountingTaskRepository, title: &str) -> Task {
        let task = test_task(title);
        tasks.create(task.clone()).await.unwrap();
        task
    }

    #[tokio::test]
    async fn blocks_create_stores_the_submitted_direction_and_reads_back_on_both_sides() {
        let (tasks, relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        let view = service
            .create(a.id, TaskRelationType::Blocks, b.id)
            .await
            .unwrap();

        // The stored row keeps the submitted direction.
        let stored = relations
            .find_by_id(view.relation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.source_task_id, a.id);
        assert_eq!(stored.target_task_id, b.id);
        assert_eq!(stored.relation_type, TaskRelationType::Blocks);
        // The view is from the path task's perspective.
        assert_eq!(view.relation_type, TaskRelationType::Blocks);
        assert_eq!(view.related_task.id, b.id);
        assert_eq!(view.related_task.title, "B");

        // The other end reads it as blocked_by.
        let from_b = service.list_for_task(b.id).await.unwrap();
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].relation_type, TaskRelationType::BlockedBy);
        assert_eq!(from_b[0].related_task.id, a.id);
    }

    #[tokio::test]
    async fn blocked_by_create_stores_a_blocks_row_with_swapped_endpoints() {
        let (tasks, relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        let view = service
            .create(a.id, TaskRelationType::BlockedBy, b.id)
            .await
            .unwrap();

        // "a is blocked by b" stores as "b blocks a".
        let stored = relations
            .find_by_id(view.relation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.source_task_id, b.id);
        assert_eq!(stored.target_task_id, a.id);
        assert_eq!(stored.relation_type, TaskRelationType::Blocks);
        // The submitter still sees blocked_by.
        assert_eq!(view.relation_type, TaskRelationType::BlockedBy);
        assert_eq!(view.related_task.id, b.id);

        // The other end reads it as blocks.
        let from_b = service.list_for_task(b.id).await.unwrap();
        assert_eq!(from_b[0].relation_type, TaskRelationType::Blocks);
        assert_eq!(from_b[0].related_task.id, a.id);
    }

    #[tokio::test]
    async fn relates_to_create_reads_the_same_from_both_ends() {
        let (tasks, relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        let view = service
            .create(a.id, TaskRelationType::RelatesTo, b.id)
            .await
            .unwrap();

        // The lower task id is stored in source.
        let stored = relations
            .find_by_id(view.relation_id)
            .await
            .unwrap()
            .unwrap();
        let (lower, higher) = if a.id.0 <= b.id.0 {
            (a.id, b.id)
        } else {
            (b.id, a.id)
        };
        assert_eq!(stored.source_task_id, lower);
        assert_eq!(stored.target_task_id, higher);
        assert_eq!(stored.relation_type, TaskRelationType::RelatesTo);

        // Both ends read relates_to, pointing at the other task.
        assert_eq!(view.relation_type, TaskRelationType::RelatesTo);
        let from_a = service.list_for_task(a.id).await.unwrap();
        let from_b = service.list_for_task(b.id).await.unwrap();
        assert_eq!(from_a[0].relation_type, TaskRelationType::RelatesTo);
        assert_eq!(from_a[0].related_task.id, b.id);
        assert_eq!(from_b[0].relation_type, TaskRelationType::RelatesTo);
        assert_eq!(from_b[0].related_task.id, a.id);
    }

    #[tokio::test]
    async fn blocked_by_a_to_b_is_the_same_relationship_as_blocks_b_to_a() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        // Both submissions normalise to the same canonical row.
        service
            .create(a.id, TaskRelationType::BlockedBy, b.id)
            .await
            .unwrap();
        assert!(matches!(
            service.create(b.id, TaskRelationType::Blocks, a.id).await,
            Err(TaskRelationError::RelationExists)
        ));
    }

    #[tokio::test]
    async fn reverse_blocks_is_rejected_in_both_submitted_forms() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        service
            .create(a.id, TaskRelationType::Blocks, b.id)
            .await
            .unwrap();
        // Both of these normalise to (b -> a, blocks), the reverse of the stored row.
        assert!(matches!(
            service.create(b.id, TaskRelationType::Blocks, a.id).await,
            Err(TaskRelationError::ReverseRelationExists)
        ));
        assert!(matches!(
            service
                .create(a.id, TaskRelationType::BlockedBy, b.id)
                .await,
            Err(TaskRelationError::ReverseRelationExists)
        ));
    }

    #[tokio::test]
    async fn relates_to_in_either_order_is_a_duplicate_the_second_time() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        service
            .create(a.id, TaskRelationType::RelatesTo, b.id)
            .await
            .unwrap();
        assert!(matches!(
            service
                .create(b.id, TaskRelationType::RelatesTo, a.id)
                .await,
            Err(TaskRelationError::RelationExists)
        ));
    }

    #[tokio::test]
    async fn self_relation_is_rejected_before_any_lookup() {
        let (tasks, _relations, service) = setup();
        // No task is seeded: the rejection must happen before any lookup.
        let a = TaskId::new();
        assert!(matches!(
            service.create(a, TaskRelationType::Blocks, a).await,
            Err(TaskRelationError::SelfRelation)
        ));
        assert_eq!(tasks.find_by_id_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn unknown_tasks_give_distinct_not_found_errors() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;

        // Unknown path task is checked first.
        assert!(matches!(
            service
                .create(TaskId::new(), TaskRelationType::Blocks, a.id)
                .await,
            Err(TaskRelationError::TaskNotFound(MissingTask::Path))
        ));
        // Known path task, unknown related task.
        assert!(matches!(
            service
                .create(a.id, TaskRelationType::Blocks, TaskId::new())
                .await,
            Err(TaskRelationError::TaskNotFound(MissingTask::Related))
        ));
    }

    #[tokio::test]
    async fn blocks_and_relates_to_between_the_same_tasks_coexist() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;

        service
            .create(a.id, TaskRelationType::Blocks, b.id)
            .await
            .unwrap();
        service
            .create(a.id, TaskRelationType::RelatesTo, b.id)
            .await
            .unwrap();

        let listed = service.list_for_task(a.id).await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[tokio::test]
    async fn list_returns_both_directions_in_order_with_summaries_and_no_n_plus_one() {
        let (tasks, relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;
        let c = seed_task(&tasks, "C").await;

        // Seed directly with controlled timestamps: a blocks b (a is source),
        // c blocks a (a is target), a relates to c.
        relations
            .create(TaskRelation::new(
                a.id,
                b.id,
                TaskRelationType::Blocks,
                at(1, 1),
            ))
            .await
            .unwrap();
        relations
            .create(TaskRelation::new(
                c.id,
                a.id,
                TaskRelationType::Blocks,
                at(1, 2),
            ))
            .await
            .unwrap();
        relations
            .create(TaskRelation::new(
                a.id,
                c.id,
                TaskRelationType::RelatesTo,
                at(1, 3),
            ))
            .await
            .unwrap();

        let listed = service.list_for_task(a.id).await.unwrap();

        // Deterministic order by created_at: the three seeded rows in sequence.
        assert_eq!(
            listed
                .iter()
                .map(|v| (v.relation_type, v.related_task.id))
                .collect::<Vec<_>>(),
            vec![
                (TaskRelationType::Blocks, b.id),
                (TaskRelationType::BlockedBy, c.id),
                (TaskRelationType::RelatesTo, c.id),
            ]
        );
        // Summaries carry the related task's title and status.
        assert_eq!(listed[0].related_task.title, "B");
        assert_eq!(listed[1].related_task.title, "C");
        assert_eq!(listed[2].related_task.status, TaskStatus::ToDo);

        // No N+1: one batched lookup for the summaries, plus the single path-
        // task existence check — not one find_by_id per relation.
        assert_eq!(tasks.find_by_ids_calls.load(Ordering::Relaxed), 1);
        assert_eq!(tasks.find_by_id_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn deleting_an_unknown_or_unrelated_relation_is_not_found() {
        let (tasks, _relations, service) = setup();
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;
        let c = seed_task(&tasks, "C").await;

        // Unknown relation id.
        assert!(matches!(
            service.delete(a.id, TaskRelationId::new()).await,
            Err(TaskRelationError::RelationNotFound)
        ));

        // A relation that exists but does not involve the path task.
        let view = service
            .create(a.id, TaskRelationType::Blocks, b.id)
            .await
            .unwrap();
        assert!(matches!(
            service.delete(c.id, view.relation_id).await,
            Err(TaskRelationError::RelationNotFound)
        ));

        // The involved task can delete it.
        service.delete(a.id, view.relation_id).await.unwrap();
        assert!(service.list_for_task(a.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn repository_failures_surface_as_the_typed_error() {
        let tasks = Arc::new(CountingTaskRepository::default());
        // Both tasks exist so create reaches the relation repository.
        let a = seed_task(&tasks, "A").await;
        let b = seed_task(&tasks, "B").await;
        let failing_relations = Arc::new(FailingTaskRelationRepository);
        let service = TaskRelationService::new(tasks.clone(), failing_relations);

        assert!(matches!(
            service.create(a.id, TaskRelationType::Blocks, b.id).await,
            Err(TaskRelationError::Repository(_))
        ));
        assert!(matches!(
            service.list_for_task(a.id).await,
            Err(TaskRelationError::Repository(_))
        ));
        assert!(matches!(
            service.delete(a.id, TaskRelationId::new()).await,
            Err(TaskRelationError::Repository(_))
        ));

        // A failing task repository is surfaced the same way.
        tasks.fail_find_by_id.store(true, Ordering::Relaxed);
        let good_relations = Arc::new(InMemoryTaskRelationRepository::new());
        let failing_service = TaskRelationService::new(tasks, good_relations);
        assert!(matches!(
            failing_service
                .create(a.id, TaskRelationType::Blocks, b.id)
                .await,
            Err(TaskRelationError::Repository(_))
        ));
    }
}
