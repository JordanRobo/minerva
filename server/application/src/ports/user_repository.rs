//! Persistence port for [`User`]s.

use domain::{User, UserId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: User) -> Result<User, RepositoryError>;

    /// Returns `None` if no user has this id.
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;

    /// Returns `None` if no user has this email. Lookups are case-insensitive:
    /// the email is lowercased before querying.
    async fn find_by_email(&self, email: String) -> Result<Option<User>, RepositoryError>;

    async fn list(&self) -> Result<Vec<User>, RepositoryError>;

    async fn update(&self, user: User) -> Result<User, RepositoryError>;
}
