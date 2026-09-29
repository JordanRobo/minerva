//! Users: the people who can log in to Minerva.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

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
    /// The hashed password. Plaintext passwords are never stored or kept in
    /// memory past the moment of verification.
    pub password_hash: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
