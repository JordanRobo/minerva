//! Persistence port for [`UserIdentity`]s.

use domain::{UserId, UserIdentity};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait UserIdentityRepository: Send + Sync {
    async fn create(&self, identity: UserIdentity) -> Result<UserIdentity, RepositoryError>;

    /// Returns `None` if no identity has this (issuer, subject) pair.
    async fn find_by_issuer_and_subject(
        &self,
        issuer: String,
        subject: String,
    ) -> Result<Option<UserIdentity>, RepositoryError>;

    /// All identities linked to a user, oldest first.
    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<UserIdentity>, RepositoryError>;
}
