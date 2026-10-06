//! Postgres implementation of [`GoalMilestoneRepository`].

use application::ports::{GoalMilestoneRepository, RepositoryError};
use diesel::prelude::*;
use domain::{Goal, GoalId, GoalMilestone, Milestone, MilestoneId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{GoalRow, MilestoneRow, goal_from_row, milestone_from_row};
use crate::schema::{goal_milestones, goals, milestones};

/// [`GoalMilestoneRepository`] backed by Postgres through Diesel.
pub struct PostgresGoalMilestoneRepository {
    pool: PgPool,
}

impl PostgresGoalMilestoneRepository {
    /// Wrap a connection pool in a goal–milestone repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl GoalMilestoneRepository for PostgresGoalMilestoneRepository {
    async fn link(&self, link: GoalMilestone) -> Result<bool, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The pair is the primary key, so a racing duplicate insert is
            // dropped instead of erroring; the row count says who won.
            let inserted = diesel::insert_into(goal_milestones::table)
                .values((
                    goal_milestones::goal_id.eq(link.goal_id.0),
                    goal_milestones::milestone_id.eq(link.milestone_id.0),
                ))
                .on_conflict_do_nothing()
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(inserted > 0)
        })
        .await
    }

    async fn unlink(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<bool, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(
                goal_milestones::table
                    .filter(goal_milestones::goal_id.eq(goal_id.0))
                    .filter(goal_milestones::milestone_id.eq(milestone_id.0)),
            )
            .execute(conn)
            .map_err(map_diesel_error)?;
            Ok(removed > 0)
        })
        .await
    }

    async fn milestones_for_goal(
        &self,
        goal_id: GoalId,
    ) -> Result<Vec<Milestone>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<MilestoneRow> = goal_milestones::table
                .inner_join(milestones::table.on(goal_milestones::milestone_id.eq(milestones::id)))
                .filter(goal_milestones::goal_id.eq(goal_id.0))
                .order((
                    milestones::target_date.asc().nulls_last(),
                    milestones::created_at.asc(),
                    milestones::id.asc(),
                ))
                .select((
                    milestones::id,
                    milestones::title,
                    milestones::description,
                    milestones::status,
                    milestones::status_source,
                    milestones::target_date,
                    milestones::created_at,
                    milestones::updated_at,
                    milestones::status_override,
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(milestone_from_row).collect()
        })
        .await
    }

    async fn goals_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<Goal>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<GoalRow> = goal_milestones::table
                .inner_join(goals::table.on(goal_milestones::goal_id.eq(goals::id)))
                .filter(goal_milestones::milestone_id.eq(milestone_id.0))
                .order((
                    goals::target_date.asc().nulls_last(),
                    goals::created_at.asc(),
                    goals::id.asc(),
                ))
                .select((
                    goals::id,
                    goals::title,
                    goals::description,
                    goals::status,
                    goals::status_source,
                    goals::target_date,
                    goals::created_at,
                    goals::updated_at,
                    goals::status_override,
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(goal_from_row).collect()
        })
        .await
    }
}
