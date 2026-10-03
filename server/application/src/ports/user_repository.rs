//! Persistence port for [`User`]s.

use domain::{Role, User, UserId};

use crate::ports::RepositoryError;

/// An access change an administrator applies to a user: the role, or the
/// active state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessChange {
    /// Set the user's role to `role`.
    Role(Role),
    /// Deactivate the account (the caller revokes its sessions).
    Deactivate,
    /// Reactivate a deactivated account.
    Reactivate,
}

/// Why [`UserRepository::apply_access_change`] did not apply the change.
#[derive(Debug)]
pub enum AccessChangeError {
    /// No user has this id.
    NotFound,
    /// The change would leave the system without an active admin.
    LastAdmin,
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl From<RepositoryError> for AccessChangeError {
    fn from(error: RepositoryError) -> Self {
        AccessChangeError::Repository(error)
    }
}

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: User) -> Result<User, RepositoryError>;

    /// Insert `user` only if the users table is completely empty, atomically:
    /// implementations count the rows and insert under a lock in one
    /// transaction, so two concurrent first-admin attempts cannot both win.
    /// Returns `Some(user)` when it inserted, `None` when any user already
    /// exists — deactivated or passwordless ones count too.
    async fn create_if_no_users(&self, user: User) -> Result<Option<User>, RepositoryError>;

    /// Returns `None` if no user has this id.
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;

    /// Returns `None` if no user has this email. Lookups are case-insensitive:
    /// the email is lowercased before querying.
    async fn find_by_email(&self, email: String) -> Result<Option<User>, RepositoryError>;

    async fn list(&self) -> Result<Vec<User>, RepositoryError>;

    async fn update(&self, user: User) -> Result<User, RepositoryError>;

    /// Atomically apply an access change to `target`, returning the user as
    /// stored afterwards.
    ///
    /// The "at least one active admin" invariant is enforced here rather than
    /// by a check-then-write in the caller: the implementation re-reads the
    /// target and counts the other active admins under a lock inside one
    /// transaction, so concurrent changes serialize instead of racing.
    /// Re-applying the current state (the same role, an already deactivated
    /// or already active account) is a successful no-op.
    async fn apply_access_change(
        &self,
        target: UserId,
        change: AccessChange,
    ) -> Result<User, AccessChangeError>;
}
