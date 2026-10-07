//! Postgres implementation of [`TaskRelationRepository`].

use application::ports::{RepositoryError, TaskRelationCreateError, TaskRelationRepository};
use diesel::prelude::*;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use domain::{TaskId, TaskRelation, TaskRelationId};
use uuid::Uuid;

use crate::db::{PgPool, run_on_postgres, run_on_postgres_with};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{TaskRelationRow, relation_type_to_db, task_relation_from_row};
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
    async fn create(
        &self,
        relation: TaskRelation,
    ) -> Result<TaskRelation, TaskRelationCreateError> {
        // The CHECK constraint is the backstop; this keeps the common case off
        // the error path and gives self-relations their own typed error.
        if relation.source_task_id == relation.target_task_id {
            return Err(TaskRelationCreateError::SelfRelation);
        }
        let pool = self.pool.clone();
        run_on_postgres_with(pool, move |conn| {
            let inserted: Result<TaskRelation, TaskRelationCreateError> =
                match diesel::insert_into(task_relations::table)
                    .values((
                        task_relations::id.eq(relation.id.0),
                        task_relations::source_task_id.eq(relation.source_task_id.0),
                        task_relations::target_task_id.eq(relation.target_task_id.0),
                        task_relations::relation_type
                            .eq(relation_type_to_db(relation.relation_type)?),
                        task_relations::created_at.eq(&relation.created_at),
                    ))
                    .execute(conn)
                {
                    Ok(_) => Ok(relation),
                    // The partial unique index fired: a row for the same unordered
                    // pair and type exists. Its direction decides which typed error
                    // this is — the lookup, not a prior check, makes that call so
                    // concurrent racers are classified against committed state.
                    Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
                        Err(classify_pair_violation(conn, &relation)?)
                    }
                    Err(other) => Err(TaskRelationCreateError::Repository(map_diesel_error(other))),
                };
            inserted
        })
        .await
    }

    async fn find_by_id(
        &self,
        id: TaskRelationId,
    ) -> Result<Option<TaskRelation>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<TaskRelationRow> = task_relations::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(task_relation_from_row).transpose()
        })
        .await
    }

    async fn delete(&self, id: TaskRelationId) -> Result<bool, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(task_relations::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(removed > 0)
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
                .order_by((task_relations::created_at.asc(), task_relations::id.asc()))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(task_relation_from_row).collect()
        })
        .await
    }
}

/// The unique-pair violation means a committed row exists for the same
/// unordered pair and type; if it points the other way this is a reversed
/// `blocks` relation, otherwise a plain duplicate.
fn classify_pair_violation(
    conn: &mut PgConnection,
    relation: &TaskRelation,
) -> Result<TaskRelationCreateError, TaskRelationCreateError> {
    let stored_type = relation_type_to_db(relation.relation_type)?;
    let existing: Option<(Uuid, Uuid)> = task_relations::table
        .select((
            task_relations::source_task_id,
            task_relations::target_task_id,
        ))
        .filter(
            task_relations::relation_type.eq(stored_type).and(
                task_relations::source_task_id
                    .eq(relation.source_task_id.0)
                    .and(task_relations::target_task_id.eq(relation.target_task_id.0))
                    .or(task_relations::source_task_id
                        .eq(relation.target_task_id.0)
                        .and(task_relations::target_task_id.eq(relation.source_task_id.0))),
            ),
        )
        .first(conn)
        .optional()
        .map_err(|err| TaskRelationCreateError::Repository(map_diesel_error(err)))?;
    match existing {
        Some((source, target))
            if source == relation.source_task_id.0 && target == relation.target_task_id.0 =>
        {
            Ok(TaskRelationCreateError::Duplicate)
        }
        Some(_) => Ok(TaskRelationCreateError::ReverseExists),
        // The conflicting row vanished between the violation and the lookup;
        // report it rather than guessing which typed error it would have been.
        None => Err(TaskRelationCreateError::Repository(
            RepositoryError::Unexpected(
                "unique violation with no existing row to classify against".to_owned(),
            ),
        )),
    }
}
