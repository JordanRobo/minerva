//! Postgres implementation of [`GoalRepository`].

use application::pagination::{Page, PageRequest};
use application::ports::{GoalListFilter, GoalRepository, RepositoryError};
use diesel::prelude::*;
use diesel::sql_types::{Nullable, Text};
use domain::{Goal, GoalId, Status, StatusSource};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{GoalRow, goal_from_row, status_source_to_db, status_to_db};
use crate::schema::goals;

// Diesel has no built-in `COALESCE`, so declare it: the effective-status
// filter compares `COALESCE(status_override, status)` against the requested
// statuses (roadmap 3.10).
diesel::define_sql_function! {
    fn coalesce(a: Nullable<Text>, b: Nullable<Text>) -> Nullable<Text>;
}

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
                    goals::status.eq(status_to_db(goal.status)),
                    goals::status_source.eq(status_source_to_db(goal.status_source())),
                    goals::target_date.eq(goal.target_date),
                    goals::created_at.eq(&goal.created_at),
                    goals::updated_at.eq(&goal.updated_at),
                    goals::status_override.eq(goal.status_override.map(status_to_db)),
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
                    goals::status.eq(status_to_db(goal.status)),
                    goals::status_source.eq(status_source_to_db(goal.status_source())),
                    goals::target_date.eq(goal.target_date),
                    goals::updated_at.eq(&goal.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            // A goal that vanished between read and write is a conflict the
            // caller needs to see, not a silent no-op. The override column is
            // deliberately absent: an ordinary update must never clear or
            // change it (roadmap 3.2).
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(goal)
        })
        .await
    }

    async fn set_status_override(
        &self,
        id: GoalId,
        status_override: Option<Status>,
    ) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The legacy `status_source` column mirrors the override so
            // direct database readers see the same source the domain derives.
            let source = match status_override {
                Some(_) => StatusSource::ManualOverride,
                None => StatusSource::Computed,
            };
            let updated = diesel::update(goals::table.find(id.0))
                .set((
                    goals::status_override.eq(status_override.map(status_to_db)),
                    goals::status_source.eq(status_source_to_db(source)),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
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

    async fn list_page(
        &self,
        filter: &GoalListFilter,
        page: &PageRequest,
    ) -> Result<Page<Goal>, RepositoryError> {
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
                let mut query = goals::table.into_boxed();
                if !filter.statuses.is_empty() {
                    // The status filter matches the effective status (roadmap
                    // 3.2): the manual override when set, else the automatic
                    // value — hence the COALESCE over both columns.
                    let statuses: Vec<&'static str> =
                        filter.statuses.iter().copied().map(status_to_db).collect();
                    query = query.filter(
                        coalesce(goals::status_override, goals::status.nullable()).eq_any(statuses),
                    );
                }
                if let Some(needle) = &filter.q {
                    query = query.filter(goals::title.ilike(escape_like_pattern(needle)));
                }
                if let Some(after) = filter.target_after {
                    query = query.filter(goals::target_date.ge(after));
                }
                if let Some(before) = filter.target_before {
                    query = query.filter(goals::target_date.le(before));
                }
                query
            };
            let total: i64 = build_query()
                .count()
                .first(conn)
                .map_err(map_diesel_error)?;
            // The default order (roadmap 3.16): target date ascending with
            // nulls last, then created_at, then id.
            let rows: Vec<GoalRow> = build_query()
                .order((
                    goals::target_date.asc().nulls_last(),
                    goals::created_at.asc(),
                    goals::id.asc(),
                ))
                .limit(limit)
                .offset(offset)
                .load(conn)
                .map_err(map_diesel_error)?;
            Ok(Page {
                items: rows
                    .into_iter()
                    .map(goal_from_row)
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
