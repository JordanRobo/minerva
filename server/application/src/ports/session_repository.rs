//! Persistence port for [`Session`]s.

use chrono::{DateTime, Utc};
use domain::{Session, SessionId, UserId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, session: Session) -> Result<Session, RepositoryError>;

    /// Returns `None` if no session has this token hash.
    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<Session>, RepositoryError>;

    /// Sessions belonging to the given user.
    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<Session>, RepositoryError>;

    async fn delete(&self, id: SessionId) -> Result<(), RepositoryError>;

    /// Delete every session of the given user (e.g. logout-everywhere or a
    /// password change). A user with no sessions is a valid state, so this
    /// never errors on an empty result.
    async fn delete_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError>;

    /// Record that the session was used at `last_seen_at` and slide its
    /// expiry to `expires_at` (sliding expiry). The update is conditional on
    /// the session still being live, so a touch can never resurrect a
    /// session that was revoked or expired in between the caller's check and
    /// this write; such a touch is a `NotFound`, not an error state.
    async fn touch_last_seen(
        &self,
        id: SessionId,
        last_seen_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError>;

    /// Delete every session past its expiry, returning how many were removed.
    /// The Redis store is a no-op here (native TTLs evict expired keys); it
    /// exists for the Postgres store, where the hourly maintenance job that
    /// calls this is the only thing that removes expired rows (roadmap 2.8).
    async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, RepositoryError>;
}
