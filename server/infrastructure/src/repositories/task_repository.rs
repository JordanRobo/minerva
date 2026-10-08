//! Postgres implementation of [`TaskRepository`].

use application::pagination::{Page, PageRequest};
use application::ports::{RepositoryError, TaskListFilter, TaskRepository};
use chrono::Utc;
use diesel::prelude::*;
use domain::{Task, TaskId, TaskStatus};
use uuid::Uuid;

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{TaskRow, task_from_row, task_status_to_db};
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

    async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
        let pool = self.pool.clone();
        let id_values: Vec<Uuid> = ids.iter().map(|id| id.0).collect();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<TaskRow> = tasks::table
                .filter(tasks::id.eq_any(id_values))
                .order_by(tasks::id.asc())
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(task_from_row).collect()
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

    async fn set_status(&self, id: TaskId, status: TaskStatus) -> Result<Task, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // One UPDATE ... RETURNING that writes only the status column and
            // updated_at, so a concurrent edit of any other field survives it
            // (roadmap 3.7). A task that does not exist matches no row.
            let row: TaskRow = diesel::update(tasks::table.find(id.0))
                .set((
                    tasks::status.eq(task_status_to_db(status)),
                    tasks::updated_at.eq(Utc::now()),
                ))
                .returning(tasks::all_columns)
                .get_result(conn)
                .map_err(map_diesel_error)?;
            task_from_row(row)
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

    async fn list_page(
        &self,
        filter: &TaskListFilter,
        page: &PageRequest,
    ) -> Result<Page<Task>, RepositoryError> {
        let pool = self.pool.clone();
        let filter = filter.clone();
        let page = *page;
        let limit = page.limit as i64;
        let offset = page.offset as i64;
        run_on_postgres(pool, move |conn| {
            // The count and the page share one filtered base query (roadmap
            // 3.10): total honours every filter but ignores limit/offset. A
            // boxed query is consumed by both `count` and `load`, so the
            // builder runs twice.
            let build_query = || {
                let mut query = tasks::table.into_boxed();
                if !filter.statuses.is_empty() {
                    let statuses: Vec<&'static str> = filter
                        .statuses
                        .iter()
                        .copied()
                        .map(task_status_to_db)
                        .collect();
                    query = query.filter(tasks::status.eq_any(statuses));
                }
                if let Some(milestone_id) = filter.milestone_id {
                    query = query.filter(tasks::milestone_id.eq(milestone_id.0));
                }
                if let Some(needle) = &filter.q {
                    query = query.filter(tasks::title.ilike(escape_like_pattern(needle)));
                }
                if let Some(after) = filter.target_after {
                    query = query.filter(tasks::target_date.ge(after));
                }
                if let Some(before) = filter.target_before {
                    query = query.filter(tasks::target_date.le(before));
                }
                query
            };
            let total: i64 = build_query()
                .count()
                .first(conn)
                .map_err(map_diesel_error)?;
            // The default order (roadmap 3.16): target date ascending with
            // nulls last, then created_at, then id.
            let rows: Vec<TaskRow> = build_query()
                .order((
                    tasks::target_date.asc().nulls_last(),
                    tasks::created_at.asc(),
                    tasks::id.asc(),
                ))
                .limit(limit)
                .offset(offset)
                .load(conn)
                .map_err(map_diesel_error)?;
            Ok(Page {
                items: rows
                    .into_iter()
                    .map(task_from_row)
                    .collect::<Result<Vec<_>, _>>()?,
                total: total as u64,
                limit: page.limit,
                offset: page.offset,
            })
        })
        .await
    }
}

/// A `LIKE`/`ILIKE` pattern matching `needle` literally as a substring: the
/// wildcard characters are escaped, so a search for `100%` finds only titles
/// containing that exact text.
fn escape_like_pattern(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len() + 2);
    for ch in needle.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    format!("%{escaped}%")
}
