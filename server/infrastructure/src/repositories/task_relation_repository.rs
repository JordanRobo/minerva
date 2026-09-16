//! Postgres implementation of [`TaskRelationRepository`].

use application::ports::{RepositoryError, TaskRelationRepository};
use diesel::prelude::*;
use domain::{TaskId, TaskRelation, TaskRelationId};

use crate::db::{run_on_postgres, PgPool};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{relation_type_to_db, task_relation_from_row, TaskRelationRow};
use crate::schema::task_relations;

/// [`TaskRelationRepository`] backed by Postgres through Diesel.
pub struct PostgresTaskRelationRepository {
    pool: PgPool,
}

impl PostgresTaskRelationRepository {
    /// Wrap a connection pool in a task-relation repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TaskRelationRepository for PostgresTaskRelationRepository {
    async fn create(&self, relation: TaskRelation) -> Result<TaskRelation, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(task_relations::table)
                .values((
                    task_relations::id.eq(relation.id.0),
                    task_relations::source_task_id.eq(relation.source_task_id.0),
                    task_relations::target_task_id.eq(relation.target_task_id.0),
                    task_relations::relation_type.eq(relation_type_to_db(relation.relation_type)?),
                    task_relations::created_at.eq(&relation.created_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(relation)
        })
        .await
    }

    async fn delete(&self, id: TaskRelationId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(task_relations::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if removed == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<TaskRelation>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<TaskRelationRow> = task_relations::table
                .filter(
                    task_relations::source_task_id
                        .eq(task_id.0)
                        .or(task_relations::target_task_id.eq(task_id.0)),
                )
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(task_relation_from_row).collect()
        })
        .await
    }
}
