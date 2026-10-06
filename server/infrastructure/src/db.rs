//! Postgres connection pool shared by the Diesel-based adapters.

use diesel::PgConnection;
use diesel::prelude::*;
use diesel::r2d2::{self, ConnectionManager};

use application::ports::RepositoryError;

use crate::error::{map_diesel_error, map_pool_error};

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
    pool.get()
        .expect("could not establish initial Postgres connection");
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
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(map_pool_error)?;
        op(&mut conn)
    })
    .await
    .map_err(|err| RepositoryError::Unexpected(format!("blocking task failed: {err}")))?
}

/// Like [`run_on_postgres`], but for operations whose failures are not
/// plain [`RepositoryError`] (e.g. the user access-change transaction, whose
/// typed outcomes — not found, last admin — are policy decisions, not storage
/// failures). Pool and task failures still surface as [`RepositoryError`]
/// and are lifted into `E` through its `From` impl.
pub async fn run_on_postgres_with<T, E, F>(pool: PgPool, op: F) -> Result<T, E>
where
    T: Send + 'static,
    E: From<RepositoryError> + Send + 'static,
    F: FnOnce(&mut PgConnection) -> Result<T, E> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(map_pool_error)?;
        op(&mut conn)
    })
    .await
    .map_err(|err| RepositoryError::Unexpected(format!("blocking task failed: {err}")))?
}

/// A session-level Postgres advisory lock held on a dedicated pooled
/// connection, released explicitly when dropped: pooled connections are
/// reused, so returning one to the pool while it still holds the lock would
/// hand the lock to the next user of that connection.
pub struct AdvisoryLock {
    key: i64,
    conn: r2d2::PooledConnection<ConnectionManager<PgConnection>>,
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        // A failed unlock means the connection is gone; there is nothing left
        // to release.
        let _ = diesel::sql_query(format!("SELECT pg_advisory_unlock({})", self.key))
            .execute(&mut self.conn);
    }
}

/// The single column of `SELECT pg_try_advisory_lock(…) AS held`; the alias
/// names it so [`diesel::sql_query`] can map it by name.
#[derive(diesel::QueryableByName)]
struct AdvisoryLockHeld {
    #[diesel(sql_type = diesel::sql_types::Bool)]
    held: bool,
}

/// Try to take a session-level advisory lock on a dedicated pooled
/// connection, off the async runtime's worker threads. `Ok(None)` means
/// another node holds the lock: the caller skips rather than waits, so this
/// never blocks a request or a maintenance tick. The lock is released by
/// dropping the returned [`AdvisoryLock`].
pub async fn try_acquire_advisory_lock(
    pool: &PgPool,
    key: i64,
) -> Result<Option<AdvisoryLock>, RepositoryError> {
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(map_pool_error)?;
        let AdvisoryLockHeld { held } =
            diesel::sql_query(format!("SELECT pg_try_advisory_lock({key}) AS held"))
                .get_result(&mut conn)
                .map_err(map_diesel_error)?;
        if held {
            Ok(Some(AdvisoryLock { key, conn }))
        } else {
            // The connection goes back to the pool without holding anything.
            Ok(None)
        }
    })
    .await
    .map_err(|err| RepositoryError::Unexpected(format!("blocking task failed: {err}")))?
}
