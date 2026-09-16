//! Postgres implementation of [`ProgressSnapshotRepository`].

use application::ports::{ProgressSnapshotRepository, RepositoryError};
use diesel::prelude::*;
use domain::{ProgressSnapshot, ProgressTarget};

use crate::db::{run_on_postgres, PgPool};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{progress_snapshot_from_row, status_to_db, ProgressSnapshotRow};
use crate::schema::progress_snapshots;

/// [`ProgressSnapshotRepository`] backed by Postgres through Diesel.
pub struct PostgresProgressSnapshotRepository {
    pool: PgPool,
}

impl PostgresProgressSnapshotRepository {
    /// Wrap a connection pool in a progress-snapshot repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProgressSnapshotRepository for PostgresProgressSnapshotRepository {
    async fn create(&self, snapshot: ProgressSnapshot) -> Result<ProgressSnapshot, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The domain type is exactly one of goal or milestone; the
            // database CHECK enforces the same on these two columns.
            let (goal_id, milestone_id) = match snapshot.target {
                ProgressTarget::Goal(goal_id) => (Some(goal_id.0), None),
                ProgressTarget::Milestone(milestone_id) => (None, Some(milestone_id.0)),
            };
            diesel::insert_into(progress_snapshots::table)
                .values((
                    progress_snapshots::id.eq(snapshot.id.0),
                    progress_snapshots::goal_id.eq(goal_id),
                    progress_snapshots::milestone_id.eq(milestone_id),
                    progress_snapshots::recorded_at.eq(&snapshot.recorded_at),
                    progress_snapshots::status.eq(status_to_db(snapshot.status)),
                    progress_snapshots::percent_complete.eq(i16::from(snapshot.percent_complete())),
                    progress_snapshots::note.eq(snapshot.note.as_deref()),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(snapshot)
        })
        .await
    }

    async fn list_for_target(
        &self,
        target: ProgressTarget,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<ProgressSnapshotRow> = match target {
                ProgressTarget::Goal(goal_id) => progress_snapshots::table
                    .filter(progress_snapshots::goal_id.eq(goal_id.0))
                    .order(progress_snapshots::recorded_at.asc())
                    .load(conn),
                ProgressTarget::Milestone(milestone_id) => progress_snapshots::table
                    .filter(progress_snapshots::milestone_id.eq(milestone_id.0))
                    .order(progress_snapshots::recorded_at.asc())
                    .load(conn),
            }
            .map_err(map_diesel_error)?;
            rows.into_iter().map(progress_snapshot_from_row).collect()
        })
        .await
    }
}
