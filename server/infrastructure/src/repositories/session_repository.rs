//! Postgres implementation of [`SessionRepository`].

use application::ports::{RepositoryError, SessionRepository};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use domain::{Session, SessionId, UserId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{SessionRow, session_from_row};
use crate::schema::sessions;

/// [`SessionRepository`] backed by Postgres through Diesel.
pub struct PostgresSessionRepository {
    pool: PgPool,
}

impl PostgresSessionRepository {
    /// Wrap a connection pool in a session repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SessionRepository for PostgresSessionRepository {
    async fn create(&self, session: Session) -> Result<Session, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(sessions::table)
                .values((
                    sessions::id.eq(session.id.0),
                    sessions::user_id.eq(session.user_id.0),
                    sessions::token_hash.eq(&session.token_hash),
                    sessions::created_at.eq(&session.created_at),
                    sessions::expires_at.eq(&session.expires_at),
                    sessions::last_seen_at.eq(&session.last_seen_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(session)
        })
        .await
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<Session>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<SessionRow> = sessions::table
                .filter(sessions::token_hash.eq(token_hash))
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(session_from_row).transpose()
        })
        .await
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<Session>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<SessionRow> = sessions::table
                .filter(sessions::user_id.eq(user_id.0))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(session_from_row).collect()
        })
        .await
    }

    async fn delete(&self, id: SessionId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let removed = diesel::delete(sessions::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if removed == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn delete_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::delete(sessions::table.filter(sessions::user_id.eq(user_id.0)))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(())
        })
        .await
    }

    async fn touch_last_seen(
        &self,
        id: SessionId,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(sessions::table.find(id.0))
                .set(sessions::last_seen_at.eq(last_seen_at))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }
}
