//! Postgres implementation of [`UserRepository`].

use application::ports::{AccessChange, AccessChangeError, RepositoryError, UserRepository};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use domain::{Role, User, UserId};

use crate::db::{PgPool, run_on_postgres, run_on_postgres_with};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{UserRow, role_from_db, role_to_db, user_from_row};
use crate::schema::users;

/// Advisory lock key for user access changes. Arbitrary but stable across
/// nodes and releases; "MNVRACCS" read as ASCII, in the style of the
/// migration lock. Transaction-level (`pg_advisory_xact_lock`), so Postgres
/// releases it at commit or rollback.
const ACCESS_CHANGE_LOCK_KEY: i64 = 0x4D4E5652_41434353;

/// Advisory lock key for first-admin bootstrap. Arbitrary but stable across
/// nodes and releases; "MNVRBOOT" read as ASCII, in the style of the
/// migration lock. Transaction-level (`pg_advisory_xact_lock`), so Postgres
/// releases it at commit or rollback.
const BOOTSTRAP_LOCK_KEY: i64 = 0x4D4E5652_424F4F54;

/// Errors inside the access-change transaction, before they are mapped to
/// [`AccessChangeError`].
#[derive(Debug)]
enum AccessTxError {
    NotFound,
    LastAdmin,
    Diesel(diesel::result::Error),
    Mapped(RepositoryError),
}

impl From<diesel::result::Error> for AccessTxError {
    fn from(error: diesel::result::Error) -> Self {
        AccessTxError::Diesel(error)
    }
}

impl From<RepositoryError> for AccessTxError {
    fn from(error: RepositoryError) -> Self {
        AccessTxError::Mapped(error)
    }
}

/// The active admins other than `target`, counted under the access-change
/// lock so concurrent changes cannot slip in between the count and the write.
fn count_other_active_admins(
    conn: &mut PgConnection,
    target: UserId,
) -> Result<i64, AccessTxError> {
    let count: i64 = users::table
        .filter(
            users::role
                .eq("admin")
                .and(users::deactivated_at.is_null())
                .and(users::id.ne(target.0)),
        )
        .count()
        .first(conn)?;
    Ok(count)
}

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
                    users::deactivated_at.eq(&user.deactivated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(user)
        })
        .await
    }

    async fn create_if_no_users(&self, user: User) -> Result<Option<User>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            conn.transaction::<Option<User>, diesel::result::Error, _>(move |conn| {
                // Serialize concurrent bootstrap attempts: the row count is
                // only trusted after every preceding attempt has committed.
                diesel::sql_query(format!(
                    "SELECT pg_advisory_xact_lock({BOOTSTRAP_LOCK_KEY})"
                ))
                .execute(conn)?;

                let count: i64 = users::table.count().first(conn)?;
                if count > 0 {
                    return Ok(None);
                }
                diesel::insert_into(users::table)
                    .values((
                        users::id.eq(user.id.0),
                        users::email.eq(&user.email),
                        users::password_hash.eq(&user.password_hash),
                        users::display_name.eq(&user.display_name),
                        users::created_at.eq(&user.created_at),
                        users::updated_at.eq(&user.updated_at),
                        users::role.eq(role_to_db(user.role)),
                        users::deactivated_at.eq(&user.deactivated_at),
                    ))
                    .execute(conn)?;
                Ok(Some(user))
            })
            .map_err(map_diesel_error)
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
            // Stable order for the admin list view: creation time, then id.
            let rows: Vec<UserRow> = users::table
                .order((users::created_at.asc(), users::id.asc()))
                .load(conn)
                .map_err(map_diesel_error)?;
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
                    users::deactivated_at.eq(&user.deactivated_at),
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

    async fn apply_access_change(
        &self,
        target: UserId,
        change: AccessChange,
    ) -> Result<User, AccessChangeError> {
        let pool = self.pool.clone();
        run_on_postgres_with(pool, move |conn| {
            let outcome = conn.transaction::<User, AccessTxError, _>(|conn| {
                // Serialize concurrent access changes: the active-admin count
                // is re-checked only after every preceding change has committed.
                diesel::sql_query(format!(
                    "SELECT pg_advisory_xact_lock({ACCESS_CHANGE_LOCK_KEY})"
                ))
                .execute(conn)?;

                let row: Option<UserRow> = users::table.find(target.0).first(conn).optional()?;
                let Some(row) = row else {
                    return Err(AccessTxError::NotFound);
                };
                let role = role_from_db(&row.6)?;
                let active = row.7.is_none();

                match change {
                    AccessChange::Role(new_role) => {
                        if new_role == role {
                            return Ok(user_from_row(row)?);
                        }
                        // Demoting the last active admin would leave no one who
                        // can manage users again.
                        if role == Role::Admin
                            && active
                            && new_role != Role::Admin
                            && count_other_active_admins(conn, target)? == 0
                        {
                            return Err(AccessTxError::LastAdmin);
                        }
                        let now = Utc::now();
                        diesel::update(users::table.find(target.0))
                            .set((
                                users::role.eq(role_to_db(new_role)),
                                users::updated_at.eq(now),
                            ))
                            .execute(conn)?;
                    }
                    AccessChange::Deactivate => {
                        if !active {
                            return Ok(user_from_row(row)?);
                        }
                        if role == Role::Admin && count_other_active_admins(conn, target)? == 0 {
                            return Err(AccessTxError::LastAdmin);
                        }
                        let now = Utc::now();
                        diesel::update(users::table.find(target.0))
                            .set((
                                users::deactivated_at.eq(Some(now)),
                                users::updated_at.eq(now),
                            ))
                            .execute(conn)?;
                    }
                    AccessChange::Reactivate => {
                        if active {
                            return Ok(user_from_row(row)?);
                        }
                        let now = Utc::now();
                        diesel::update(users::table.find(target.0))
                            .set((
                                users::deactivated_at.eq(None::<DateTime<Utc>>),
                                users::updated_at.eq(now),
                            ))
                            .execute(conn)?;
                    }
                }

                // Return the row as stored, not a copy of what was intended.
                let row: UserRow = users::table.find(target.0).first(conn)?;
                Ok(user_from_row(row)?)
            });
            match outcome {
                Ok(user) => Ok(user),
                Err(AccessTxError::NotFound) => Err(AccessChangeError::NotFound),
                Err(AccessTxError::LastAdmin) => Err(AccessChangeError::LastAdmin),
                Err(AccessTxError::Diesel(err)) => {
                    Err(AccessChangeError::Repository(map_diesel_error(err)))
                }
                Err(AccessTxError::Mapped(err)) => Err(AccessChangeError::Repository(err)),
            }
        })
        .await
    }
}
