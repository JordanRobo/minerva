//! Users: the people who can log in to Minerva.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::role::Role;

/// Identifier for a [`User`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UserId(pub Uuid);

impl UserId {
    /// Create a fresh identifier for a user that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

/// A person who can log in to Minerva.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: UserId,
    pub email: String,
    /// The hashed password, or `None` for a user who can only sign in via an
    /// external identity provider. Plaintext passwords are never stored or
    /// kept in memory past the moment of verification.
    pub password_hash: Option<String>,
    pub display_name: String,
    /// What this user is allowed to do; see [`Role::allows`].
    pub role: Role,
    /// Set by SSO sign-in while the user's role was recomputed from the IdP
    /// groups claim (D15). While any group rule exists, hand edits to such a
    /// role are rejected; with no rules the flag is inert.
    pub role_managed_by_sso: bool,
    /// Exempt from SSO role recomputation (D15); set on the 2.5 bootstrap
    /// admin and never by any other creation path.
    pub sso_role_exempt: bool,
    /// When an admin deactivated this account; `None` while it is active. A
    /// deactivated account cannot sign in and its sessions are revoked.
    pub deactivated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    /// Whether this account may sign in and use the system.
    pub fn is_active(&self) -> bool {
        self.deactivated_at.is_none()
    }
}

/// A short, safe view of a [`User`] for pickers and lists (D17): who they are
/// and whether the account can still sign in. Never carries the email or the
/// password hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserSummary {
    pub id: UserId,
    pub display_name: String,
    pub active: bool,
}

impl UserSummary {
    /// Build a summary from the stored values. A blank display name falls back
    /// to the local part of `email` (D17); `email` is used for that fallback
    /// only and is never kept on the summary.
    pub fn new(id: UserId, display_name: &str, email: &str, active: bool) -> Self {
        let display_name = if display_name.trim().is_empty() {
            email.split('@').next().unwrap_or("").to_owned()
        } else {
            display_name.to_owned()
        };
        Self {
            id,
            display_name,
            active,
        }
    }
}
