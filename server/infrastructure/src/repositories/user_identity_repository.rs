//! Postgres implementation of [`UserIdentityRepository`].

use application::ports::{RepositoryError, UserIdentityRepository};
use diesel::prelude::*;
use domain::{UserId, UserIdentity};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{UserIdentityRow, user_identity_from_row};
use crate::schema::user_identities;

/// [`UserIdentityRepository`] backed by Postgres through Diesel.
pub struct PostgresUserIdentityRepository {
    pool: PgPool,
}

impl PostgresUserIdentityRepository {
    /// Wrap a connection pool in a user identity repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserIdentityRepository for PostgresUserIdentityRepository {
    async fn create(&self, identity: UserIdentity) -> Result<UserIdentity, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // A duplicate (issuer, subject) pair violates the unique
            // constraint and is mapped to `Conflict` by `map_diesel_error`.
            diesel::insert_into(user_identities::table)
                .values((
                    user_identities::id.eq(identity.id.0),
                    user_identities::user_id.eq(identity.user_id.0),
                    user_identities::issuer.eq(&identity.issuer),
                    user_identities::subject.eq(&identity.subject),
                    user_identities::email.eq(&identity.email),
                    user_identities::created_at.eq(&identity.created_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(identity)
        })
        .await
    }

    async fn find_by_issuer_and_subject(
        &self,
        issuer: String,
        subject: String,
    ) -> Result<Option<UserIdentity>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<UserIdentityRow> = user_identities::table
                .filter(user_identities::issuer.eq(issuer))
                .filter(user_identities::subject.eq(subject))
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(user_identity_from_row).transpose()
        })
        .await
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<UserIdentity>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let rows: Vec<UserIdentityRow> = user_identities::table
                .filter(user_identities::user_id.eq(user_id.0))
                .order(user_identities::created_at.asc())
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(user_identity_from_row).collect()
        })
        .await
    }
}
