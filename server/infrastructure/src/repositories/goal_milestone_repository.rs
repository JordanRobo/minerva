//! Postgres implementation of [`GoalMilestoneRepository`].

use application::ports::{GoalMilestoneRepository, RepositoryError};
use diesel::prelude::*;
use domain::{GoalId, GoalMilestone, MilestoneId};
use uuid::Uuid;

use crate::db::{run_on_postgres, PgPool};
use crate::error::map_diesel_error;
use crate::schema::goal_milestones;

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
    async fn link(&self, link: GoalMilestone) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(goal_milestones::table)
                .values((
                    goal_milestones::goal_id.eq(link.goal_id.0),
                    goal_milestones::milestone_id.eq(link.milestone_id.0),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(())
        })
        .await
    }

    async fn unlink(
        &self,
        goal_id: GoalId,
        milestone_id: MilestoneId,
    ) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(
                goal_milestones::table
                    .filter(goal_milestones::goal_id.eq(goal_id.0))
                    .filter(goal_milestones::milestone_id.eq(milestone_id.0)),
            )
            .execute(conn)
            .map_err(map_diesel_error)?;
            if removed == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn milestones_for_goal(&self, goal_id: GoalId) -> Result<Vec<MilestoneId>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let ids: Vec<Uuid> = goal_milestones::table
                .select(goal_milestones::milestone_id)
                .filter(goal_milestones::goal_id.eq(goal_id.0))
                .load(conn)
                .map_err(map_diesel_error)?;
            Ok(ids.into_iter().map(MilestoneId).collect())
        })
        .await
    }

    async fn goals_for_milestone(
        &self,
        milestone_id: MilestoneId,
    ) -> Result<Vec<GoalId>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let ids: Vec<Uuid> = goal_milestones::table
                .select(goal_milestones::goal_id)
                .filter(goal_milestones::milestone_id.eq(milestone_id.0))
                .load(conn)
                .map_err(map_diesel_error)?;
            Ok(ids.into_iter().map(GoalId).collect())
        })
        .await
    }
}
