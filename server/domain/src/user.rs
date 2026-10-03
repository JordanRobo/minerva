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
