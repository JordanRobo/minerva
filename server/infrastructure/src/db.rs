//! Postgres connection pool shared by the Diesel-based adapters.

use diesel::r2d2::{self, ConnectionManager};
use diesel::PgConnection;

use application::ports::RepositoryError;

use crate::error::map_pool_error;

/// A pool of reusable Postgres connections.
pub type PgPool = r2d2::Pool<ConnectionManager<PgConnection>>;

/// Build a connection pool for `database_url`.
///
/// Panics if the pool cannot be created or no initial connection can be
/// established: a server without its primary datastore has no meaningful
/// degraded mode, so we fail fast at startup rather than surfacing the error
/// through every repository call.
pub fn build_pool(database_url: &str) -> PgPool {
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    let pool = r2d2::Pool::builder()
        .build(manager)
        .expect("could not create Postgres connection pool");
    // Check one connection out (and return it to the pool) so an
    // unreachable database fails here, at startup, instead of on the first
    // query.
    pool.get().expect("could not establish initial Postgres connection");
    pool
}

/// Run a blocking operation on a pooled connection, off the async runtime's
/// worker threads.
///
/// `op` receives a live connection and must return a [`RepositoryError`]
/// itself, mapping query errors through [`crate::error::map_diesel_error`] —
/// row-mapping failures (e.g. an unknown status value in the database) are
/// not Diesel errors and cannot be mapped here. Pool errors are mapped
/// through [`map_pool_error`]. The connection is passed mutably because
/// Diesel's write operations (`execute`) require it; read-only queries can
/// ignore the mutability.
pub async fn run_on_postgres<T, F>(pool: PgPool, op: F) -> Result<T, RepositoryError>
where
    T: Send + 'static,
    F: FnOnce(&mut PgConnection) -> Result<T, RepositoryError> + Send + 'static,
{
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(map_pool_error)?;
        op(&mut conn)
    })
    .await
    .map_err(|err| RepositoryError::Unexpected(format!("blocking task failed: {err}")))?;
    result
}
