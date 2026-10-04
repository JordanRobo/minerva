//! Account tokens: the single-use links behind invites and password resets
//! (roadmap 2.6). One token type serves both purposes so the two flows share
//! one storage and validation mechanism.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::role::Role;
use crate::user::UserId;

/// Identifier for an [`AccountToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct AccountTokenId(pub Uuid);

impl AccountTokenId {
    /// Create a fresh identifier for a token that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AccountTokenId {
    fn default() -> Self {
        Self::new()
    }
}

/// What an [`AccountToken`] grants to whoever presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountTokenKind {
    /// Invite `email` to create an account with `role`. No user exists for
    /// the email yet; the account is created when the invite is accepted.
    Invite { email: String, role: Role },
    /// Let the holder of `user_id` set a new password.
    PasswordReset { user_id: UserId },
}

/// A single-use link (invite or password reset).
///
/// Only the hash of the token is stored; the raw value is shown to the
/// client once, inside the link, and never kept server-side. Deliberately
/// not `Serialize`: a response that leaked this type would leak the hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountToken {
    pub id: AccountTokenId,
    pub kind: AccountTokenKind,
    /// The hashed token; see the type docs.
    pub token_hash: String,
    /// Who created this token (e.g. the admin who issued an invite); `None`
    /// when no specific user created it.
    pub created_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    /// When the token stops working if it has not been used or revoked.
    pub expires_at: DateTime<Utc>,
    /// When the token was used; `None` while it is still available.
    pub consumed_at: Option<DateTime<Utc>>,
    /// When an admin (or the system) cancelled the token before use;
    /// `None` while it has not been revoked.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// The lifecycle state of an [`AccountToken`] at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountTokenStatus {
    /// Not yet used, not revoked, not expired.
    Pending,
    /// Used exactly once; no further use is possible.
    Accepted,
    /// Cancelled before use.
    Revoked,
    /// Its validity window has ended without use.
    Expired,
}

impl AccountToken {
    /// The token's state as of `now`.
    ///
    /// Precedence: a used token is [`Accepted`] no matter what else happened
    /// to it; otherwise a revoked one stays [`Revoked`] even after its expiry
    /// (revocation was the deliberate act); only an untouched, out-of-date
    /// token is [`Expired`]. As with [`Session::is_expired`](crate::session::Session::is_expired),
    /// the token expires at the exact moment of `expires_at`, not only after.
    pub fn status(&self, now: DateTime<Utc>) -> AccountTokenStatus {
        if self.consumed_at.is_some() {
            return AccountTokenStatus::Accepted;
        }
        if self.revoked_at.is_some() {
            return AccountTokenStatus::Revoked;
        }
        if now >= self.expires_at {
            return AccountTokenStatus::Expired;
        }
        AccountTokenStatus::Pending
    }

    /// Whether this token may still be used as of `now`.
    pub fn is_pending(&self, now: DateTime<Utc>) -> bool {
        self.status(now) == AccountTokenStatus::Pending
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

    fn test_token(expires_at: DateTime<Utc>) -> AccountToken {
        let created_at = timestamp_at(8, 0, 0);
        AccountToken {
            id: AccountTokenId::new(),
            kind: AccountTokenKind::Invite {
                email: "invitee@example.com".to_owned(),
                role: Role::Staff,
            },
            token_hash: "hash".to_owned(),
            created_by: None,
            created_at,
            expires_at,
            consumed_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn fresh_token_is_pending() {
        let token = test_token(timestamp_at(9, 0, 0));
        assert_eq!(
            token.status(timestamp_at(8, 30, 0)),
            AccountTokenStatus::Pending
        );
        assert!(token.is_pending(timestamp_at(8, 30, 0)));
    }

    #[test]
    fn token_expires_at_exact_expiry() {
        let token = test_token(timestamp_at(9, 0, 0));
        assert_eq!(
            token.status(timestamp_at(9, 0, 0)),
            AccountTokenStatus::Expired
        );
        assert!(!token.is_pending(timestamp_at(9, 0, 0)));
    }

    #[test]
    fn token_expired_after_expiry() {
        let token = test_token(timestamp_at(9, 0, 0));
        assert_eq!(
            token.status(timestamp_at(9, 0, 1)),
            AccountTokenStatus::Expired
        );
    }

    #[test]
    fn consumed_token_is_accepted() {
        let mut token = test_token(timestamp_at(9, 0, 0));
        token.consumed_at = Some(timestamp_at(8, 30, 0));
        assert_eq!(
            token.status(timestamp_at(8, 45, 0)),
            AccountTokenStatus::Accepted
        );
        assert!(!token.is_pending(timestamp_at(8, 45, 0)));
    }

    #[test]
    fn revocation_beats_expiry() {
        let mut token = test_token(timestamp_at(9, 0, 0));
        token.revoked_at = Some(timestamp_at(8, 30, 0));
        assert_eq!(
            token.status(timestamp_at(10, 0, 0)),
            AccountTokenStatus::Revoked
        );
    }

    #[test]
    fn consumption_beats_revocation_and_expiry() {
        let mut token = test_token(timestamp_at(9, 0, 0));
        token.consumed_at = Some(timestamp_at(8, 30, 0));
        token.revoked_at = Some(timestamp_at(8, 45, 0));
        assert_eq!(
            token.status(timestamp_at(10, 0, 0)),
            AccountTokenStatus::Accepted
        );
    }
}
