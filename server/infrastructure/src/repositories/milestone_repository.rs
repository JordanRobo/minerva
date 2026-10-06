//! Postgres implementation of [`MilestoneRepository`].

use application::ports::{MilestoneRepository, RepositoryError};
use diesel::prelude::*;
use domain::{Milestone, MilestoneId, Status, StatusSource};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{
    MilestoneRow, milestone_from_row, status_source_to_db, status_to_db,
};
use crate::schema::milestones;

/// [`MilestoneRepository`] backed by Postgres through Diesel.
pub struct PostgresMilestoneRepository {
    pool: PgPool,
}

impl PostgresMilestoneRepository {
    /// Wrap a connection pool in a milestone repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MilestoneRepository for PostgresMilestoneRepository {
    async fn create(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(milestones::table)
                .values((
                    milestones::id.eq(milestone.id.0),
                    milestones::title.eq(&milestone.title),
                    milestones::description.eq(milestone.description.as_deref()),
                    milestones::status.eq(status_to_db(milestone.status)),
                    milestones::status_source.eq(status_source_to_db(milestone.status_source())),
                    milestones::target_date.eq(milestone.target_date),
                    milestones::created_at.eq(&milestone.created_at),
                    milestones::updated_at.eq(&milestone.updated_at),
                    milestones::status_override.eq(milestone.status_override.map(status_to_db)),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(milestone)
        })
        .await
    }

    async fn find_by_id(&self, id: MilestoneId) -> Result<Option<Milestone>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<MilestoneRow> = milestones::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(milestone_from_row).transpose()
        })
        .await
    }

    async fn list(&self) -> Result<Vec<Milestone>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            let rows: Vec<MilestoneRow> = milestones::table.load(conn).map_err(map_diesel_error)?;
            rows.into_iter().map(milestone_from_row).collect()
        })
        .await
    }

    async fn update(&self, milestone: Milestone) -> Result<Milestone, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(milestones::table.find(milestone.id.0))
                .set((
                    milestones::title.eq(&milestone.title),
                    milestones::description.eq(milestone.description.as_deref()),
                    milestones::status.eq(status_to_db(milestone.status)),
                    milestones::status_source.eq(status_source_to_db(milestone.status_source())),
                    milestones::target_date.eq(milestone.target_date),
                    milestones::updated_at.eq(&milestone.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            // A milestone that vanished between read and write is a conflict
            // the caller needs to see, not a silent no-op. The override
            // column is deliberately absent: an ordinary update must never
            // clear or change it (roadmap 3.2).
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(milestone)
        })
        .await
    }

    async fn set_status_override(
        &self,
        id: MilestoneId,
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
            let updated = diesel::update(milestones::table.find(id.0))
                .set((
                    milestones::status_override.eq(status_override.map(status_to_db)),
                    milestones::status_source.eq(status_source_to_db(source)),
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

    async fn delete(&self, id: MilestoneId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(milestones::table.find(id.0))
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
