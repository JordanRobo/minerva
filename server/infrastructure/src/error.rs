//! Mapping from Diesel and connection-pool errors to the application
//! layer's [`RepositoryError`].

use application::ports::RepositoryError;
use diesel::result::{DatabaseErrorKind, Error as DieselError};

/// Map a Diesel query error to [`RepositoryError`].
///
/// Unique-constraint violations become [`RepositoryError::Conflict`],
/// "no rows" results become [`RepositoryError::NotFound`], and everything
/// else is surfaced as [`RepositoryError::Unexpected`] carrying the
/// underlying message.
pub fn map_diesel_error(err: DieselError) -> RepositoryError {
    match err {
        DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _) => {
            RepositoryError::Conflict(err.to_string())
        }
        DieselError::NotFound => RepositoryError::NotFound,
        other => RepositoryError::Unexpected(other.to_string()),
    }
}

/// Map a connection-pool error to [`RepositoryError::Unexpected`].
///
/// Takes [`diesel::r2d2::PoolError`] — what `Pool::get()` actually returns.
/// (`diesel::r2d2::Error` is a different, Diesel-local enum whose name
/// shadows the r2d2 re-export.)
pub fn map_pool_error(err: diesel::r2d2::PoolError) -> RepositoryError {
    RepositoryError::Unexpected(format!("connection pool error: {err}"))
}
