//! Postgres implementation of [`UserRepository`].

use application::ports::{RepositoryError, UserRepository};
use diesel::prelude::*;
use domain::{User, UserId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{UserRow, role_to_db, user_from_row};
use crate::schema::users;

/// [`UserRepository`] backed by Postgres through Diesel.
pub struct PostgresUserRepository {
    pool: PgPool,
}

impl PostgresUserRepository {
    /// Wrap a connection pool in a user repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserRepository for PostgresUserRepository {
    async fn create(&self, user: User) -> Result<User, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            diesel::insert_into(users::table)
                .values((
                    users::id.eq(user.id.0),
                    users::email.eq(&user.email),
                    users::password_hash.eq(&user.password_hash),
                    users::display_name.eq(&user.display_name),
                    users::created_at.eq(&user.created_at),
                    users::updated_at.eq(&user.updated_at),
                    users::role.eq(role_to_db(user.role)),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(user)
        })
        .await
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<UserRow> = users::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(user_from_row).transpose()
        })
        .await
    }

    async fn find_by_email(&self, email: String) -> Result<Option<User>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // Lookups are case-insensitive (see the port docs); the stored
            // value is whatever registration recorded, so normalize here.
            let email = email.to_lowercase();
            let row: Option<UserRow> = users::table
                .filter(users::email.eq(email))
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(user_from_row).transpose()
        })
        .await
    }

    async fn list(&self) -> Result<Vec<User>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            let rows: Vec<UserRow> = users::table.load(conn).map_err(map_diesel_error)?;
            rows.into_iter().map(user_from_row).collect()
        })
        .await
    }

    async fn update(&self, user: User) -> Result<User, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(users::table.find(user.id.0))
                .set((
                    users::email.eq(&user.email),
                    users::password_hash.eq(&user.password_hash),
                    users::display_name.eq(&user.display_name),
                    users::updated_at.eq(&user.updated_at),
                    users::role.eq(role_to_db(user.role)),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            // A user that vanished between read and write is a conflict the
            // caller needs to see, not a silent no-op.
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(user)
        })
        .await
    }
}
