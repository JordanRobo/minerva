//! Sessions: server-side records of users' authenticated logins.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::user::UserId;

/// Identifier for a [`Session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Create a fresh identifier for a session that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// An authenticated login of a user, valid until [`Session::expires_at`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    /// The hashed session token. The raw token is only ever shown to the
    /// client once, at creation; only its hash is stored.
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

impl Session {
    /// Whether this session's validity window has ended as of `now`.
    ///
    /// A session is expired at the exact moment of `expires_at`, not only
    /// after it.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn timestamp_at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(hour, minute, second)
            .expect("valid time")
            .and_utc()
    }

    fn test_session(expires_at: DateTime<Utc>) -> Session {
        let created_at = timestamp_at(8, 0, 0);
        Session {
            id: SessionId::new(),
            user_id: UserId::new(),
            token_hash: "hash".to_owned(),
            created_at,
            expires_at,
            last_seen_at: created_at,
        }
    }

    #[test]
    fn session_not_expired_before_expiry() {
        let session = test_session(timestamp_at(9, 0, 0));
        assert!(!session.is_expired(timestamp_at(8, 59, 59)));
    }

    #[test]
    fn session_expired_at_exact_expiry() {
        let session = test_session(timestamp_at(9, 0, 0));
        assert!(session.is_expired(timestamp_at(9, 0, 0)));
    }

    #[test]
    fn session_expired_after_expiry() {
        let session = test_session(timestamp_at(9, 0, 0));
        assert!(session.is_expired(timestamp_at(9, 0, 1)));
    }
}
