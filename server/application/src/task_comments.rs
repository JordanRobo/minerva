//! Task comments (roadmap 3.12, D20): flat, plain-text notes on a task.
//!
//! The rules live here, not in the handler: the task must exist before any
//! comment operation (an unknown task is a `TaskNotFound` even when it has
//! zero comments); a comment must exist *on that task* — a comment of
//! another task is a `CommentNotFound`, so its existence is not leaked; only
//! the author may edit, and the author or an Admin may delete (the decisions
//! are in [`crate::authz`], never re-implemented here). The body is
//! normalised by the domain rule before any write. Commenting touches
//! nothing but the comment row: it does not touch the task (not its
//! `updated_at`, not its status), fires no snapshot trigger and recomputes
//! no statuses (D5, D20).

use std::sync::Arc;

use chrono::Utc;
use domain::{CommentBodyError, TaskComment, TaskCommentId, TaskId, User, normalize_comment_body};

use crate::authz;
use crate::pagination::{Page, PageRequest};
use crate::ports::{RepositoryError, StatusSnapshotTrigger, TaskCommentRepository, TaskRepository};
use crate::status_computation::StatusRecomputation;

/// An error from a task-comment operation.
#[derive(Debug)]
pub enum TaskCommentError {
    /// No task with the given id exists.
    TaskNotFound,
    /// No comment with the given id exists on the task. A comment that lives
    /// on another task is reported the same way, so its existence is not
    /// leaked.
    CommentNotFound,
    /// The body failed the D20 rules (blank after trimming, or over 4000
    /// characters).
    InvalidBody(CommentBodyError),
    /// The actor may not perform this operation on the comment.
    Forbidden,
    /// The repository failed.
    Repository(RepositoryError),
}

impl std::fmt::Display for TaskCommentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskCommentError::TaskNotFound => write!(f, "no task with the given id exists"),
            TaskCommentError::CommentNotFound => {
                write!(f, "no comment with the given id exists on the task")
            }
            TaskCommentError::InvalidBody(err) => write!(f, "{err}"),
            TaskCommentError::Forbidden => write!(f, "forbidden"),
            TaskCommentError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TaskCommentError {}

impl From<crate::authz::AuthzError> for TaskCommentError {
    fn from(_: crate::authz::AuthzError) -> Self {
        TaskCommentError::Forbidden
    }
}

/// Creates, reads, edits and deletes comments on tasks (roadmap 3.12, D20).
pub struct TaskCommentService {
    tasks: Arc<dyn TaskRepository>,
    comments: Arc<dyn TaskCommentRepository>,
    // Held but never called (D20): commenting touches nothing but the
    // comment row, so it fires no snapshot trigger and recomputes no
    // statuses. The fakes are injected so the tests can pin that with
    // counters.
    #[allow(dead_code)]
    snapshots: Arc<dyn StatusSnapshotTrigger>,
    #[allow(dead_code)]
    computation: Arc<dyn StatusRecomputation>,
}

impl TaskCommentService {
    pub fn new(
        tasks: Arc<dyn TaskRepository>,
        comments: Arc<dyn TaskCommentRepository>,
        snapshots: Arc<dyn StatusSnapshotTrigger>,
        computation: Arc<dyn StatusRecomputation>,
    ) -> Self {
        Self {
            tasks,
            comments,
            snapshots,
            computation,
        }
    }

    /// One page of the task's comments, oldest first (D20). The task must
    /// exist even when it has no comments.
    pub async fn list(
        &self,
        task_id: TaskId,
        page: &PageRequest,
    ) -> Result<Page<TaskComment>, TaskCommentError> {
        self.ensure_task(task_id).await?;
        self.comments
            .list_page_for_task(task_id, page)
            .await
            .map_err(TaskCommentError::Repository)
    }

    /// Add a comment to the task. The actor is the author (D20: any Staff or
    /// Admin may comment — that is the route's EditContent check, not an
    /// ownership rule); the body is normalised by the domain rule.
    pub async fn create(
        &self,
        actor: &User,
        task_id: TaskId,
        body: &str,
    ) -> Result<TaskComment, TaskCommentError> {
        self.ensure_task(task_id).await?;
        let body = normalize_comment_body(body).map_err(TaskCommentError::InvalidBody)?;
        let now = Utc::now();
        let comment = TaskComment {
            id: TaskCommentId::new(),
            task_id,
            author_id: Some(actor.id),
            body,
            created_at: now,
            updated_at: now,
        };
        self.comments
            .create(comment)
            .await
            .map_err(TaskCommentError::Repository)
    }

    /// Replace the comment's body in place (D20). Check order: the task must
    /// exist, the comment must exist on that task, only the author may edit,
    /// then the body is validated. Re-submitting the stored body writes
    /// nothing and leaves `updated_at` alone.
    pub async fn update(
        &self,
        actor: &User,
        task_id: TaskId,
        comment_id: TaskCommentId,
        body: &str,
    ) -> Result<TaskComment, TaskCommentError> {
        self.ensure_task(task_id).await?;
        let comment = self.find_on_task(task_id, comment_id).await?;
        authz::authorize_comment_edit(actor, &comment)?;
        let body = normalize_comment_body(body).map_err(TaskCommentError::InvalidBody)?;
        if comment.body == body {
            // Idempotent no-op: nothing is written, so updated_at is untouched.
            return Ok(comment);
        }
        self.comments
            .set_body(comment_id, body)
            .await
            .map_err(TaskCommentError::Repository)
    }

    /// Delete the comment (D20: the author or an Admin; a hard delete — no
    /// edit history is kept).
    pub async fn delete(
        &self,
        actor: &User,
        task_id: TaskId,
        comment_id: TaskCommentId,
    ) -> Result<(), TaskCommentError> {
        self.ensure_task(task_id).await?;
        let comment = self.find_on_task(task_id, comment_id).await?;
        authz::authorize_comment_delete(actor, &comment)?;
        self.comments
            .delete(comment_id)
            .await
            .map_err(TaskCommentError::Repository)
    }

    async fn ensure_task(&self, task_id: TaskId) -> Result<(), TaskCommentError> {
        let exists = self
            .tasks
            .find_by_id(task_id)
            .await
            .map_err(TaskCommentError::Repository)?
            .is_some();
        if exists {
            Ok(())
        } else {
            Err(TaskCommentError::TaskNotFound)
        }
    }

    /// The comment if it exists on the given task; a comment of another task
    /// is indistinguishable from a missing one (no existence leak).
    async fn find_on_task(
        &self,
        task_id: TaskId,
        comment_id: TaskCommentId,
    ) -> Result<TaskComment, TaskCommentError> {
        let Some(comment) = self
            .comments
            .find_by_id(comment_id)
            .await
            .map_err(TaskCommentError::Repository)?
        else {
            return Err(TaskCommentError::CommentNotFound);
        };
        if comment.task_id != task_id {
            return Err(TaskCommentError::CommentNotFound);
        }
        Ok(comment)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use chrono::Utc;
    use domain::{Role, Task, TaskComment, TaskCommentId, TaskId, User, UserId};

    use super::*;
    use crate::pagination::PageRequest;
    use crate::ports::{
        StatusChangeTarget, StatusSnapshotError, task_repository::fakes::InMemoryTaskRepository,
    };
    use crate::status_computation::fakes::RecordingRecomputation;

    fn user_with(role: Role) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: "test@example.com".into(),
            password_hash: None,
            display_name: "Test".into(),
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_task() -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            milestone_id: None,
            title: "Test task".to_owned(),
            description: None,
            status: domain::TaskStatus::ToDo,
            target_date: None,
            assignee_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// A comment on the given task with a deterministic timestamp.
    fn stored_comment(
        task_id: TaskId,
        author: Option<UserId>,
        body: &str,
        at: chrono::DateTime<Utc>,
    ) -> TaskComment {
        TaskComment {
            id: TaskCommentId::new(),
            task_id,
            author_id: author,
            body: body.to_owned(),
            created_at: at,
            updated_at: at,
        }
    }

    /// A [`TaskCommentRepository`] mirroring the in-memory fake, plus write
    /// counters and a failure switch so tests can prove what was written.
    #[derive(Default)]
    struct CountingCommentRepository {
        comments: Mutex<Vec<TaskComment>>,
        set_body_writes: AtomicUsize,
        delete_writes: AtomicUsize,
        fail_set_body: AtomicBool,
    }

    impl CountingCommentRepository {
        fn new() -> Self {
            Self::default()
        }

        fn set_fail_set_body(&self, fail: bool) {
            self.fail_set_body.store(fail, Ordering::Relaxed);
        }

        fn locked(&self) -> MutexGuard<'_, Vec<TaskComment>> {
            self.comments.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl TaskCommentRepository for CountingCommentRepository {
        async fn create(&self, comment: TaskComment) -> Result<TaskComment, RepositoryError> {
            self.locked().push(comment.clone());
            Ok(comment)
        }

        async fn find_by_id(
            &self,
            id: TaskCommentId,
        ) -> Result<Option<TaskComment>, RepositoryError> {
            Ok(self
                .locked()
                .iter()
                .find(|comment| comment.id == id)
                .cloned())
        }

        async fn list_page_for_task(
            &self,
            task_id: TaskId,
            page: &PageRequest,
        ) -> Result<Page<TaskComment>, RepositoryError> {
            let mut matched: Vec<TaskComment> = self
                .locked()
                .iter()
                .filter(|comment| comment.task_id == task_id)
                .cloned()
                .collect();
            // Oldest first (D20): created_at, then id.
            matched.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.0.cmp(&b.id.0))
            });
            let total = matched.len() as u64;
            let items = matched
                .into_iter()
                .skip(page.offset as usize)
                .take(page.limit as usize)
                .collect();
            Ok(Page {
                items,
                total,
                limit: page.limit,
                offset: page.offset,
            })
        }

        async fn set_body(
            &self,
            id: TaskCommentId,
            body: String,
        ) -> Result<TaskComment, RepositoryError> {
            if self.fail_set_body.load(Ordering::Relaxed) {
                return Err(RepositoryError::Unexpected(
                    "faking a comment write failure".to_owned(),
                ));
            }
            let mut comments = self.locked();
            let Some(comment) = comments.iter_mut().find(|existing| existing.id == id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror the Postgres implementation: a repeated body writes
            // nothing and leaves updated_at alone (D20).
            if comment.body == body {
                return Ok(comment.clone());
            }
            comment.body = body;
            comment.updated_at = Utc::now();
            self.set_body_writes.fetch_add(1, Ordering::Relaxed);
            Ok(comment.clone())
        }

        async fn delete(&self, id: TaskCommentId) -> Result<(), RepositoryError> {
            let mut comments = self.locked();
            let before = comments.len();
            comments.retain(|comment| comment.id != id);
            if comments.len() == before {
                return Err(RepositoryError::NotFound);
            }
            self.delete_writes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    /// A [`StatusSnapshotTrigger`] that counts every call.
    #[derive(Default)]
    struct CountingSnapshotTrigger {
        snapshots: AtomicUsize,
    }

    impl CountingSnapshotTrigger {
        fn count(&self) -> usize {
            self.snapshots.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl StatusSnapshotTrigger for CountingSnapshotTrigger {
        async fn snapshot(&self, _target: StatusChangeTarget) -> Result<(), StatusSnapshotError> {
            self.snapshots.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn setup() -> (
        Arc<InMemoryTaskRepository>,
        Arc<CountingCommentRepository>,
        Arc<CountingSnapshotTrigger>,
        Arc<RecordingRecomputation>,
        TaskCommentService,
    ) {
        let tasks = Arc::new(InMemoryTaskRepository::new());
        let comments = Arc::new(CountingCommentRepository::new());
        let snapshots = Arc::new(CountingSnapshotTrigger::default());
        let computation = Arc::new(RecordingRecomputation::new());
        let service = TaskCommentService::new(
            tasks.clone(),
            comments.clone(),
            snapshots.clone(),
            computation.clone(),
        );
        (tasks, comments, snapshots, computation, service)
    }

    /// Seed the shared task store with a fresh task and return it.
    async fn seed_task(tasks: &InMemoryTaskRepository) -> Task {
        let task = test_task();
        tasks.create(task.clone()).await.unwrap();
        task
    }

    #[tokio::test]
    async fn list_pages_the_tasks_comments_and_requires_the_task_to_exist() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let other = seed_task(&tasks).await;

        // A task with no comments lists an empty page, not an error.
        let page = service
            .list(other.id, &PageRequest::new(None, None).unwrap())
            .await
            .unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.total, 0);

        // Seeded out of order with distinct timestamps; the thread reads
        // oldest first.
        let day = |n: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 1, n)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap()
                .and_utc()
        };
        for (n, body) in [(3u32, "third"), (1, "first"), (2, "second")] {
            comments
                .create(stored_comment(task.id, None, body, day(n)))
                .await
                .unwrap();
        }

        let page = service
            .list(task.id, &PageRequest::new(None, None).unwrap())
            .await
            .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(
            page.items
                .iter()
                .map(|comment| comment.body.clone())
                .collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );

        // An unknown task is a TaskNotFound even though it has zero comments.
        let result = service
            .list(TaskId::new(), &PageRequest::new(None, None).unwrap())
            .await;
        assert!(matches!(result, Err(TaskCommentError::TaskNotFound)));
    }

    #[tokio::test]
    async fn create_sets_the_actor_as_author_with_equal_timestamps() {
        let (tasks, _comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let actor = user_with(Role::Staff);

        let comment = service.create(&actor, task.id, "hello").await.unwrap();

        assert_eq!(comment.task_id, task.id);
        assert_eq!(comment.author_id, Some(actor.id));
        assert_eq!(comment.body, "hello");
        assert_eq!(comment.created_at, comment.updated_at);
    }

    #[tokio::test]
    async fn create_rejects_blank_and_overlong_bodies_without_writing() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let actor = user_with(Role::Staff);

        let blank = service.create(&actor, task.id, "  \n ").await;
        assert!(matches!(
            blank,
            Err(TaskCommentError::InvalidBody(CommentBodyError::Blank))
        ));

        let overlong = "a".repeat(domain::MAX_COMMENT_BODY_CHARS + 1);
        let result = service.create(&actor, task.id, &overlong).await;
        assert!(matches!(
            result,
            Err(TaskCommentError::InvalidBody(CommentBodyError::TooLong(len)))
                if len == domain::MAX_COMMENT_BODY_CHARS + 1
        ));

        assert!(
            comments.locked().is_empty(),
            "a rejected body must not write"
        );
    }

    #[tokio::test]
    async fn create_trims_the_body_and_keeps_interior_newlines() {
        let (tasks, _comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let actor = user_with(Role::Staff);

        let comment = service
            .create(&actor, task.id, "  hello\nworld  \n")
            .await
            .unwrap();

        assert_eq!(comment.body, "hello\nworld");
    }

    #[tokio::test]
    async fn update_by_the_author_replaces_the_body_and_bumps_updated_at() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let original = service.create(&author, task.id, "original").await.unwrap();

        let edited = service
            .update(&author, task.id, original.id, "edited")
            .await
            .unwrap();

        assert_eq!(edited.body, "edited");
        assert_eq!(edited.created_at, original.created_at);
        assert_eq!(edited.author_id, Some(author.id));
        assert!(edited.updated_at > original.updated_at);
        assert_eq!(comments.set_body_writes.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn update_with_the_same_body_writes_nothing() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let original = service.create(&author, task.id, "hello").await.unwrap();

        // Normalised to the stored body, so it is a no-op: no write, and
        // updated_at stays where the create left it.
        let same = service
            .update(&author, task.id, original.id, "  hello \n")
            .await
            .unwrap();

        assert_eq!(same, original);
        assert_eq!(comments.set_body_writes.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn update_by_a_non_author_is_forbidden_and_writes_nothing() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let comment = service.create(&author, task.id, "original").await.unwrap();

        for role in [Role::Staff, Role::Admin, Role::ReadOnly] {
            let result = service
                .update(&user_with(role), task.id, comment.id, "sneaky")
                .await;
            assert!(
                matches!(result, Err(TaskCommentError::Forbidden)),
                "{role:?}"
            );
        }

        assert_eq!(comments.set_body_writes.load(Ordering::Relaxed), 0);
        let stored = service
            .list(task.id, &PageRequest::new(None, None).unwrap())
            .await
            .unwrap()
            .items
            .pop()
            .unwrap();
        assert_eq!(stored.body, "original");
    }

    #[tokio::test]
    async fn update_and_delete_report_the_right_not_founds() {
        let (tasks, _comments, _snapshots, _computation, service) = setup();
        let task_a = seed_task(&tasks).await;
        let task_b = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let comment = service.create(&author, task_a.id, "on A").await.unwrap();

        // An unknown task is a TaskNotFound...
        assert!(matches!(
            service
                .update(&author, TaskId::new(), comment.id, "x")
                .await,
            Err(TaskCommentError::TaskNotFound)
        ));
        assert!(matches!(
            service.delete(&author, TaskId::new(), comment.id).await,
            Err(TaskCommentError::TaskNotFound)
        ));
        // ...an unknown comment on a real task is a CommentNotFound...
        assert!(matches!(
            service
                .update(&author, task_a.id, TaskCommentId::new(), "x")
                .await,
            Err(TaskCommentError::CommentNotFound)
        ));
        assert!(matches!(
            service
                .delete(&author, task_a.id, TaskCommentId::new())
                .await,
            Err(TaskCommentError::CommentNotFound)
        ));
        // ...and a comment of another task is a CommentNotFound too, so its
        // existence is not leaked.
        assert!(matches!(
            service.update(&author, task_b.id, comment.id, "x").await,
            Err(TaskCommentError::CommentNotFound)
        ));
        assert!(matches!(
            service.delete(&author, task_b.id, comment.id).await,
            Err(TaskCommentError::CommentNotFound)
        ));
    }

    #[tokio::test]
    async fn delete_by_the_author_or_an_admin_works_and_other_staff_cannot() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);

        // The author may delete their own comment.
        let own = service.create(&author, task.id, "mine").await.unwrap();
        service.delete(&author, task.id, own.id).await.unwrap();
        assert!(
            service
                .list(task.id, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap()
                .items
                .is_empty()
        );

        // An Admin may delete someone else's...
        let theirs = service.create(&author, task.id, "theirs").await.unwrap();
        service
            .delete(&user_with(Role::Admin), task.id, theirs.id)
            .await
            .unwrap();
        // ...but another Staff may not, and the comment survives.
        let kept = service.create(&author, task.id, "kept").await.unwrap();
        let writes_before = comments.delete_writes.load(Ordering::Relaxed);
        let result = service
            .delete(&user_with(Role::Staff), task.id, kept.id)
            .await;
        assert!(matches!(result, Err(TaskCommentError::Forbidden)));
        assert_eq!(
            comments.delete_writes.load(Ordering::Relaxed),
            writes_before
        );
        let stored = service
            .list(task.id, &PageRequest::new(None, None).unwrap())
            .await
            .unwrap()
            .items
            .pop()
            .unwrap();
        assert_eq!(stored.id, kept.id);
    }

    #[tokio::test]
    async fn the_check_order_is_pinned() {
        let (tasks, _comments, _snapshots, _computation, service) = setup();
        let task_a = seed_task(&tasks).await;
        let task_b = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let stranger = user_with(Role::ReadOnly);
        let comment = service
            .create(&author, task_a.id, "original")
            .await
            .unwrap();

        // Not-found errors precede Forbidden and InvalidBody: an unknown
        // task wins over a non-author actor and a blank body...
        let result = service
            .update(&stranger, TaskId::new(), comment.id, "   ")
            .await;
        assert!(matches!(result, Err(TaskCommentError::TaskNotFound)));
        // ...as does an unknown comment...
        let result = service
            .update(&stranger, task_a.id, TaskCommentId::new(), "   ")
            .await;
        assert!(matches!(result, Err(TaskCommentError::CommentNotFound)));
        // ...and a comment of another task.
        let result = service
            .update(&stranger, task_b.id, comment.id, "   ")
            .await;
        assert!(matches!(result, Err(TaskCommentError::CommentNotFound)));

        // Forbidden precedes InvalidBody: a non-author with a blank body is
        // refused before the body is even looked at.
        let result = service
            .update(&stranger, task_a.id, comment.id, "   ")
            .await;
        assert!(matches!(result, Err(TaskCommentError::Forbidden)));
    }

    #[tokio::test]
    async fn a_repository_failure_surfaces_as_the_typed_error() {
        let (tasks, comments, _snapshots, _computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);
        let comment = service.create(&author, task.id, "original").await.unwrap();
        comments.set_fail_set_body(true);

        let result = service.update(&author, task.id, comment.id, "edited").await;
        assert!(matches!(result, Err(TaskCommentError::Repository(_))));
    }

    #[tokio::test]
    async fn commenting_fires_no_snapshot_and_no_recompute() {
        let (tasks, _comments, snapshots, computation, service) = setup();
        let task = seed_task(&tasks).await;
        let author = user_with(Role::Staff);

        let comment = service.create(&author, task.id, "hello").await.unwrap();
        service
            .update(&author, task.id, comment.id, "edited")
            .await
            .unwrap();
        service.delete(&author, task.id, comment.id).await.unwrap();

        assert_eq!(snapshots.count(), 0, "commenting fires no snapshot (D5)");
        assert!(
            computation.calls().is_empty(),
            "commenting recomputes no statuses (D20)"
        );
    }
}
