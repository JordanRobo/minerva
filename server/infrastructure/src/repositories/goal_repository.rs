//! Postgres implementation of [`GoalRepository`].

use application::ports::{GoalRepository, RepositoryError};
use diesel::prelude::*;
use domain::{Goal, GoalId};

use crate::db::{run_on_postgres, PgPool};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{goal_from_row, status_source_to_db, status_to_db, GoalRow};
use crate::schema::goals;

/// [`GoalRepository`] backed by Postgres through Diesel.
pub struct PostgresGoalRepository {
    pool: PgPool,
}

impl PostgresGoalRepository {
    /// Wrap a connection pool in a goal repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl GoalRepository for PostgresGoalRepository {
    async fn create(&self, goal: Goal) -> Result<Goal, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(goals::table)
                .values((
                    goals::id.eq(goal.id.0),
                    goals::title.eq(&goal.title),
                    goals::description.eq(goal.description.as_deref()),
                    goals::status.eq(status_to_db(goal.status.status)),
                    goals::status_source.eq(status_source_to_db(goal.status.source)),
                    goals::target_date.eq(goal.target_date),
                    goals::created_at.eq(&goal.created_at),
                    goals::updated_at.eq(&goal.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(goal)
        })
        .await
    }

    async fn find_by_id(&self, id: GoalId) -> Result<Option<Goal>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<GoalRow> = goals::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(goal_from_row).transpose()
        })
        .await
    }

    async fn list(&self) -> Result<Vec<Goal>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            let rows: Vec<GoalRow> = goals::table.load(conn).map_err(map_diesel_error)?;
            rows.into_iter().map(goal_from_row).collect()
        })
        .await
    }

    async fn update(&self, goal: Goal) -> Result<Goal, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(goals::table.find(goal.id.0))
                .set((
                    goals::title.eq(&goal.title),
                    goals::description.eq(goal.description.as_deref()),
                    goals::status.eq(status_to_db(goal.status.status)),
                    goals::status_source.eq(status_source_to_db(goal.status.source)),
                    goals::target_date.eq(goal.target_date),
                    goals::updated_at.eq(&goal.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            // A goal that vanished between read and write is a conflict the
            // caller needs to see, not a silent no-op.
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(goal)
        })
        .await
    }

    async fn delete(&self, id: GoalId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(goals::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if removed == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }
}
