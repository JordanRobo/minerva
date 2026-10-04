//! Persistence port for [`AccountToken`]s: the single-use links behind
//! invites and password resets (roadmap 2.6).

use chrono::{DateTime, Utc};
use domain::{AccountToken, AccountTokenId, User, UserId};

use crate::ports::RepositoryError;

/// The result of trying to accept an invite with a token.
#[derive(Debug)]
pub enum AcceptInviteOutcome {
    /// The token was usable and the invited account was created.
    Accepted(User),
    /// No token has this id, or it is already consumed, revoked or expired.
    TokenUnusable,
    /// A user with the invited email already exists; the whole step rolled
    /// back, so the token was left unconsumed.
    EmailTaken,
}

/// The result of trying to reset a password with a token.
#[derive(Debug)]
pub enum ResetPasswordOutcome {
    /// The password was set and the token consumed.
    Done,
    /// No token has this id, or it is already consumed, revoked or expired.
    TokenUnusable,
    /// No user has the id the reset targets; the whole step rolled back, so
    /// the token was left unconsumed.
    UserNotFound,
}

#[async_trait::async_trait]
pub trait AccountTokenRepository: Send + Sync {
    /// Insert `token`, atomically revoking any live (unconsumed and unrevoked
    /// — expired or not) token for the same subject first: the same email for
    /// invites, the same user for resets. This is what makes re-issuing and
    /// "replace an expired invite" one atomic step instead of a
    /// revoke-then-insert that could interleave. A unique violation (two
    /// issuers racing on the same token hash) surfaces as `Conflict`.
    async fn issue(
        &self,
        token: AccountToken,
        now: DateTime<Utc>,
    ) -> Result<AccountToken, RepositoryError>;

    /// Returns `None` if no token has this hash.
    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<AccountToken>, RepositoryError>;

    /// Returns `None` if no token has this id.
    async fn find_by_id(&self, id: AccountTokenId)
    -> Result<Option<AccountToken>, RepositoryError>;

    /// All invite tokens, newest first, in every state.
    async fn list_invites(&self) -> Result<Vec<AccountToken>, RepositoryError>;

    /// The live (unconsumed, unrevoked, unexpired) invite for `email`, if any.
    /// Emails are stored normalized (trimmed, lowercased); the lookup
    /// normalizes the same way before querying.
    async fn find_pending_invite_for_email(
        &self,
        email: String,
        now: DateTime<Utc>,
    ) -> Result<Option<AccountToken>, RepositoryError>;

    /// Revoke a token so it can no longer be used. Idempotent: revoking an
    /// already consumed or revoked token is a successful no-op; `NotFound`
    /// when no token has this id.
    async fn revoke(&self, id: AccountTokenId, now: DateTime<Utc>) -> Result<(), RepositoryError>;

    /// Atomically claim the token (mark it consumed at `now`) if it is still
    /// usable — unconsumed, unrevoked and not yet expired. Returns whether
    /// *this* call claimed it; a missing or unusable token claims nothing.
    async fn consume(
        &self,
        id: AccountTokenId,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError>;

    /// Accept an invite: in one transaction claim the token and create
    /// `user`, the invited account. If the email is already taken the whole
    /// step rolls back — the token is not burned — and the outcome is
    /// [`AcceptInviteOutcome::EmailTaken`]; a missing or unusable token
    /// yields [`AcceptInviteOutcome::TokenUnusable`] without creating anyone.
    async fn accept_invite(
        &self,
        id: AccountTokenId,
        user: User,
        now: DateTime<Utc>,
    ) -> Result<AcceptInviteOutcome, RepositoryError>;

    /// Reset a password: in one transaction claim the token, set
    /// `password_hash` on `user_id`, and revoke any other live reset tokens
    /// for that user so an earlier link cannot be used afterwards. A missing
    /// user rolls everything back ([`ResetPasswordOutcome::UserNotFound`]);
    /// a missing or unusable token yields
    /// [`ResetPasswordOutcome::TokenUnusable`] without touching the user.
    async fn reset_password(
        &self,
        id: AccountTokenId,
        user_id: UserId,
        password_hash: String,
        now: DateTime<Utc>,
    ) -> Result<ResetPasswordOutcome, RepositoryError>;
}
