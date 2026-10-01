//! Applies the embedded Diesel migrations at startup.
//!
//! The API is designed to scale horizontally, so several nodes can start
//! against the same database at once. A session-level advisory lock on the
//! migration connection serializes them instead of letting them race over
//! `diesel_schema_info`.

use diesel::prelude::*;
use diesel_migrations::{embed_migrations, MigrationHarness};

use crate::db::PgPool;

/// The migrations embedded in this binary. The path is relative to
/// `infrastructure/Cargo.toml`; `build.rs` re-runs when the directory
/// changes so a new migration triggers a rebuild.
const MIGRATIONS: diesel_migrations::EmbeddedMigrations = embed_migrations!("../migrations");

/// Advisory lock key for startup migrations. Arbitrary but stable across
/// nodes and releases; "MNVRMIGR" read as ASCII, so it cannot collide with
/// any other advisory lock in the deployment.
const MIGRATION_LOCK_KEY: i64 = 0x4D4E5652_4D494752;

/// Apply all pending migrations, returning the names of the ones applied.
///
/// Takes a session-level advisory lock on the connection it runs on so that
/// nodes starting simultaneously serialize instead of racing, and releases
/// it before returning (on success or failure).
pub fn run_migrations(pool: &PgPool) -> Result<Vec<String>, String> {
    let mut conn = pool
        .get()
        .map_err(|err| format!("could not check out a connection for migrations: {err}"))?;
    diesel::sql_query(format!("SELECT pg_advisory_lock({MIGRATION_LOCK_KEY})"))
        .execute(&mut conn)
        .map_err(|err| format!("could not take the migration advisory lock: {err}"))?;

    let applied = conn
        .run_pending_migrations(MIGRATIONS)
        .map_err(|err| format!("could not apply migrations: {err}"))
        .map(|versions| versions.into_iter().map(|version| version.to_string()).collect());

    // Release the lock even when the migrations failed: the connection goes
    // back to the pool and would otherwise hold it until closed, blocking
    // every other node's startup. A failure here means the connection is
    // gone, in which case there is nothing left to do anyway.
    let _ = diesel::sql_query(format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY})"))
        .execute(&mut conn);

    applied
}
