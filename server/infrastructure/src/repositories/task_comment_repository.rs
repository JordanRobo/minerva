//! Postgres implementation of [`TaskCommentRepository`].

use application::pagination::{Page, PageRequest};
use application::ports::{RepositoryError, TaskCommentRepository};
use chrono::Utc;
use diesel::prelude::*;
use domain::{TaskComment, TaskCommentId, TaskId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{TaskCommentRow, task_comment_from_row};
use crate::schema::task_comments;

/// [`TaskCommentRepository`] backed by Postgres through Diesel.
pub struct PostgresTaskCommentRepository {
    pool: PgPool,
}

impl PostgresTaskCommentRepository {
    /// Wrap a connection pool in a comment repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TaskCommentRepository for PostgresTaskCommentRepository {
    async fn create(&self, comment: TaskComment) -> Result<TaskComment, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(task_comments::table)
                .values((
                    task_comments::id.eq(comment.id.0),
                    task_comments::task_id.eq(comment.task_id.0),
                    task_comments::author_id.eq(comment.author_id.map(|u| u.0)),
                    task_comments::body.eq(&comment.body),
                    task_comments::created_at.eq(&comment.created_at),
                    task_comments::updated_at.eq(&comment.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(comment)
        })
        .await
    }

    async fn find_by_id(&self, id: TaskCommentId) -> Result<Option<TaskComment>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<TaskCommentRow> = task_comments::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(task_comment_from_row).transpose()
        })
        .await
    }

    async fn list_page_for_task(
        &self,
        task_id: TaskId,
        page: &PageRequest,
    ) -> Result<Page<TaskComment>, RepositoryError> {
        let pool = self.pool.clone();
        let task_id = task_id.0;
        let page = *page;
        let limit = page.limit as i64;
        let offset = page.offset as i64;
        run_on_postgres(pool, move |conn| {
            // The count and the page share one filtered base query (D16):
            // total counts only this task's comments, ignoring paging. A
            // boxed query is consumed by both `count` and `load`, so the
            // builder runs twice.
            let build_query = || {
                task_comments::table
                    .filter(task_comments::task_id.eq(task_id))
                    .into_boxed()
            };
            let total: i64 = build_query()
                .count()
                .first(conn)
                .map_err(map_diesel_error)?;
            // A thread reads oldest first (D20): created_at, then id. Served
            // by idx_task_comments_task_id.
            let rows: Vec<TaskCommentRow> = build_query()
                .order((task_comments::created_at.asc(), task_comments::id.asc()))
                .limit(limit)
                .offset(offset)
                .load(conn)
                .map_err(map_diesel_error)?;
            Ok(Page {
                items: rows
                    .into_iter()
                    .map(task_comment_from_row)
                    .collect::<Result<Vec<_>, _>>()?,
                total: total as u64,
                limit: page.limit,
                offset: page.offset,
            })
        })
        .await
    }

    async fn set_body(
        &self,
        id: TaskCommentId,
        body: String,
    ) -> Result<TaskComment, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // One conditional UPDATE ... RETURNING that writes only the body
            // and updated_at (D20). The DISTINCT FROM filter makes a repeated
            // body write nothing; zero matched rows then means either an
            // unknown id or an unchanged body, and a read tells which.
            let written: Option<TaskCommentRow> = diesel::update(task_comments::table.find(id.0))
                .filter(task_comments::body.is_distinct_from(&body))
                .set((
                    task_comments::body.eq(&body),
                    task_comments::updated_at.eq(Utc::now()),
                ))
                .returning(task_comments::all_columns)
                .get_result(conn)
                .optional()
                .map_err(map_diesel_error)?;
            if let Some(row) = written {
                return task_comment_from_row(row);
            }
            match task_comments::table
                .find(id.0)
                .first::<TaskCommentRow>(conn)
                .optional()
                .map_err(map_diesel_error)?
            {
                // The body was already this: the stored row, untouched.
                Some(row) => task_comment_from_row(row),
                None => Err(RepositoryError::NotFound),
            }
        })
        .await
    }

    async fn delete(&self, id: TaskCommentId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(task_comments::table.find(id.0))
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
