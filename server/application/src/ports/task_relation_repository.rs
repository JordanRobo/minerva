//! Persistence port for [`TaskRelation`]s.

use domain::{TaskId, TaskRelation, TaskRelationId};

use crate::ports::RepositoryError;

/// Why [`TaskRelationRepository::create`] refused a relation.
#[derive(Debug)]
pub enum TaskRelationCreateError {
    /// The relation links a task to itself.
    SelfRelation,
    /// A relation of the same type already exists between the two tasks.
    Duplicate,
    /// A `blocks` relation in the opposite direction already exists: the two
    /// tasks would block each other.
    ReverseExists,
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl From<RepositoryError> for TaskRelationCreateError {
    fn from(error: RepositoryError) -> Self {
        TaskRelationCreateError::Repository(error)
    }
}

#[async_trait::async_trait]
pub trait TaskRelationRepository: Send + Sync {
    /// Store a relation, returning it. Callers pass the canonical form (see
    /// [`TaskRelation::canonical_form`]) so that one row serves both views of
    /// a relationship; the storage-level invariants (no self-relations, one
    /// row per unordered pair and type) are still enforced here, race-free.
    async fn create(&self, relation: TaskRelation)
    -> Result<TaskRelation, TaskRelationCreateError>;

    /// Returns `None` if no relation has this id.
    async fn find_by_id(&self, id: TaskRelationId)
    -> Result<Option<TaskRelation>, RepositoryError>;

    /// Remove a relation; returns whether a row was removed.
    async fn delete(&self, id: TaskRelationId) -> Result<bool, RepositoryError>;

    /// Relations in which the task participates as either source or target,
    /// ordered by created_at then id.
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<TaskRelation>, RepositoryError>;
}

#[cfg(test)]
pub mod fakes {
    use std::sync::Mutex;

    use domain::{TaskId, TaskRelation, TaskRelationId, TaskRelationType};

    use super::{TaskRelationCreateError, TaskRelationRepository};
    use crate::ports::RepositoryError;

    /// In-memory [`TaskRelationRepository`] mirroring the Postgres
    /// invariants: no self-relations, one row per unordered pair and type.
    #[derive(Default)]
    pub struct InMemoryTaskRelationRepository {
        relations: Mutex<Vec<TaskRelation>>,
    }

    impl InMemoryTaskRelationRepository {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl TaskRelationRepository for InMemoryTaskRelationRepository {
        async fn create(
            &self,
            relation: TaskRelation,
        ) -> Result<TaskRelation, TaskRelationCreateError> {
            let mut relations = self.relations.lock().unwrap();
            if relation.source_task_id == relation.target_task_id {
                return Err(TaskRelationCreateError::SelfRelation);
            }
            for existing in relations
                .iter()
                .filter(|r| r.relation_type == relation.relation_type)
            {
                let same_direction = existing.source_task_id == relation.source_task_id
                    && existing.target_task_id == relation.target_task_id;
                let reversed = existing.source_task_id == relation.target_task_id
                    && existing.target_task_id == relation.source_task_id;
                if same_direction || reversed {
                    return if same_direction {
                        Err(TaskRelationCreateError::Duplicate)
                    } else {
                        Err(TaskRelationCreateError::ReverseExists)
                    };
                }
            }
            relations.push(relation.clone());
            Ok(relation)
        }

        async fn find_by_id(
            &self,
            id: TaskRelationId,
        ) -> Result<Option<TaskRelation>, RepositoryError> {
            Ok(self
                .relations
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.id == id)
                .cloned())
        }

        async fn delete(&self, id: TaskRelationId) -> Result<bool, RepositoryError> {
            let mut relations = self.relations.lock().unwrap();
            let before = relations.len();
            relations.retain(|r| r.id != id);
            Ok(relations.len() != before)
        }

        async fn list_for_task(
            &self,
            task_id: TaskId,
        ) -> Result<Vec<TaskRelation>, RepositoryError> {
            let mut listed: Vec<TaskRelation> = self
                .relations
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.source_task_id == task_id || r.target_task_id == task_id)
                .cloned()
                .collect();
            listed.sort_by_key(|a| (a.created_at, a.id.0));
            Ok(listed)
        }
    }

    #[cfg(test)]
    mod tests {
        use chrono::NaiveDate;
        use domain::{TaskId, TaskRelation};

        use super::*;

        fn relation(source: TaskId, target: TaskId, kind: TaskRelationType) -> TaskRelation {
            let created_at = NaiveDate::from_ymd_opt(2026, 1, 15)
                .expect("valid date")
                .and_hms_opt(9, 0, 0)
                .expect("valid time")
                .and_utc();
            TaskRelation::new(source, target, kind, created_at)
        }

        #[tokio::test]
        async fn the_fake_enforces_the_same_invariants_as_postgres() {
            let repo = InMemoryTaskRelationRepository::new();
            let a = TaskId::new();
            let b = TaskId::new();

            assert!(matches!(
                repo.create(relation(a, a, TaskRelationType::Blocks)).await,
                Err(TaskRelationCreateError::SelfRelation)
            ));

            let stored = repo
                .create(relation(a, b, TaskRelationType::Blocks))
                .await
                .expect("first create succeeds");
            assert!(matches!(
                repo.create(relation(a, b, TaskRelationType::Blocks)).await,
                Err(TaskRelationCreateError::Duplicate)
            ));
            assert!(matches!(
                repo.create(relation(b, a, TaskRelationType::Blocks)).await,
                Err(TaskRelationCreateError::ReverseExists)
            ));
            // A different type between the same pair is allowed.
            assert!(
                repo.create(relation(a, b, TaskRelationType::RelatesTo))
                    .await
                    .is_ok()
            );

            assert_eq!(
                repo.find_by_id(stored.id).await.unwrap(),
                Some(stored.clone())
            );
            assert_eq!(repo.list_for_task(a).await.unwrap().len(), 2);
            assert!(repo.delete(stored.id).await.unwrap());
            assert!(!repo.delete(stored.id).await.unwrap());
        }
    }
}
