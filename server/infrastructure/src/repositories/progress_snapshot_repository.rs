//! Postgres implementation of [`ProgressSnapshotRepository`].

use application::ports::{ProgressSnapshotRepository, RepositoryError};
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{GoalId, MilestoneId, NewSnapshot, ProgressSnapshot, ProgressTarget};
use uuid::Uuid;

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{
    ProgressSnapshotRow, progress_snapshot_from_row, snapshot_reason_to_db, status_source_to_db,
    status_to_db,
};
use crate::schema::{goals, milestones, progress_snapshots};

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

/// Split a snapshot's target into the table's two nullable entity columns;
/// the database CHECK enforces that exactly one is set.
fn entity_columns(snapshot: &NewSnapshot) -> (Option<Uuid>, Option<Uuid>) {
    match snapshot.target {
        ProgressTarget::Goal(goal_id) => (Some(goal_id.0), None),
        ProgressTarget::Milestone(milestone_id) => (None, Some(milestone_id.0)),
    }
}

/// The columns read back for a snapshot row, in [`ProgressSnapshotRow`]
/// order. The stored `snapshot_date` is a pure function of `taken_at`, so it
/// is not selected: the domain re-derives it.
fn snapshot_columns() -> (
    progress_snapshots::id,
    progress_snapshots::goal_id,
    progress_snapshots::milestone_id,
    progress_snapshots::taken_at,
    progress_snapshots::status,
    progress_snapshots::status_source,
    progress_snapshots::reason,
    progress_snapshots::percent_complete,
) {
    (
        progress_snapshots::id,
        progress_snapshots::goal_id,
        progress_snapshots::milestone_id,
        progress_snapshots::taken_at,
        progress_snapshots::status,
        progress_snapshots::status_source,
        progress_snapshots::reason,
        progress_snapshots::percent_complete,
    )
}

#[async_trait::async_trait]
impl ProgressSnapshotRepository for PostgresProgressSnapshotRepository {
    async fn record(&self, snapshot: NewSnapshot) -> Result<ProgressSnapshot, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let (goal_id, milestone_id) = entity_columns(&snapshot);
            diesel::insert_into(progress_snapshots::table)
                .values((
                    progress_snapshots::id.eq(snapshot.id.0),
                    progress_snapshots::goal_id.eq(goal_id),
                    progress_snapshots::milestone_id.eq(milestone_id),
                    progress_snapshots::taken_at.eq(&snapshot.taken_at),
                    progress_snapshots::snapshot_date.eq(snapshot.snapshot_date),
                    progress_snapshots::status.eq(status_to_db(snapshot.status)),
                    progress_snapshots::status_source.eq(status_source_to_db(snapshot.source)),
                    progress_snapshots::reason.eq(snapshot_reason_to_db(snapshot.reason)),
                    progress_snapshots::percent_complete
                        .eq(snapshot.percent_complete().map(i16::from)),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(ProgressSnapshot::from_new(snapshot))
        })
        .await
    }

    async fn record_daily_batch(
        &self,
        snapshots: &[NewSnapshot],
    ) -> Result<usize, RepositoryError> {
        if snapshots.is_empty() {
            return Ok(0);
        }
        // Owned values, so the closure below can be 'static.
        let rows: Vec<(
            Uuid,
            Option<Uuid>,
            Option<Uuid>,
            DateTime<Utc>,
            NaiveDate,
            String,
            String,
            String,
            Option<i16>,
        )> = snapshots
            .iter()
            .map(|snapshot| {
                (
                    snapshot.id.0,
                    entity_columns(snapshot).0,
                    entity_columns(snapshot).1,
                    snapshot.taken_at,
                    snapshot.snapshot_date,
                    status_to_db(snapshot.status).to_owned(),
                    status_source_to_db(snapshot.source).to_owned(),
                    snapshot_reason_to_db(snapshot.reason).to_owned(),
                    snapshot.percent_complete().map(i16::from),
                )
            })
            .collect();
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // One row of column expressions per snapshot; Diesel's multi-row
            // insert takes exactly that shape.
            let values: Vec<_> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        goal_id,
                        milestone_id,
                        taken_at,
                        snapshot_date,
                        status,
                        status_source,
                        reason,
                        percent_complete,
                    )| {
                        (
                            progress_snapshots::id.eq(id),
                            progress_snapshots::goal_id.eq(goal_id),
                            progress_snapshots::milestone_id.eq(milestone_id),
                            progress_snapshots::taken_at.eq(taken_at),
                            progress_snapshots::snapshot_date.eq(snapshot_date),
                            progress_snapshots::status.eq(status),
                            progress_snapshots::status_source.eq(status_source),
                            progress_snapshots::reason.eq(reason),
                            progress_snapshots::percent_complete.eq(percent_complete),
                        )
                    },
                )
                .collect();
            // DO NOTHING on the daily partial unique index: an entity that
            // already has its row for the day is skipped, and the count says
            // how many were actually stored.
            diesel::insert_into(progress_snapshots::table)
                .values(values)
                .on_conflict_do_nothing()
                .execute(conn)
                .map_err(map_diesel_error)
        })
        .await
    }

    async fn entity_ids_missing_daily(
        &self,
        date: NaiveDate,
    ) -> Result<(Vec<GoalId>, Vec<MilestoneId>), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The anti-join in two steps, like task_counts_for_milestones:
            // the ids with a daily row for the date, then the diff against
            // every id. A manual-change row does not count as a daily one.
            let covered_goal_rows: Vec<Option<Uuid>> = progress_snapshots::table
                .filter(
                    progress_snapshots::reason
                        .eq("daily")
                        .and(progress_snapshots::snapshot_date.eq(date)),
                )
                .select(progress_snapshots::goal_id)
                .load(conn)
                .map_err(map_diesel_error)?;
            let covered_goals: Vec<Uuid> = covered_goal_rows.into_iter().flatten().collect();
            let goal_ids: Vec<Uuid> = goals::table
                .select(goals::id)
                .order(goals::id.asc())
                .load(conn)
                .map_err(map_diesel_error)?;
            let missing_goals: Vec<GoalId> = goal_ids
                .into_iter()
                .filter(|id| !covered_goals.contains(id))
                .map(GoalId)
                .collect();

            let covered_milestone_rows: Vec<Option<Uuid>> = progress_snapshots::table
                .filter(
                    progress_snapshots::reason
                        .eq("daily")
                        .and(progress_snapshots::snapshot_date.eq(date)),
                )
                .select(progress_snapshots::milestone_id)
                .load(conn)
                .map_err(map_diesel_error)?;
            let covered_milestones: Vec<Uuid> =
                covered_milestone_rows.into_iter().flatten().collect();
            let milestone_ids: Vec<Uuid> = milestones::table
                .select(milestones::id)
                .order(milestones::id.asc())
                .load(conn)
                .map_err(map_diesel_error)?;
            let missing_milestones: Vec<MilestoneId> = milestone_ids
                .into_iter()
                .filter(|id| !covered_milestones.contains(id))
                .map(MilestoneId)
                .collect();

            Ok((missing_goals, missing_milestones))
        })
        .await
    }

    async fn list_for_goal(
        &self,
        goal_id: GoalId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<ProgressSnapshotRow> = progress_snapshots::table
                .filter(progress_snapshots::goal_id.eq(goal_id.0))
                .select(snapshot_columns())
                .order((
                    progress_snapshots::taken_at.asc(),
                    progress_snapshots::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(progress_snapshot_from_row).collect()
        })
        .await
    }

    async fn list_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<ProgressSnapshot>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<ProgressSnapshotRow> = progress_snapshots::table
                .filter(progress_snapshots::milestone_id.eq(milestone_id.0))
                .select(snapshot_columns())
                .order((
                    progress_snapshots::taken_at.asc(),
                    progress_snapshots::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(progress_snapshot_from_row).collect()
        })
        .await
    }
}
