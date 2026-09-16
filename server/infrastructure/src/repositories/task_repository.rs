//! Postgres implementation of [`TaskRepository`].

use application::ports::{RepositoryError, TaskRepository};
use diesel::prelude::*;
use domain::{MilestoneId, Task, TaskId};

use crate::db::{run_on_postgres, PgPool};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{task_from_row, task_status_to_db, TaskRow};
use crate::schema::tasks;

/// [`TaskRepository`] backed by Postgres through Diesel.
pub struct PostgresTaskRepository {
    pool: PgPool,
}

impl PostgresTaskRepository {
    /// Wrap a connection pool in a task repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TaskRepository for PostgresTaskRepository {
    async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(tasks::table)
                .values((
                    tasks::id.eq(task.id.0),
                    tasks::milestone_id.eq(task.milestone_id.map(|m| m.0)),
                    tasks::title.eq(&task.title),
                    tasks::description.eq(task.description.as_deref()),
                    tasks::status.eq(task_status_to_db(task.status)),
                    tasks::target_date.eq(task.target_date),
                    tasks::created_at.eq(&task.created_at),
                    tasks::updated_at.eq(&task.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(task)
        })
        .await
    }

    async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<TaskRow> = tasks::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(task_from_row).transpose()
        })
        .await
    }

    async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(tasks::table.find(task.id.0))
                .set((
                    tasks::milestone_id.eq(task.milestone_id.map(|m| m.0)),
                    tasks::title.eq(&task.title),
                    tasks::description.eq(task.description.as_deref()),
                    tasks::status.eq(task_status_to_db(task.status)),
                    tasks::target_date.eq(task.target_date),
                    tasks::updated_at.eq(&task.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            // A task that vanished between read and write is a conflict the
            // caller needs to see, not a silent no-op.
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(task)
        })
        .await
    }

    async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(tasks::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if removed == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn list_by_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Task>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<TaskRow> = tasks::table
                .filter(tasks::milestone_id.eq(milestone_id.0))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(task_from_row).collect()
        })
        .await
    }

    async fn list_unassigned(&self) -> Result<Vec<Task>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            let rows: Vec<TaskRow> = tasks::table
                .filter(tasks::milestone_id.is_null())
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(task_from_row).collect()
        })
        .await
    }
}
