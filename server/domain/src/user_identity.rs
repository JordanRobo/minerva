//! Identities of a [`User`] at external identity providers: the link that
//! lets a user sign in through an OIDC provider instead of a password.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::user::UserId;

/// Identifier for a [`UserIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UserIdentityId(pub Uuid);

impl UserIdentityId {
    /// Create a fresh identifier for a user identity that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserIdentityId {
    fn default() -> Self {
        Self::new()
    }
}

/// The link between a [`User`] and an identity at an external provider.
///
/// A user is identified at the provider by the pair (`issuer`, `subject`):
/// the provider itself (the OIDC `issuer`) and the user's unique identifier
/// there (the `subject` claim). The pair is unique, so one external identity
/// can never be linked to two Minerva users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserIdentity {
    pub id: UserIdentityId,
    pub user_id: UserId,
    /// The identity provider (the OIDC `issuer`).
    pub issuer: String,
    /// The user's unique identifier at the provider.
    pub subject: String,
    /// The email the provider reported for the identity, if any.
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl UserIdentity {
    /// Link `user_id` to an identity at the given provider.
    pub fn new(
        user_id: UserId,
        issuer: String,
        subject: String,
        email: Option<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: UserIdentityId::new(),
            user_id,
            issuer,
            subject,
            email,
            created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, Utc};

    use super::*;

    fn test_timestamp() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 1, 15)
            .expect("valid date")
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    #[test]
    fn new_identity_keeps_the_given_fields_and_mints_a_fresh_id() {
        let user_id = UserId::new();
        let created_at = test_timestamp();
        let identity = UserIdentity::new(
            user_id,
            "https://idp.example".into(),
            "sub-123".into(),
            Some("user@example.com".into()),
            created_at,
        );

        assert_eq!(identity.user_id, user_id);
        assert_eq!(identity.issuer, "https://idp.example");
        assert_eq!(identity.subject, "sub-123");
        assert_eq!(identity.email, Some("user@example.com".into()));
        assert_eq!(identity.created_at, created_at);

        // Each call mints a fresh id, so two identities for the same user
        // and provider are distinct rows.
        let other = UserIdentity::new(
            user_id,
            "https://idp.example".into(),
            "sub-123".into(),
            None,
            created_at,
        );
        assert_ne!(identity.id, other.id);
    }
}
