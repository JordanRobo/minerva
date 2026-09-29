//! Redis implementation of [`SessionRepository`].
//!
//! Sessions are stored as hashes keyed by token hash, with a native Redis
//! TTL matching `expires_at`, so expired sessions are evicted by Redis
//! itself — no cleanup job needed. A per-user set indexes that user's
//! session tokens for `list_for_user` and `delete_all_for_user`.

use application::ports::{RepositoryError, SessionRepository};
use chrono::{DateTime, Utc};
use domain::{Session, SessionId, UserId};
use redis::Commands;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::map_redis_error;

/// Hash holding a session's fields, keyed by the token hash.
fn session_key(token_hash: &str) -> String {
    format!("session:{token_hash}")
}

/// Maps a session id to its token hash, so `delete` can find the session
/// key (which is keyed by token hash, not id).
fn session_id_key(id: &SessionId) -> String {
    format!("session_id:{}", id.0)
}

/// Set of the token hashes of all sessions belonging to a user.
fn user_sessions_key(user_id: &UserId) -> String {
    format!("user_sessions:{}", user_id.0)
}

fn field(fields: &mut HashMap<String, String>, name: &str) -> Result<String, RepositoryError> {
    fields.remove(name).ok_or_else(|| {
        RepositoryError::Unexpected(format!("missing field {name:?} in redis session hash"))
    })
}

/// Format a timestamp for storage as RFC 3339 with a `Z` suffix.
///
/// Does not use `DateTime::to_string()`: its format is not guaranteed to be
/// RFC 3339, and [`timestamp`] (the reader) only accepts RFC 3339.
fn ts_to_str(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

fn timestamp(
    fields: &mut HashMap<String, String>,
    name: &str,
) -> Result<DateTime<Utc>, RepositoryError> {
    DateTime::parse_from_rfc3339(&field(fields, name)?)
        .map(|ts| ts.with_timezone(&Utc))
        .map_err(|err| {
            RepositoryError::Unexpected(format!("bad timestamp in field {name:?}: {err}"))
        })
}

/// Rebuild a [`Session`] from the fields of a `session:*` hash.
fn session_from_hash(mut fields: HashMap<String, String>) -> Result<Session, RepositoryError> {
    let id = Uuid::parse_str(&field(&mut fields, "id")?).map_err(|err| {
        RepositoryError::Unexpected(format!("bad session id in redis session hash: {err}"))
    })?;
    let user_id = Uuid::parse_str(&field(&mut fields, "user_id")?).map_err(|err| {
        RepositoryError::Unexpected(format!("bad user id in redis session hash: {err}"))
    })?;
    Ok(Session {
        id: SessionId(id),
        user_id: UserId(user_id),
        token_hash: field(&mut fields, "token_hash")?,
        created_at: timestamp(&mut fields, "created_at")?,
        expires_at: timestamp(&mut fields, "expires_at")?,
        last_seen_at: timestamp(&mut fields, "last_seen_at")?,
    })
}

/// [`SessionRepository`] backed by Redis.
///
/// ponytail: each operation opens its own short-lived connection (the
/// `Client` is cheap to clone and holds no socket); move to a redis r2d2
/// pool if session operations become hot.
pub struct RedisSessionRepository {
    client: redis::Client,
}

impl RedisSessionRepository {
    /// Connect to Redis at `url`, failing fast if it is unreachable — the
    /// same fail-fast-when-configured behavior as [`crate::db::build_pool`].
    pub fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        // Check one connection (and drop it) so an unreachable Redis fails
        // here, at startup, instead of on the first session lookup.
        client.get_connection()?;
        Ok(Self { client })
    }
}

/// Run a blocking Redis operation off the async runtime's worker threads,
/// mirroring [`crate::db::run_on_postgres`]. The closure opens its own
/// connection from the (cheaply cloned) client it captures.
async fn run_on_redis<T, F>(op: F) -> Result<T, RepositoryError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RepositoryError> + Send + 'static,
{
    tokio::task::spawn_blocking(op)
        .await
        .map_err(|err| RepositoryError::Unexpected(format!("blocking task failed: {err}")))?
}

#[async_trait::async_trait]
impl SessionRepository for RedisSessionRepository {
    async fn create(&self, session: Session) -> Result<Session, RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            // Native TTL: Redis evicts the session at expires_at. Clamped to
            // a minimum of one second so even an already-expired session gets
            // a TTL instead of living forever.
            let ttl = (session.expires_at - Utc::now()).num_seconds().max(1);
            let key = session_key(&session.token_hash);
            for (name, value) in [
                ("id", session.id.0.to_string()),
                ("user_id", session.user_id.0.to_string()),
                ("token_hash", session.token_hash.clone()),
                ("created_at", ts_to_str(session.created_at)),
                ("expires_at", ts_to_str(session.expires_at)),
                ("last_seen_at", ts_to_str(session.last_seen_at)),
            ] {
                let _: usize = conn.hset(&key, name, value).map_err(map_redis_error)?;
            }
            let _: bool = conn.expire(&key, ttl).map_err(map_redis_error)?;
            let id_key = session_id_key(&session.id);
            let _: () = conn.set(&id_key, &session.token_hash).map_err(map_redis_error)?;
            let _: bool = conn.expire(&id_key, ttl).map_err(map_redis_error)?;
            // The index set has no TTL: stale members are harmless (their
            // session keys are already evicted) and get removed the next
            // time delete_all_for_user runs for this user.
            let _: usize = conn
                .sadd(user_sessions_key(&session.user_id), &session.token_hash)
                .map_err(map_redis_error)?;
            Ok(session)
        })
        .await
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<Session>, RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            // A missing (or already evicted) key is a plain `None`, not an error.
            let fields: HashMap<String, String> = conn
                .hgetall(&session_key(&token_hash))
                .map_err(map_redis_error)?;
            if fields.is_empty() {
                return Ok(None);
            }
            session_from_hash(fields).map(Some)
        })
        .await
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<Session>, RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            let token_hashes: Vec<String> = conn
                .smembers(user_sessions_key(&user_id))
                .map_err(map_redis_error)?;
            // ponytail: one HGETALL per session (N+1 reads); fine while a
            // user holds only a handful of sessions, batch with MGET/pipeline
            // if that changes.
            let mut sessions = Vec::new();
            for token_hash in token_hashes {
                let fields: HashMap<String, String> = conn
                    .hgetall(&session_key(&token_hash))
                    .map_err(map_redis_error)?;
                // Evicted sessions leave stale set members behind; skip them.
                if !fields.is_empty() {
                    sessions.push(session_from_hash(fields)?);
                }
            }
            Ok(sessions)
        })
        .await
    }

    async fn delete(&self, id: SessionId) -> Result<(), RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            let id_key = session_id_key(&id);
            let token_hash: Option<String> = conn.get(&id_key).map_err(map_redis_error)?;
            let Some(token_hash) = token_hash else {
                return Err(RepositoryError::NotFound);
            };
            let key = session_key(&token_hash);
            // The user id is needed to remove the session from the index set.
            let user_id: Option<String> = conn.hget(&key, "user_id").map_err(map_redis_error)?;
            let _: usize = conn.del(&key).map_err(map_redis_error)?;
            let _: usize = conn.del(&id_key).map_err(map_redis_error)?;
            if let Some(user_id) = user_id {
                let set_key = format!("user_sessions:{user_id}");
                let _: usize = conn.srem(&set_key, &token_hash).map_err(map_redis_error)?;
            }
            Ok(())
        })
        .await
    }

    async fn delete_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            let token_hashes: Vec<String> = conn
                .smembers(user_sessions_key(&user_id))
                .map_err(map_redis_error)?;
            for token_hash in &token_hashes {
                // The `session_id:*` mapping keys carry the same TTL as their
                // session keys and evict with them, so only the session keys
                // and the index need explicit removal.
                let _: usize = conn.del(session_key(token_hash)).map_err(map_redis_error)?;
            }
            let _: usize = conn.del(user_sessions_key(&user_id)).map_err(map_redis_error)?;
            Ok(())
        })
        .await
    }

    async fn touch_last_seen(
        &self,
        id: SessionId,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let client = self.client.clone();
        run_on_redis(move || {
            let mut conn = client.get_connection().map_err(map_redis_error)?;
            let token_hash: Option<String> = conn
                .get(&session_id_key(&id))
                .map_err(map_redis_error)?;
            let Some(token_hash) = token_hash else {
                return Err(RepositoryError::NotFound);
            };
            let key = session_key(&token_hash);
            // HSET on a missing key would resurrect the session without its
            // TTL, so check existence first.
            let exists: bool = conn.exists(&key).map_err(map_redis_error)?;
            if !exists {
                return Err(RepositoryError::NotFound);
            }
            let _: usize = conn
                .hset(&key, "last_seen_at", ts_to_str(last_seen_at))
                .map_err(map_redis_error)?;
            Ok(())
        })
        .await
    }
}
