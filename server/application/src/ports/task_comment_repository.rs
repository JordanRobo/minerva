//! Persistence port for [`TaskComment`]s (roadmap 3.12, D20).

use domain::{TaskComment, TaskCommentId, TaskId};

use crate::pagination::{Page, PageRequest};
use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait TaskCommentRepository: Send + Sync {
    /// Store a comment, returning it. The body must already be normalised
    /// (see [`domain::normalize_comment_body`]); the storage-level CHECK
    /// constraint is the backstop. The domain does not check that the task
    /// or author exists — a dangling task reference is an `InvalidReference`
    /// from Postgres, and the author may be `None`.
    async fn create(&self, comment: TaskComment) -> Result<TaskComment, RepositoryError>;

    /// Returns `None` if no comment has this id.
    async fn find_by_id(&self, id: TaskCommentId) -> Result<Option<TaskComment>, RepositoryError>;

    /// One page of the comments on one task (roadmap 3.12), oldest first —
    /// created_at, then id — a thread reads top to bottom. There are no
    /// filters; [`Page::total`] counts only this task's comments, ignoring
    /// `page` (D16). A task with no comments — or no such task — is an empty
    /// page, never an error.
    async fn list_page_for_task(
        &self,
        task_id: TaskId,
        page: &PageRequest,
    ) -> Result<Page<TaskComment>, RepositoryError>;

    /// Replace the comment's body in place (D20): one write of `body` and
    /// `updated_at`, returning the updated comment. An unknown id is a
    /// `NotFound`; a body identical to the stored one writes nothing and
    /// leaves `updated_at` alone, returning the stored comment.
    async fn set_body(
        &self,
        id: TaskCommentId,
        body: String,
    ) -> Result<TaskComment, RepositoryError>;

    /// Hard delete (D20: no edit history). An unknown id is a `NotFound`.
    async fn delete(&self, id: TaskCommentId) -> Result<(), RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::sync::Mutex;

    use chrono::Utc;
    use domain::{TaskComment, TaskCommentId, TaskId};

    use super::TaskCommentRepository;
    use crate::pagination::{Page, PageRequest};
    use crate::ports::RepositoryError;

    /// In-memory [`TaskCommentRepository`] mirroring the Postgres
    /// implementation, for tests that need a comment store without a database.
    #[derive(Default)]
    pub struct InMemoryTaskCommentRepository {
        comments: Mutex<Vec<TaskComment>>,
    }

    impl InMemoryTaskCommentRepository {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl TaskCommentRepository for InMemoryTaskCommentRepository {
        async fn create(&self, comment: TaskComment) -> Result<TaskComment, RepositoryError> {
            self.comments.lock().unwrap().push(comment.clone());
            Ok(comment)
        }

        async fn find_by_id(
            &self,
            id: TaskCommentId,
        ) -> Result<Option<TaskComment>, RepositoryError> {
            Ok(self
                .comments
                .lock()
                .unwrap()
                .iter()
                .find(|comment| comment.id == id)
                .cloned())
        }

        async fn list_page_for_task(
            &self,
            task_id: TaskId,
            page: &PageRequest,
        ) -> Result<Page<TaskComment>, RepositoryError> {
            let comments = self.comments.lock().unwrap();
            // Oldest first (D20): created_at, then id — the same order as the
            // Postgres implementation.
            let mut matched: Vec<TaskComment> = comments
                .iter()
                .filter(|comment| comment.task_id == task_id)
                .cloned()
                .collect();
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
            let mut comments = self.comments.lock().unwrap();
            let Some(comment) = comments.iter_mut().find(|existing| existing.id == id) else {
                return Err(RepositoryError::NotFound);
            };
            // Mirror Postgres: a repeated body writes nothing and leaves
            // updated_at alone (D20).
            if comment.body == body {
                return Ok(comment.clone());
            }
            comment.body = body;
            comment.updated_at = Utc::now();
            Ok(comment.clone())
        }

        async fn delete(&self, id: TaskCommentId) -> Result<(), RepositoryError> {
            let mut comments = self.comments.lock().unwrap();
            let before = comments.len();
            comments.retain(|comment| comment.id != id);
            if comments.len() == before {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use chrono::{DateTime, Utc};
        use domain::{TaskCommentId, TaskId, UserId};
        use uuid::Uuid;

        use super::*;

        fn at(day: u32) -> DateTime<Utc> {
            let date = chrono::NaiveDate::from_ymd_opt(2026, 1, day).expect("valid date");
            date.and_hms_opt(9, 0, 0).expect("valid time").and_utc()
        }

        /// A comment with a deterministic id on the given task.
        fn comment(
            id: u128,
            task_id: TaskId,
            body: &str,
            created_at: DateTime<Utc>,
        ) -> TaskComment {
            TaskComment {
                id: TaskCommentId(Uuid::from_u128(id)),
                task_id,
                author_id: None,
                body: body.to_owned(),
                created_at,
                updated_at: created_at,
            }
        }

        fn ids(page: &Page<TaskComment>) -> Vec<u128> {
            page.items
                .iter()
                .map(|comment| comment.id.0.as_u128())
                .collect()
        }

        #[tokio::test]
        async fn the_list_is_oldest_first_with_an_id_tie_break_and_counts_only_the_task() {
            let repo = InMemoryTaskCommentRepository::new();
            let task_a = TaskId::new();
            let task_b = TaskId::new();
            // Inserted out of order, with a created_at tie on task A.
            for comment in [
                comment(3, task_a, "third", at(2)),
                comment(1, task_a, "first", at(1)),
                comment(9, task_b, "other task", at(1)),
                comment(2, task_a, "second", at(2)),
            ] {
                repo.create(comment).await.unwrap();
            }

            let page = repo
                .list_page_for_task(task_a, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap();
            assert_eq!(ids(&page), vec![1, 2, 3]);
            // The total counts only this task's comments...
            assert_eq!(page.total, 3);
            // ...and a deep page keeps it.
            let page = repo
                .list_page_for_task(task_a, &PageRequest::new(Some(2), Some(1)).unwrap())
                .await
                .unwrap();
            assert_eq!(ids(&page), vec![2, 3]);
            assert_eq!(page.total, 3);

            // Another task's thread is independent...
            let page = repo
                .list_page_for_task(task_b, &PageRequest::new(None, None).unwrap())
                .await
                .unwrap();
            assert_eq!(ids(&page), vec![9]);
            assert_eq!(page.total, 1);
            // ...and an unknown task is an empty page, not an error (D16).
            let page = repo
                .list_page_for_task(TaskId::new(), &PageRequest::new(None, None).unwrap())
                .await
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.total, 0);
        }

        #[tokio::test]
        async fn set_body_writes_only_when_the_body_changes() {
            let repo = InMemoryTaskCommentRepository::new();
            let task_id = TaskId::new();
            let author = UserId::new();
            let mut stored = comment(1, task_id, "original", at(1));
            stored.author_id = Some(author);
            repo.create(stored.clone()).await.unwrap();

            // An unchanged body writes nothing: the returned comment is the
            // stored one, updated_at included (D20).
            let same = repo
                .set_body(stored.id, "original".to_owned())
                .await
                .unwrap();
            assert_eq!(same, stored);

            // A changed body replaces it in place and bumps only updated_at.
            let edited = repo.set_body(stored.id, "edited".to_owned()).await.unwrap();
            assert_eq!(edited.body, "edited");
            assert_eq!(edited.created_at, at(1));
            assert_eq!(edited.author_id, Some(author));
            assert!(edited.updated_at > at(1));

            // Unknown ids are a typed NotFound for both writers...
            assert!(matches!(
                repo.set_body(TaskCommentId::new(), "x".to_owned()).await,
                Err(RepositoryError::NotFound)
            ));
            // ...and the delete.
            assert!(matches!(
                repo.delete(TaskCommentId::new()).await,
                Err(RepositoryError::NotFound)
            ));

            repo.delete(stored.id).await.unwrap();
            assert_eq!(repo.find_by_id(stored.id).await.unwrap(), None);
        }
    }
}
