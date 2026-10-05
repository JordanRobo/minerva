//! Account links: the use cases behind invites and password resets
//! (roadmap 2.6, step 2).
//!
//! [`AccountLinkService`] is the single place the link rules live: which
//! emails may be invited, when a link stops working, and what accepting or
//! resetting does to the account. Raw tokens are generated here and returned
//! once, inside the link; only their hashes are ever persisted, so neither a
//! leaked database nor a logged response can be turned into a live link.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use domain::{
    AccountToken, AccountTokenId, AccountTokenKind, AccountTokenStatus, Role, User, UserId,
};

use crate::auth::{SessionService, normalize_email, validate_email};
use crate::bootstrap::MIN_PASSWORD_LENGTH;
use crate::ports::{
    AcceptInviteOutcome, AccountEmailSender, AccountTokenRepository, PasswordHashError,
    PasswordHasher, RepositoryError, ResetPasswordOutcome, SessionTokens, UserRepository,
};

/// How long an invite link stays valid before it stops working.
pub const INVITE_TTL: Duration = Duration::days(7);

/// How long a password-reset link stays valid before it stops working.
pub const PASSWORD_RESET_TTL: Duration = Duration::hours(24);

const ACCEPT_INVITE_PATH: &str = "/accept-invite";
const RESET_PASSWORD_PATH: &str = "/reset-password";

/// A failure of an account-link operation, in plain language for the caller.
#[derive(Debug)]
pub enum AccountLinkError {
    /// The email address failed the shape check; the message says why.
    InvalidEmail(String),
    /// A user with that email already exists.
    EmailAlreadyRegistered,
    /// An unexpired invite for that email is already out there; re-issue it.
    InvitePending,
    /// The invite was already accepted; it cannot be revoked or re-issued.
    InviteAlreadyAccepted,
    /// No account link has this id.
    NotFound,
    /// The account signs in only through an external identity provider.
    NoPasswordAccount,
    /// The account is deactivated.
    AccountDeactivated,
    /// The token is unknown or no longer usable — deliberately one error for
    /// all of them, so the answer never hints which.
    InvalidToken,
    /// The password is shorter than [`MIN_PASSWORD_LENGTH`].
    PasswordTooShort,
    /// Hashing the password failed.
    Hash(PasswordHashError),
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl std::fmt::Display for AccountLinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountLinkError::InvalidEmail(why) => write!(f, "not a valid email address: {why}"),
            AccountLinkError::EmailAlreadyRegistered => {
                write!(f, "an account with that email already exists")
            }
            AccountLinkError::InvitePending => {
                write!(f, "an unexpired invite for that email is already pending")
            }
            AccountLinkError::InviteAlreadyAccepted => {
                write!(f, "this invite has already been accepted")
            }
            AccountLinkError::NotFound => write!(f, "no such account link"),
            AccountLinkError::NoPasswordAccount => write!(
                f,
                "this account signs in through an external provider and has no password to reset"
            ),
            AccountLinkError::AccountDeactivated => write!(f, "this account is deactivated"),
            AccountLinkError::InvalidToken => write!(f, "this link is not valid"),
            AccountLinkError::PasswordTooShort => {
                write!(
                    f,
                    "the password must be at least {MIN_PASSWORD_LENGTH} characters"
                )
            }
            AccountLinkError::Hash(err) => write!(f, "could not hash the password: {err}"),
            AccountLinkError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AccountLinkError {}

/// A freshly issued account link: the stored token, the one-time link built
/// from its raw token, and whether the email actually went out.
pub struct IssuedLink<T> {
    /// The token as stored (its hash only — never the raw value).
    pub item: T,
    /// The link with the raw token in it; shown to the caller exactly once
    /// and never logged (hence the redacted [`std::fmt::Debug`]).
    pub link: String,
    /// Whether the email delivery succeeded; when false the caller must show
    /// `link` so it can be shared by hand.
    pub emailed: bool,
}

impl<T: std::fmt::Debug> std::fmt::Debug for IssuedLink<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedLink")
            .field("item", &self.item)
            .field("link", &"<redacted>")
            .field("emailed", &self.emailed)
            .finish()
    }
}

/// What an account link grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPurpose {
    /// Create a new account with the invited role.
    Invite,
    /// Set a new password on an existing account.
    PasswordReset,
}

/// What a link is for and when it stops working, as shown to its holder
/// before they act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub purpose: LinkPurpose,
    /// Who the link is for: the invited address, or the reset target's email.
    pub email: String,
    /// The role the invite grants; `None` for password resets.
    pub role: Option<Role>,
    pub expires_at: DateTime<Utc>,
}

/// Issues and consumes the one-time links behind invites and password
/// resets. Every rule lives here, not in the HTTP handlers (like
/// [`UserAdminService`](crate::user_admin::UserAdminService)).
pub struct AccountLinkService {
    tokens: Arc<dyn AccountTokenRepository>,
    users: Arc<dyn UserRepository>,
    hasher: Arc<dyn PasswordHasher>,
    /// The raw-token generator and one-way hash, shared with the session
    /// service's port so both kinds of token come from one implementation.
    session_tokens: Arc<dyn SessionTokens>,
    sessions: SessionService,
    email: Arc<dyn AccountEmailSender>,
    /// The already-validated public origin without trailing slash; when empty
    /// the links are site-relative paths.
    link_base_url: String,
}

impl AccountLinkService {
    pub fn new(
        tokens: Arc<dyn AccountTokenRepository>,
        users: Arc<dyn UserRepository>,
        hasher: Arc<dyn PasswordHasher>,
        session_tokens: Arc<dyn SessionTokens>,
        sessions: SessionService,
        email: Arc<dyn AccountEmailSender>,
        link_base_url: String,
    ) -> Self {
        Self {
            tokens,
            users,
            hasher,
            session_tokens,
            sessions,
            email,
            link_base_url,
        }
    }

    /// Invite `email` to create an account with `role`. The email is
    /// normalized and shape-checked; a registered address and an unexpired
    /// pending invite are both refused (the admin should re-issue the latter).
    /// The token's hash is stored through [`AccountTokenRepository::issue`],
    /// which also revokes whatever live token the subject still has, so an
    /// expired invite is simply replaced. Delivery is best-effort: a failed
    /// send only flips `emailed` to false — the link is never lost and never
    /// logged.
    pub async fn create_invite(
        &self,
        actor: &User,
        email: &str,
        role: Role,
    ) -> Result<IssuedLink<AccountToken>, AccountLinkError> {
        let email = normalize_email(email);
        if let Some(why) = validate_email(&email) {
            return Err(AccountLinkError::InvalidEmail(why));
        }
        let now = Utc::now();
        // A registered account and a pending invite are different answers:
        // the admin must not be told "invite pending" for an address that
        // already has an account.
        if self
            .users
            .find_by_email(email.clone())
            .await
            .map_err(AccountLinkError::Repository)?
            .is_some()
        {
            return Err(AccountLinkError::EmailAlreadyRegistered);
        }
        // Only an unexpired pending invite blocks a new one; an expired or
        // revoked one is replaced by the issue below.
        if self
            .tokens
            .find_pending_invite_for_email(email.clone(), now)
            .await
            .map_err(AccountLinkError::Repository)?
            .is_some()
        {
            return Err(AccountLinkError::InvitePending);
        }
        let (token, raw) = self.fresh_token(
            AccountTokenKind::Invite {
                email: email.clone(),
                role,
            },
            Some(actor.id),
            now,
            INVITE_TTL,
        );
        let issued = self
            .tokens
            .issue(token, now)
            .await
            .map_err(AccountLinkError::Repository)?;
        let link = self.link(ACCEPT_INVITE_PATH, &raw);
        let emailed = self
            .email
            .send_invite(&email, &link, role, issued.expires_at)
            .await
            .is_ok();
        Ok(IssuedLink {
            item: issued,
            link,
            emailed,
        })
    }

    /// Every invite, newest first, in whatever state — for the admin list
    /// view.
    pub async fn list_invites(&self) -> Result<Vec<AccountToken>, AccountLinkError> {
        self.tokens
            .list_invites()
            .await
            .map_err(AccountLinkError::Repository)
    }

    /// Cancel an invite before it is used, returning the updated token.
    /// Revoking an already-revoked invite is a successful no-op; an accepted
    /// one cannot be undone. A password-reset token's id is "no such invite",
    /// as in [`reissue`](Self::reissue_invite).
    pub async fn revoke_invite(
        &self,
        id: AccountTokenId,
    ) -> Result<AccountToken, AccountLinkError> {
        let mut token = self.find_token(id).await?;
        if !matches!(token.kind, AccountTokenKind::Invite { .. }) {
            return Err(AccountLinkError::NotFound);
        }
        let now = Utc::now();
        match token.status(now) {
            AccountTokenStatus::Accepted => Err(AccountLinkError::InviteAlreadyAccepted),
            AccountTokenStatus::Revoked => Ok(token),
            _ => {
                self.tokens
                    .revoke(id, now)
                    .await
                    .map_err(AccountLinkError::Repository)?;
                token.revoked_at = Some(now);
                Ok(token)
            }
        }
    }

    /// Replace an unused invite with a fresh token and expiry — the old link
    /// stops working. Refused once the invite has been accepted, or if a user
    /// with the invited email has registered in the meantime.
    pub async fn reissue_invite(
        &self,
        actor: &User,
        id: AccountTokenId,
    ) -> Result<IssuedLink<AccountToken>, AccountLinkError> {
        let token = self.find_token(id).await?;
        if token.status(Utc::now()) == AccountTokenStatus::Accepted {
            return Err(AccountLinkError::InviteAlreadyAccepted);
        }
        // Only invites can be re-issued; a reset token id is "no such invite".
        let AccountTokenKind::Invite { email, role } = token.kind else {
            return Err(AccountLinkError::NotFound);
        };
        if self
            .users
            .find_by_email(email.clone())
            .await
            .map_err(AccountLinkError::Repository)?
            .is_some()
        {
            return Err(AccountLinkError::EmailAlreadyRegistered);
        }
        let now = Utc::now();
        let (fresh, raw) = self.fresh_token(
            AccountTokenKind::Invite {
                email: email.clone(),
                role,
            },
            Some(actor.id),
            now,
            INVITE_TTL,
        );
        // `issue` revokes the old token for this subject in the same step.
        let issued = self
            .tokens
            .issue(fresh, now)
            .await
            .map_err(AccountLinkError::Repository)?;
        let link = self.link(ACCEPT_INVITE_PATH, &raw);
        let emailed = self
            .email
            .send_invite(&email, &link, role, issued.expires_at)
            .await
            .is_ok();
        Ok(IssuedLink {
            item: issued,
            link,
            emailed,
        })
    }

    /// Let an account set a new password: issues a reset link for `user_id`,
    /// replacing any live one the user already has. Refused for deactivated
    /// accounts and for SSO-only accounts that have no password to reset.
    pub async fn issue_password_reset(
        &self,
        actor: &User,
        user_id: UserId,
    ) -> Result<IssuedLink<AccountToken>, AccountLinkError> {
        let user = self
            .users
            .find_by_id(user_id)
            .await
            .map_err(AccountLinkError::Repository)?
            .ok_or(AccountLinkError::NotFound)?;
        if !user.is_active() {
            return Err(AccountLinkError::AccountDeactivated);
        }
        if user.password_hash.is_none() {
            return Err(AccountLinkError::NoPasswordAccount);
        }
        let now = Utc::now();
        let (token, raw) = self.fresh_token(
            AccountTokenKind::PasswordReset { user_id },
            Some(actor.id),
            now,
            PASSWORD_RESET_TTL,
        );
        // `issue` revokes the user's other live reset token in the same step.
        let issued = self
            .tokens
            .issue(token, now)
            .await
            .map_err(AccountLinkError::Repository)?;
        let link = self.link(RESET_PASSWORD_PATH, &raw);
        let emailed = self
            .email
            .send_password_reset(&user.email, &link, issued.expires_at)
            .await
            .is_ok();
        Ok(IssuedLink {
            item: issued,
            link,
            emailed,
        })
    }

    /// What a link is for, shown to its holder before they act on it. Every
    /// unusable link — unknown, expired, consumed or revoked — is the same
    /// [`AccountLinkError::InvalidToken`], so the answer never hints which.
    pub async fn inspect(&self, raw_token: &str) -> Result<LinkInfo, AccountLinkError> {
        let token = self.pending_token(raw_token).await?;
        match &token.kind {
            AccountTokenKind::Invite { email, role } => Ok(LinkInfo {
                purpose: LinkPurpose::Invite,
                email: email.clone(),
                role: Some(*role),
                expires_at: token.expires_at,
            }),
            AccountTokenKind::PasswordReset { user_id } => {
                // The account's email is what the holder sees; a gone account
                // is just another invalid link.
                let user = self
                    .users
                    .find_by_id(*user_id)
                    .await
                    .map_err(AccountLinkError::Repository)?
                    .ok_or(AccountLinkError::InvalidToken)?;
                Ok(LinkInfo {
                    purpose: LinkPurpose::PasswordReset,
                    email: user.email,
                    role: None,
                    expires_at: token.expires_at,
                })
            }
        }
    }

    /// Create the account an invite grants. The link must be an unexpired
    /// invite; the password is hashed and stored only as its hash, and a
    /// blank display name falls back to the email's local part (as SSO does).
    /// If the email was registered in the meantime the repository rolls its
    /// claim back — the link survives the failed attempt.
    pub async fn accept_invite(
        &self,
        raw_token: &str,
        password: &str,
        display_name: Option<String>,
    ) -> Result<User, AccountLinkError> {
        let now = Utc::now();
        let token = self.pending_token(raw_token).await?;
        let (email, role) = match &token.kind {
            // A reset link is not an invite; the single InvalidToken keeps
            // the answer from hinting which kind of link this was.
            AccountTokenKind::Invite { email, role } => (email.clone(), *role),
            AccountTokenKind::PasswordReset { .. } => return Err(AccountLinkError::InvalidToken),
        };
        if password.chars().count() < MIN_PASSWORD_LENGTH {
            return Err(AccountLinkError::PasswordTooShort);
        }
        let password_hash = self
            .hasher
            .hash(password)
            .await
            .map_err(AccountLinkError::Hash)?;
        let display_name = display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                email
                    .split('@')
                    .next()
                    .map(str::to_owned)
                    .expect("validated above")
            });
        let user = User {
            id: UserId::new(),
            email,
            password_hash: Some(password_hash),
            display_name,
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        };
        match self
            .tokens
            .accept_invite(token.id, user, now)
            .await
            .map_err(AccountLinkError::Repository)?
        {
            AcceptInviteOutcome::Accepted(user) => Ok(user),
            // The email was registered between the invite and its acceptance;
            // the repository rolled the claim back, so the link is unburned.
            AcceptInviteOutcome::EmailTaken => Err(AccountLinkError::EmailAlreadyRegistered),
            AcceptInviteOutcome::TokenUnusable => Err(AccountLinkError::InvalidToken),
        }
    }

    /// Set a new password with a reset link: the link must be an unexpired
    /// reset for an existing, active account. The password change is committed
    /// before the sessions go, so a failure there cannot leave the old
    /// password usable without also leaving live sessions behind — and every
    /// session of the user is revoked either way, forcing a sign-in with the
    /// new password.
    pub async fn reset_password(
        &self,
        raw_token: &str,
        new_password: &str,
    ) -> Result<(), AccountLinkError> {
        let now = Utc::now();
        let token = self.pending_token(raw_token).await?;
        let (token_id, user_id) = match token.kind {
            AccountTokenKind::PasswordReset { user_id } => (token.id, user_id),
            // An invite link is not a reset link; one opaque error for both.
            AccountTokenKind::Invite { .. } => return Err(AccountLinkError::InvalidToken),
        };
        // The account must still exist and be active; a gone or deactivated
        // account gets the same answer as a bad link.
        let active = self
            .users
            .find_by_id(user_id)
            .await
            .map_err(AccountLinkError::Repository)?
            .filter(User::is_active)
            .is_some();
        if !active {
            return Err(AccountLinkError::InvalidToken);
        }
        if new_password.chars().count() < MIN_PASSWORD_LENGTH {
            return Err(AccountLinkError::PasswordTooShort);
        }
        let password_hash = self
            .hasher
            .hash(new_password)
            .await
            .map_err(AccountLinkError::Hash)?;
        match self
            .tokens
            .reset_password(token_id, user_id, password_hash, now)
            .await
            .map_err(AccountLinkError::Repository)?
        {
            ResetPasswordOutcome::Done => {
                self.sessions
                    .revoke_all_for_user(user_id)
                    .await
                    .map_err(AccountLinkError::Repository)?;
                Ok(())
            }
            // The account vanished between the check and the transaction.
            ResetPasswordOutcome::TokenUnusable | ResetPasswordOutcome::UserNotFound => {
                Err(AccountLinkError::InvalidToken)
            }
        }
    }

    /// The token behind `raw_token`, when it is still usable; otherwise the
    /// single [`AccountLinkError::InvalidToken`].
    async fn pending_token(&self, raw_token: &str) -> Result<AccountToken, AccountLinkError> {
        let token = self
            .tokens
            .find_by_token_hash(self.session_tokens.hash(raw_token))
            .await
            .map_err(AccountLinkError::Repository)?;
        match token.filter(|token| token.is_pending(Utc::now())) {
            Some(token) => Ok(token),
            None => Err(AccountLinkError::InvalidToken),
        }
    }

    /// The stored token with this id, or [`AccountLinkError::NotFound`].
    async fn find_token(&self, id: AccountTokenId) -> Result<AccountToken, AccountLinkError> {
        self.tokens
            .find_by_id(id)
            .await
            .map_err(AccountLinkError::Repository)?
            .ok_or(AccountLinkError::NotFound)
    }

    /// A fresh token of `kind` expiring after `ttl`, with the raw value that
    /// was hashed into it. The raw value is returned once and never stored.
    fn fresh_token(
        &self,
        kind: AccountTokenKind,
        created_by: Option<UserId>,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> (AccountToken, String) {
        let raw = self.session_tokens.generate();
        let token = AccountToken {
            id: AccountTokenId::new(),
            kind,
            token_hash: self.session_tokens.hash(&raw),
            created_by,
            created_at: now,
            expires_at: now + ttl,
            consumed_at: None,
            revoked_at: None,
        };
        (token, raw)
    }

    /// The one-time link for `path` and its raw token. With an empty
    /// `link_base_url` the result is a site-relative path, which the frontend
    /// resolves against its own origin.
    fn link(&self, path: &str, raw_token: &str) -> String {
        format!("{}{path}?token={raw_token}", self.link_base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::{
        DeterministicTokens, FailingUserRepository, FakePasswordHasher,
        InMemoryAccountTokenRepository, InMemorySessionRepository, InMemoryUserRepository,
        RecordingEmailSender,
    };
    use crate::ports::AccountTokenRepository;

    /// A [`PasswordHasher`] that always fails, for the `Hash` error path.
    struct FailingHasher;

    #[async_trait::async_trait]
    impl PasswordHasher for FailingHasher {
        async fn hash(&self, _password: &str) -> Result<String, PasswordHashError> {
            Err(PasswordHashError::OperationFailed(
                "faking a hash failure".to_owned(),
            ))
        }

        async fn verify(&self, _password: &str, _hash: &str) -> Result<bool, PasswordHashError> {
            Err(PasswordHashError::OperationFailed(
                "faking a hash failure".to_owned(),
            ))
        }

        async fn verify_dummy(&self, _password: &str) -> Result<bool, PasswordHashError> {
            Err(PasswordHashError::OperationFailed(
                "faking a hash failure".to_owned(),
            ))
        }
    }

    fn user(email: &str, role: Role, password_hash: Option<&str>, deactivated: bool) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: normalize_email(email),
            password_hash: password_hash.map(str::to_owned),
            display_name: "Test".to_owned(),
            role,
            deactivated_at: deactivated.then_some(now),
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn admin() -> User {
        user(
            "admin@example.com",
            Role::Admin,
            Some("hash-of-admin"),
            false,
        )
    }

    /// A service over in-memory fakes; the stores come back so tests can seed
    /// and inspect them.
    fn service(
        link_base_url: &str,
    ) -> (
        AccountLinkService,
        Arc<InMemoryUserRepository>,
        Arc<InMemoryAccountTokenRepository>,
        Arc<RecordingEmailSender>,
        Arc<InMemorySessionRepository>,
    ) {
        let users = Arc::new(InMemoryUserRepository::new());
        let tokens = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let sessions = Arc::new(InMemorySessionRepository::new());
        let email = Arc::new(RecordingEmailSender::new());
        let service = AccountLinkService::new(
            tokens.clone(),
            users.clone(),
            Arc::new(FakePasswordHasher),
            Arc::new(DeterministicTokens::default()),
            SessionService::new(
                sessions.clone(),
                Arc::new(DeterministicTokens::default()),
                SessionService::DEFAULT_SESSION_TTL,
            ),
            email.clone(),
            link_base_url.to_owned(),
        );
        (service, users, tokens, email, sessions)
    }

    /// An invite token seeded directly into the store — e.g. an expired one
    /// the service would never create. Its raw value is "seeded", matching
    /// the deterministic hash.
    fn seeded_invite(email: &str, expires_at: DateTime<Utc>) -> AccountToken {
        AccountToken {
            id: AccountTokenId::new(),
            kind: AccountTokenKind::Invite {
                email: normalize_email(email),
                role: Role::Staff,
            },
            token_hash: "hash-of-seeded".to_owned(),
            created_by: None,
            created_at: expires_at - Duration::days(1),
            expires_at,
            consumed_at: None,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn create_invite_stores_only_the_hash_and_builds_the_link() {
        let (service, _users, tokens, email, _sessions) = service("https://minerva.example.com");
        let actor = admin();
        let issued = service
            .create_invite(&actor, "  New@Example.COM ", Role::Staff)
            .await
            .unwrap();

        assert_eq!(
            issued.item.kind,
            AccountTokenKind::Invite {
                email: "new@example.com".to_owned(),
                role: Role::Staff
            }
        );
        assert_eq!(issued.item.created_by, Some(actor.id));
        // The first deterministic token is "token-1"; only its hash is stored.
        assert_eq!(issued.item.token_hash, "hash-of-token-1");
        assert_eq!(
            issued.link,
            "https://minerva.example.com/accept-invite?token=token-1"
        );
        assert!(issued.emailed);
        let by_raw = tokens
            .find_by_token_hash("token-1".to_owned())
            .await
            .unwrap();
        assert!(by_raw.is_none(), "the raw token must not be stored");
        // The email carries the same one-time link.
        assert_eq!(email.sent_links(), vec![issued.link.clone()]);
    }

    #[tokio::test]
    async fn create_invite_rejects_invalid_emails() {
        let (service, _users, tokens, _email, _sessions) = service("");
        let actor = admin();
        for email in ["not-an-email", "a@b@c", "@example.com", "new@"] {
            assert!(matches!(
                service.create_invite(&actor, email, Role::Staff).await,
                Err(AccountLinkError::InvalidEmail(_))
            ));
        }
        // Nothing was stored.
        assert_eq!(tokens.list_invites().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn create_invite_refuses_a_registered_email() {
        let (service, users, _tokens, _email, _sessions) = service("");
        users
            .create(user("taken@example.com", Role::Staff, Some("h"), false))
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_invite(&admin(), "Taken@Example.com", Role::Staff)
                .await,
            Err(AccountLinkError::EmailAlreadyRegistered)
        ));
    }

    #[tokio::test]
    async fn a_duplicate_unexpired_invite_is_refused_but_an_expired_one_is_replaced() {
        let (service, _users, tokens, _email, _sessions) = service("");
        let actor = admin();
        service
            .create_invite(&actor, "dup@example.com", Role::Staff)
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_invite(&actor, "dup@example.com", Role::Staff)
                .await,
            Err(AccountLinkError::InvitePending)
        ));

        // An expired invite for another email does not block: the new issue
        // revokes it and the fresh link works.
        let expired = seeded_invite("late@example.com", Utc::now() - Duration::hours(1));
        tokens.insert(expired.clone());
        service
            .create_invite(&actor, "late@example.com", Role::Staff)
            .await
            .unwrap();
        let replaced = tokens.find_by_id(expired.id).await.unwrap().unwrap();
        assert_eq!(replaced.status(Utc::now()), AccountTokenStatus::Revoked);
        // The second successful issue got raw token "token-2".
        assert!(service.inspect("token-2").await.is_ok());
    }

    #[tokio::test]
    async fn reissue_invalidates_the_old_link_and_the_new_one_works() {
        let (service, _users, tokens, _email, _sessions) = service("");
        let actor = admin();
        let first = service
            .create_invite(&actor, "re@example.com", Role::Staff)
            .await
            .unwrap();
        let second = service.reissue_invite(&actor, first.item.id).await.unwrap();

        assert_ne!(second.link, first.link);
        // The old link is dead...
        assert!(matches!(
            service.inspect("token-1").await,
            Err(AccountLinkError::InvalidToken)
        ));
        // ...and the new one works.
        let user = service
            .accept_invite("token-2", "long-enough-pass", None)
            .await
            .unwrap();
        assert_eq!(user.email, "re@example.com");
        // The old token row is revoked, not deleted.
        let old = tokens.find_by_id(first.item.id).await.unwrap().unwrap();
        assert_eq!(old.status(Utc::now()), AccountTokenStatus::Revoked);
    }

    #[tokio::test]
    async fn accept_invite_creates_a_user_with_the_invites_role_and_cannot_be_reused() {
        let (service, users, _tokens, _email, _sessions) = service("");
        let actor = admin();
        service
            .create_invite(&actor, "new@example.com", Role::Staff)
            .await
            .unwrap();

        let user = service
            .accept_invite(
                "token-1",
                "long-enough-pass",
                Some("  New Person ".to_owned()),
            )
            .await
            .unwrap();
        assert_eq!(user.email, "new@example.com");
        assert_eq!(user.role, Role::Staff);
        assert_eq!(user.display_name, "New Person");
        assert!(user.deactivated_at.is_none());
        // D15: normal creations never set the SSO flags; only the bootstrap
        // admin is exempt from recomputation.
        assert!(!user.role_managed_by_sso);
        assert!(!user.sso_role_exempt);
        // The password is stored hashed — never the plaintext.
        assert_eq!(
            user.password_hash.as_deref(),
            Some("hash-of-long-enough-pass")
        );
        let stored = users
            .find_by_email("new@example.com".to_owned())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(stored.password_hash.as_deref(), Some("long-enough-pass"));

        // The token is single-use.
        assert!(matches!(
            service
                .accept_invite("token-1", "long-enough-pass", None)
                .await,
            Err(AccountLinkError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn accepting_with_a_short_password_leaves_the_token_usable() {
        let (service, _users, _tokens, _email, _sessions) = service("");
        service
            .create_invite(&admin(), "short@example.com", Role::Staff)
            .await
            .unwrap();
        assert!(matches!(
            service.accept_invite("token-1", "short", None).await,
            Err(AccountLinkError::PasswordTooShort)
        ));
        // The failed attempt must not have burned the link.
        assert!(service.inspect("token-1").await.is_ok());
    }

    #[tokio::test]
    async fn an_email_registered_after_the_invite_is_refused_and_the_token_stays_pending() {
        let (service, users, tokens, _email, _sessions) = service("");
        let issued = service
            .create_invite(&admin(), "late@example.com", Role::Staff)
            .await
            .unwrap();
        // The address registers through another path (e.g. SSO) after the
        // invite went out.
        users
            .create(user("late@example.com", Role::ReadOnly, None, false))
            .await
            .unwrap();

        assert!(matches!(
            service
                .accept_invite("token-1", "long-enough-pass", None)
                .await,
            Err(AccountLinkError::EmailAlreadyRegistered)
        ));
        // The failed attempt must not have burned the invite.
        let stored = tokens.find_by_id(issued.item.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Pending);
    }

    #[tokio::test]
    async fn reset_password_works_once_replaces_the_hash_and_revokes_sessions() {
        let (service, users, _tokens, _email, sessions) = service("");
        let actor = admin();
        let target = user("reset@example.com", Role::Staff, Some("hash-of-old"), false);
        users.create(target.clone()).await.unwrap();

        // A live session issued before the reset...
        let issuer = SessionService::new(
            sessions.clone(),
            Arc::new(DeterministicTokens::default()),
            SessionService::DEFAULT_SESSION_TTL,
        );
        let issued_session = issuer.issue(target.id).await.unwrap();

        let link = service
            .issue_password_reset(&actor, target.id)
            .await
            .unwrap();
        assert_eq!(link.link, "/reset-password?token=token-1");
        service
            .reset_password("token-1", "brand-new-pass")
            .await
            .unwrap();

        let stored = users.find_by_id(target.id).await.unwrap().unwrap();
        assert_eq!(
            stored.password_hash.as_deref(),
            Some("hash-of-brand-new-pass")
        );
        assert_ne!(stored.password_hash.as_deref(), Some("brand-new-pass"));
        // Every session of the user is gone.
        assert!(
            issuer
                .resolve(&issued_session.token)
                .await
                .unwrap()
                .is_none()
        );

        // The link is single-use.
        assert!(matches!(
            service.reset_password("token-1", "another-pass").await,
            Err(AccountLinkError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn issuing_a_second_reset_kills_the_first_link() {
        let (service, users, _tokens, _email, _sessions) = service("");
        let actor = admin();
        let target = user("two@example.com", Role::Staff, Some("h"), false);
        users.create(target.clone()).await.unwrap();

        service
            .issue_password_reset(&actor, target.id)
            .await
            .unwrap();
        let second = service
            .issue_password_reset(&actor, target.id)
            .await
            .unwrap();

        assert!(matches!(
            service.inspect("token-1").await,
            Err(AccountLinkError::InvalidToken)
        ));
        assert_eq!(second.link, "/reset-password?token=token-2");
        service
            .reset_password("token-2", "brand-new-pass")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn reset_is_refused_for_sso_only_and_deactivated_users() {
        let (service, users, _tokens, _email, _sessions) = service("");
        let actor = admin();
        let sso_only = user("sso@example.com", Role::Staff, None, false);
        let deactivated = user("off@example.com", Role::Staff, Some("h"), true);
        users.create(sso_only.clone()).await.unwrap();
        users.create(deactivated.clone()).await.unwrap();

        assert!(matches!(
            service.issue_password_reset(&actor, sso_only.id).await,
            Err(AccountLinkError::NoPasswordAccount)
        ));
        assert!(matches!(
            service.issue_password_reset(&actor, deactivated.id).await,
            Err(AccountLinkError::AccountDeactivated)
        ));
        assert!(matches!(
            service.issue_password_reset(&actor, UserId::new()).await,
            Err(AccountLinkError::NotFound)
        ));
    }

    #[tokio::test]
    async fn an_email_failure_still_returns_the_link_with_emailed_false() {
        let (service, _users, _tokens, email, _sessions) = service("https://minerva.example.com");
        email.set_fail(true);
        let issued = service
            .create_invite(&admin(), "nofail@example.com", Role::Staff)
            .await
            .unwrap();
        assert!(!issued.emailed);
        assert_eq!(
            issued.link,
            "https://minerva.example.com/accept-invite?token=token-1"
        );
        assert!(email.sent_links().is_empty());
    }

    #[tokio::test]
    async fn an_empty_link_base_url_yields_a_relative_link() {
        let (service, _users, _tokens, _email, _sessions) = service("");
        let issued = service
            .create_invite(&admin(), "rel@example.com", Role::Staff)
            .await
            .unwrap();
        assert_eq!(issued.link, "/accept-invite?token=token-1");
    }

    #[tokio::test]
    async fn inspect_returns_invalid_token_for_unknown_expired_consumed_and_revoked_alike() {
        let (service, _users, tokens, _email, _sessions) = service("");
        // Unknown.
        assert!(matches!(
            service.inspect("no-such-token").await,
            Err(AccountLinkError::InvalidToken)
        ));

        // Expired: seeded directly, since the service never creates one this old.
        let expired = seeded_invite("old@example.com", Utc::now() - Duration::hours(1));
        tokens.insert(expired);
        assert!(matches!(
            service.inspect("seeded").await,
            Err(AccountLinkError::InvalidToken)
        ));

        // Consumed.
        let consumed = service
            .create_invite(&admin(), "c@example.com", Role::Staff)
            .await
            .unwrap();
        tokens.consume(consumed.item.id, Utc::now()).await.unwrap();
        assert!(matches!(
            service.inspect("token-1").await,
            Err(AccountLinkError::InvalidToken)
        ));

        // Revoked.
        let revoked = service
            .create_invite(&admin(), "r@example.com", Role::Staff)
            .await
            .unwrap();
        service.revoke_invite(revoked.item.id).await.unwrap();
        assert!(matches!(
            service.inspect("token-2").await,
            Err(AccountLinkError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn inspect_describes_pending_links() {
        let (service, users, _tokens, _email, _sessions) = service("");
        let actor = admin();
        let issued = service
            .create_invite(&actor, "info@example.com", Role::Staff)
            .await
            .unwrap();
        let info = service.inspect("token-1").await.unwrap();
        assert_eq!(info.purpose, LinkPurpose::Invite);
        assert_eq!(info.email, "info@example.com");
        assert_eq!(info.role, Some(Role::Staff));
        assert_eq!(info.expires_at, issued.item.expires_at);

        let target = user("resetme@example.com", Role::Staff, Some("h"), false);
        users.create(target.clone()).await.unwrap();
        service
            .issue_password_reset(&actor, target.id)
            .await
            .unwrap();
        let info = service.inspect("token-2").await.unwrap();
        assert_eq!(info.purpose, LinkPurpose::PasswordReset);
        assert_eq!(info.email, "resetme@example.com");
        assert_eq!(info.role, None);
    }

    #[tokio::test]
    async fn revoke_and_reissue_error_paths() {
        let (service, users, _tokens, _email, _sessions) = service("");
        let actor = admin();
        let issued = service
            .create_invite(&actor, "err@example.com", Role::Staff)
            .await
            .unwrap();

        // Unknown ids.
        assert!(matches!(
            service.revoke_invite(AccountTokenId::new()).await,
            Err(AccountLinkError::NotFound)
        ));
        assert!(matches!(
            service.reissue_invite(&actor, AccountTokenId::new()).await,
            Err(AccountLinkError::NotFound)
        ));

        // Revoking twice is a no-op...
        service.revoke_invite(issued.item.id).await.unwrap();
        service.revoke_invite(issued.item.id).await.unwrap();

        // ...and an accepted invite can neither be revoked nor re-issued.
        let accepted = service
            .create_invite(&actor, "acc@example.com", Role::Staff)
            .await
            .unwrap();
        service
            .accept_invite("token-2", "long-enough-pass", None)
            .await
            .unwrap();
        assert!(matches!(
            service.revoke_invite(accepted.item.id).await,
            Err(AccountLinkError::InviteAlreadyAccepted)
        ));
        assert!(matches!(
            service.reissue_invite(&actor, accepted.item.id).await,
            Err(AccountLinkError::InviteAlreadyAccepted)
        ));

        // Re-issuing for an email that registered in the meantime.
        let late = service
            .create_invite(&actor, "late2@example.com", Role::Staff)
            .await
            .unwrap();
        users
            .create(user("late2@example.com", Role::ReadOnly, None, false))
            .await
            .unwrap();
        assert!(matches!(
            service.reissue_invite(&actor, late.item.id).await,
            Err(AccountLinkError::EmailAlreadyRegistered)
        ));
    }

    #[tokio::test]
    async fn a_hasher_failure_surfaces_as_a_hash_error() {
        let users = Arc::new(InMemoryUserRepository::new());
        let tokens = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let service = AccountLinkService::new(
            tokens,
            users,
            Arc::new(FailingHasher),
            Arc::new(DeterministicTokens::default()),
            SessionService::new(
                Arc::new(InMemorySessionRepository::new()),
                Arc::new(DeterministicTokens::default()),
                SessionService::DEFAULT_SESSION_TTL,
            ),
            Arc::new(RecordingEmailSender::new()),
            String::new(),
        );
        service
            .create_invite(&admin(), "hash@example.com", Role::Staff)
            .await
            .unwrap();
        assert!(matches!(
            service
                .accept_invite("token-1", "long-enough-pass", None)
                .await,
            Err(AccountLinkError::Hash(_))
        ));
    }

    #[tokio::test]
    async fn a_repository_failure_surfaces_as_a_repository_error() {
        // The token store keeps its own user store for the taken-email check;
        // the service gets the failing one instead.
        let backing_users = Arc::new(InMemoryUserRepository::new());
        let tokens = Arc::new(InMemoryAccountTokenRepository::new(backing_users));
        let service = AccountLinkService::new(
            tokens,
            Arc::new(FailingUserRepository),
            Arc::new(FakePasswordHasher),
            Arc::new(DeterministicTokens::default()),
            SessionService::new(
                Arc::new(InMemorySessionRepository::new()),
                Arc::new(DeterministicTokens::default()),
                SessionService::DEFAULT_SESSION_TTL,
            ),
            Arc::new(RecordingEmailSender::new()),
            String::new(),
        );
        assert!(matches!(
            service
                .create_invite(&admin(), "repo@example.com", Role::Staff)
                .await,
            Err(AccountLinkError::Repository(_))
        ));
    }
}
