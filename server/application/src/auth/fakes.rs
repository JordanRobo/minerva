//! In-memory fakes for unit-testing the auth services without Postgres or
//! Redis. Test-only: compiled under `#[cfg(test)]` in [`super`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use domain::{
    AccountToken, AccountTokenId, AccountTokenKind, Role, Session, SessionId, SsoGroupRule,
    SsoGroupRuleId, User, UserId, UserIdentity,
};

use crate::ports::{
    AcceptInviteOutcome, AccessChange, AccessChangeError, AccountEmailSender,
    AccountTokenRepository, EmailSendError, PasswordHashError, PasswordHasher, RepositoryError,
    ResetPasswordOutcome, SessionRepository, SessionTokens, SsoGroupRuleRepository,
    UserIdentityRepository, UserRepository,
};

/// A [`SessionRepository`] that keeps sessions in a `HashMap`.
#[derive(Default)]
pub struct InMemorySessionRepository {
    sessions: Mutex<HashMap<SessionId, Session>>,
}

impl InMemorySessionRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InMemorySessionRepository {
    fn locked(&self) -> MutexGuard<'_, HashMap<SessionId, Session>> {
        self.sessions.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl SessionRepository for InMemorySessionRepository {
    async fn create(&self, session: Session) -> Result<Session, RepositoryError> {
        self.locked().insert(session.id, session.clone());
        Ok(session)
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<Session>, RepositoryError> {
        Ok(self
            .locked()
            .values()
            .find(|session| session.token_hash == token_hash)
            .cloned())
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<Session>, RepositoryError> {
        Ok(self
            .locked()
            .values()
            .filter(|session| session.user_id == user_id)
            .cloned()
            .collect())
    }

    async fn delete(&self, id: SessionId) -> Result<(), RepositoryError> {
        self.locked().remove(&id);
        Ok(())
    }

    async fn delete_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        self.locked()
            .retain(|_, session| session.user_id != user_id);
        Ok(())
    }

    async fn touch_last_seen(
        &self,
        id: SessionId,
        last_seen_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        // Mirror the real repositories: a session that is gone or already
        // expired at `last_seen_at` (the caller's "now") is not touched.
        match self.locked().get_mut(&id) {
            Some(session) if !session.is_expired(last_seen_at) => {
                session.last_seen_at = last_seen_at;
                session.expires_at = expires_at;
                Ok(())
            }
            _ => Err(RepositoryError::NotFound),
        }
    }

    async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        let mut removed = 0u64;
        self.locked().retain(|_, session| {
            if session.is_expired(now) {
                removed += 1;
                false
            } else {
                true
            }
        });
        Ok(removed)
    }
}

/// An [`InMemorySessionRepository`] whose `touch_last_seen` always fails, so
/// tests can prove a failed touch never fails the request.
pub struct FailingTouchSessionRepository(Arc<InMemorySessionRepository>);

impl FailingTouchSessionRepository {
    pub fn new(inner: Arc<InMemorySessionRepository>) -> Self {
        Self(inner)
    }
}

#[async_trait::async_trait]
impl SessionRepository for FailingTouchSessionRepository {
    async fn create(&self, session: Session) -> Result<Session, RepositoryError> {
        self.0.create(session).await
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<Session>, RepositoryError> {
        self.0.find_by_token_hash(token_hash).await
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<Session>, RepositoryError> {
        self.0.list_for_user(user_id).await
    }

    async fn delete(&self, id: SessionId) -> Result<(), RepositoryError> {
        self.0.delete(id).await
    }

    async fn delete_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        self.0.delete_all_for_user(user_id).await
    }

    async fn touch_last_seen(
        &self,
        _id: SessionId,
        _last_seen_at: DateTime<Utc>,
        _expires_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a touch failure".to_owned(),
        ))
    }

    async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        self.0.purge_expired(now).await
    }
}

/// A [`UserRepository`] that keeps users in a `HashMap`.
#[derive(Default)]
pub struct InMemoryUserRepository {
    users: Mutex<HashMap<UserId, User>>,
}

impl InMemoryUserRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InMemoryUserRepository {
    fn locked(&self) -> MutexGuard<'_, HashMap<UserId, User>> {
        self.users.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn create(&self, user: User) -> Result<User, RepositoryError> {
        self.locked().insert(user.id, user.clone());
        Ok(user)
    }

    async fn create_if_no_users(&self, user: User) -> Result<Option<User>, RepositoryError> {
        // The mutex plays the part of the Postgres advisory lock: the count
        // and the insert happen under it, so a concurrent call cannot slip in
        // between them.
        let mut users = self.locked();
        if !users.is_empty() {
            return Ok(None);
        }
        users.insert(user.id, user.clone());
        Ok(Some(user))
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(self.locked().get(&id).cloned())
    }

    async fn find_by_email(&self, email: String) -> Result<Option<User>, RepositoryError> {
        let email = email.to_lowercase();
        Ok(self
            .locked()
            .values()
            .find(|user| user.email == email)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<User>, RepositoryError> {
        Ok(self.locked().values().cloned().collect())
    }

    async fn update(&self, user: User) -> Result<User, RepositoryError> {
        self.locked().insert(user.id, user.clone());
        Ok(user)
    }

    async fn apply_access_change(
        &self,
        target: UserId,
        change: AccessChange,
    ) -> Result<User, AccessChangeError> {
        // The mutex plays the part of the Postgres advisory lock: every access
        // change re-checks the active-admin count while holding it, so the
        // fake enforces the same last-admin rule as the real repository.
        let mut users = self.locked();
        let Some(user) = users.get(&target) else {
            return Err(AccessChangeError::NotFound);
        };
        let other_active_admins = users
            .values()
            .filter(|other| other.id != target && other.role == Role::Admin && other.is_active())
            .count();
        let now = Utc::now();
        let updated = match change {
            AccessChange::Role(role) => {
                if user.role == role {
                    return Ok(user.clone());
                }
                if user.role == Role::Admin
                    && user.is_active()
                    && role != Role::Admin
                    && other_active_admins == 0
                {
                    return Err(AccessChangeError::LastAdmin);
                }
                User {
                    role,
                    updated_at: now,
                    ..user.clone()
                }
            }
            AccessChange::RoleManagedBySso(role) => {
                // The flag is part of the state: same role but an unflagged
                // row still needs the write.
                if user.role == role && user.role_managed_by_sso {
                    return Ok(user.clone());
                }
                if user.role == Role::Admin
                    && user.is_active()
                    && role != Role::Admin
                    && other_active_admins == 0
                {
                    return Err(AccessChangeError::LastAdmin);
                }
                User {
                    role,
                    role_managed_by_sso: true,
                    updated_at: now,
                    ..user.clone()
                }
            }
            AccessChange::Deactivate => {
                if !user.is_active() {
                    return Ok(user.clone());
                }
                if user.role == Role::Admin && other_active_admins == 0 {
                    return Err(AccessChangeError::LastAdmin);
                }
                // The SSO flags are untouched, like in Postgres: deactivation
                // neither grants nor removes the exemption.
                User {
                    deactivated_at: Some(now),
                    updated_at: now,
                    ..user.clone()
                }
            }
            AccessChange::Reactivate => {
                if user.is_active() {
                    return Ok(user.clone());
                }
                User {
                    deactivated_at: None,
                    updated_at: now,
                    ..user.clone()
                }
            }
        };
        users.insert(target, updated.clone());
        Ok(updated)
    }
}

/// A [`UserIdentityRepository`] that keeps identities in a `HashMap` keyed by
/// (issuer, subject), the pair the port treats as unique: a second create for
/// the same pair is a `Conflict`, like the real repository's constraint.
#[derive(Default)]
pub struct InMemoryUserIdentityRepository {
    identities: Mutex<HashMap<(String, String), UserIdentity>>,
}

impl InMemoryUserIdentityRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InMemoryUserIdentityRepository {
    fn locked(&self) -> MutexGuard<'_, HashMap<(String, String), UserIdentity>> {
        self.identities.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl UserIdentityRepository for InMemoryUserIdentityRepository {
    async fn create(&self, identity: UserIdentity) -> Result<UserIdentity, RepositoryError> {
        let key = (identity.issuer.clone(), identity.subject.clone());
        if self.locked().contains_key(&key) {
            return Err(RepositoryError::Conflict(
                "an identity with this issuer and subject already exists".to_owned(),
            ));
        }
        self.locked().insert(key, identity.clone());
        Ok(identity)
    }

    async fn find_by_issuer_and_subject(
        &self,
        issuer: String,
        subject: String,
    ) -> Result<Option<UserIdentity>, RepositoryError> {
        Ok(self.locked().get(&(issuer, subject)).cloned())
    }

    async fn list_for_user(&self, user_id: UserId) -> Result<Vec<UserIdentity>, RepositoryError> {
        Ok(self
            .locked()
            .values()
            .filter(|identity| identity.user_id == user_id)
            .cloned()
            .collect())
    }
}

/// A deterministic [`SessionTokens`]: the Nth generated token is
/// `"token-N"` and the hash of any token is `"hash-of-{token}"`, so tests can
/// predict both sides of every lookup.
#[derive(Default)]
pub struct DeterministicTokens {
    counter: Mutex<u64>,
}

impl SessionTokens for DeterministicTokens {
    fn generate(&self) -> String {
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        format!("token-{counter}")
    }

    fn hash(&self, token: &str) -> String {
        format!("hash-of-{token}")
    }
}

/// A [`PasswordHasher`] for tests: it accepts a password exactly when the
/// stored hash is `"hash-of-{password}"`, so tests control right and wrong
/// per user without Argon2.
pub struct FakePasswordHasher;

#[async_trait::async_trait]
impl PasswordHasher for FakePasswordHasher {
    async fn hash(&self, password: &str) -> Result<String, PasswordHashError> {
        Ok(format!("hash-of-{password}"))
    }

    async fn verify(&self, password: &str, hash: &str) -> Result<bool, PasswordHashError> {
        Ok(hash == format!("hash-of-{password}"))
    }
}

/// A [`UserRepository`] whose operations always fail, for testing the error
/// paths of services that wrap it.
pub struct FailingUserRepository;

#[async_trait::async_trait]
impl UserRepository for FailingUserRepository {
    async fn create(&self, _user: User) -> Result<User, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn create_if_no_users(&self, _user: User) -> Result<Option<User>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn find_by_id(&self, _id: UserId) -> Result<Option<User>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn find_by_email(&self, _email: String) -> Result<Option<User>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn list(&self) -> Result<Vec<User>, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn update(&self, _user: User) -> Result<User, RepositoryError> {
        Err(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        ))
    }

    async fn apply_access_change(
        &self,
        _target: UserId,
        _change: AccessChange,
    ) -> Result<User, AccessChangeError> {
        Err(AccessChangeError::Repository(RepositoryError::Unexpected(
            "faking a repository failure".to_owned(),
        )))
    }
}

/// An [`AccountTokenRepository`] that keeps tokens in a `Vec`; the mutex
/// plays the part of the Postgres transactions, so the fake produces the same
/// outcomes as the real one (issue revokes live siblings, accept_invite rolls
/// its claim back on a taken email, ...). It also holds the user store it
/// checks for the taken-email rule, standing in for the real repository's
/// users table.
pub struct InMemoryAccountTokenRepository {
    tokens: Mutex<Vec<AccountToken>>,
    users: Arc<InMemoryUserRepository>,
}

impl InMemoryAccountTokenRepository {
    pub fn new(users: Arc<InMemoryUserRepository>) -> Self {
        Self {
            tokens: Mutex::new(Vec::new()),
            users,
        }
    }

    /// Seed a token directly (e.g. an expired one the service would never
    /// create).
    pub fn insert(&self, token: AccountToken) {
        self.tokens.lock().unwrap().push(token);
    }

    fn locked(&self) -> MutexGuard<'_, Vec<AccountToken>> {
        self.tokens.lock().unwrap()
    }
}

/// Whether a token is live in the repository's sense: unconsumed and
/// unrevoked, expired or not.
fn token_is_live(token: &AccountToken) -> bool {
    token.consumed_at.is_none() && token.revoked_at.is_none()
}

/// Whether two tokens are for the same subject: the same email for invites,
/// the same user for resets.
fn tokens_share_subject(a: &AccountTokenKind, b: &AccountTokenKind) -> bool {
    match (a, b) {
        (AccountTokenKind::Invite { email: a, .. }, AccountTokenKind::Invite { email: b, .. }) => {
            a == b
        }
        (
            AccountTokenKind::PasswordReset { user_id: a },
            AccountTokenKind::PasswordReset { user_id: b },
        ) => a == b,
        _ => false,
    }
}

#[async_trait::async_trait]
impl AccountTokenRepository for InMemoryAccountTokenRepository {
    async fn issue(
        &self,
        token: AccountToken,
        now: DateTime<Utc>,
    ) -> Result<AccountToken, RepositoryError> {
        let mut tokens = self.locked();
        // Revoke the subject's live token first, like the Postgres
        // transaction.
        for old in tokens.iter_mut() {
            if token_is_live(old) && tokens_share_subject(&old.kind, &token.kind) {
                old.revoked_at = Some(now);
            }
        }
        tokens.push(token.clone());
        Ok(token)
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        Ok(self
            .locked()
            .iter()
            .find(|token| token.token_hash == token_hash)
            .cloned())
    }

    async fn find_by_id(
        &self,
        id: AccountTokenId,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        Ok(self.locked().iter().find(|token| token.id == id).cloned())
    }

    async fn list_invites(&self) -> Result<Vec<AccountToken>, RepositoryError> {
        let mut invites = self
            .locked()
            .iter()
            .filter(|token| matches!(token.kind, AccountTokenKind::Invite { .. }))
            .cloned()
            .collect::<Vec<_>>();
        // Newest first; the id breaks created_at ties, like the real query.
        invites.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        Ok(invites)
    }

    async fn find_pending_invite_for_email(
        &self,
        email: String,
        now: DateTime<Utc>,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        // Stored emails are normalized (trimmed, lowercased); match that.
        let email = email.trim().to_lowercase();
        Ok(self
            .locked()
            .iter()
            .find(|token| {
                matches!(&token.kind, AccountTokenKind::Invite { email: e, .. } if *e == email)
                    && token.consumed_at.is_none()
                    && token.revoked_at.is_none()
                    && token.expires_at > now
            })
            .cloned())
    }

    async fn revoke(&self, id: AccountTokenId, now: DateTime<Utc>) -> Result<(), RepositoryError> {
        let mut tokens = self.locked();
        let Some(token) = tokens.iter_mut().find(|token| token.id == id) else {
            return Err(RepositoryError::NotFound);
        };
        // Only a live token flips; an already-consumed or revoked one is an
        // idempotent no-op.
        if token_is_live(token) {
            token.revoked_at = Some(now);
        }
        Ok(())
    }

    async fn consume(
        &self,
        id: AccountTokenId,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let mut tokens = self.locked();
        let Some(token) = tokens.iter_mut().find(|token| token.id == id) else {
            return Ok(false);
        };
        if token_is_live(token) && token.expires_at > now {
            token.consumed_at = Some(now);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn accept_invite(
        &self,
        id: AccountTokenId,
        user: User,
        now: DateTime<Utc>,
    ) -> Result<AcceptInviteOutcome, RepositoryError> {
        // The taken-email check runs before the claim so no lock is held
        // across an await; single-threaded tests see the same outcome as the
        // Postgres transaction either way.
        let email_taken = self
            .users
            .find_by_email(user.email.clone())
            .await?
            .is_some();
        // Claim under the tokens lock, like the Postgres transaction...
        let claimed = {
            let mut tokens = self.locked();
            match tokens.iter_mut().find(|token| token.id == id) {
                Some(token) if token.is_pending(now) => {
                    token.consumed_at = Some(now);
                    true
                }
                _ => false,
            }
        };
        if !claimed {
            return Ok(AcceptInviteOutcome::TokenUnusable);
        }
        if email_taken {
            // ...and roll the claim back when the email is taken, so a failed
            // attempt does not burn the invite.
            let mut tokens = self.locked();
            if let Some(token) = tokens.iter_mut().find(|token| token.id == id) {
                token.consumed_at = None;
            }
            return Ok(AcceptInviteOutcome::EmailTaken);
        }
        self.users.create(user.clone()).await?;
        Ok(AcceptInviteOutcome::Accepted(user))
    }

    async fn reset_password(
        &self,
        id: AccountTokenId,
        user_id: UserId,
        password_hash: String,
        now: DateTime<Utc>,
    ) -> Result<ResetPasswordOutcome, RepositoryError> {
        // Claim under the tokens lock...
        let claimed = {
            let mut tokens = self.locked();
            match tokens.iter_mut().find(|token| token.id == id) {
                Some(token) if token.is_pending(now) => {
                    token.consumed_at = Some(now);
                    Some(token.kind.clone())
                }
                _ => None,
            }
        };
        let Some(kind) = claimed else {
            return Ok(ResetPasswordOutcome::TokenUnusable);
        };
        // ...then apply the user change outside it (no lock is held across an
        // await).
        match self.users.find_by_id(user_id).await? {
            None => {
                // Roll the claim back when the user is gone.
                let mut tokens = self.locked();
                if let Some(token) = tokens.iter_mut().find(|token| token.id == id) {
                    token.consumed_at = None;
                }
                Ok(ResetPasswordOutcome::UserNotFound)
            }
            Some(mut user) => {
                user.password_hash = Some(password_hash);
                user.updated_at = now;
                self.users.update(user).await?;
                // Any other live reset link for this user stops working now.
                let mut tokens = self.locked();
                for old in tokens.iter_mut() {
                    if token_is_live(old) && tokens_share_subject(&old.kind, &kind) {
                        old.revoked_at = Some(now);
                    }
                }
                Ok(ResetPasswordOutcome::Done)
            }
        }
    }
}

/// An [`AccountEmailSender`] for tests: delivery succeeds or fails on demand,
/// and every successfully sent link is recorded so tests can assert what was
/// emailed.
pub struct RecordingEmailSender {
    fail: Mutex<bool>,
    sent: Mutex<Vec<String>>,
}

impl Default for RecordingEmailSender {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingEmailSender {
    pub fn new() -> Self {
        Self {
            fail: Mutex::new(false),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Make every send fail (or succeed again).
    pub fn set_fail(&self, fail: bool) {
        *self.fail.lock().unwrap() = fail;
    }

    /// The links that were actually sent.
    pub fn sent_links(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }

    fn record(&self, link: &str) -> Result<(), EmailSendError> {
        if *self.fail.lock().unwrap() {
            return Err(EmailSendError("faking a delivery failure".to_owned()));
        }
        self.sent.lock().unwrap().push(link.to_owned());
        Ok(())
    }
}

#[async_trait::async_trait]
impl AccountEmailSender for RecordingEmailSender {
    fn is_configured(&self) -> bool {
        !*self.fail.lock().unwrap()
    }

    async fn send_invite(
        &self,
        _to_email: &str,
        link: &str,
        _role: Role,
        _expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError> {
        self.record(link)
    }

    async fn send_password_reset(
        &self,
        _to_email: &str,
        link: &str,
        _expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError> {
        self.record(link)
    }
}

/// A [`SsoGroupRuleRepository`] that keeps rules in a `HashMap`. The unique
/// group name is enforced like the real repository's index: a second rule for
/// the same name is a `Conflict`.
#[derive(Default)]
pub struct InMemorySsoGroupRuleRepository {
    rules: Mutex<HashMap<SsoGroupRuleId, SsoGroupRule>>,
}

impl InMemorySsoGroupRuleRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InMemorySsoGroupRuleRepository {
    fn locked(&self) -> MutexGuard<'_, HashMap<SsoGroupRuleId, SsoGroupRule>> {
        self.rules.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl SsoGroupRuleRepository for InMemorySsoGroupRuleRepository {
    async fn list(&self) -> Result<Vec<SsoGroupRule>, RepositoryError> {
        let mut rules: Vec<SsoGroupRule> = self.locked().values().cloned().collect();
        // Group name then id, like the real query.
        rules.sort_by(|a, b| {
            a.group_name
                .cmp(&b.group_name)
                .then_with(|| a.id.0.cmp(&b.id.0))
        });
        Ok(rules)
    }

    async fn find_by_id(
        &self,
        id: SsoGroupRuleId,
    ) -> Result<Option<SsoGroupRule>, RepositoryError> {
        Ok(self.locked().get(&id).cloned())
    }

    async fn create(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
        let mut rules = self.locked();
        if rules
            .values()
            .any(|existing| existing.group_name == rule.group_name)
        {
            return Err(RepositoryError::Conflict(format!(
                "a rule for group {} already exists",
                rule.group_name
            )));
        }
        rules.insert(rule.id, rule.clone());
        Ok(rule)
    }

    async fn update(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
        let mut rules = self.locked();
        if !rules.contains_key(&rule.id) {
            return Err(RepositoryError::NotFound);
        }
        if rules
            .values()
            .any(|existing| existing.id != rule.id && existing.group_name == rule.group_name)
        {
            return Err(RepositoryError::Conflict(format!(
                "a rule for group {} already exists",
                rule.group_name
            )));
        }
        rules.insert(rule.id, rule.clone());
        Ok(rule)
    }

    async fn delete(&self, id: SsoGroupRuleId) -> Result<(), RepositoryError> {
        if self.locked().remove(&id).is_none() {
            return Err(RepositoryError::NotFound);
        }
        Ok(())
    }

    async fn any_exist(&self) -> Result<bool, RepositoryError> {
        Ok(!self.locked().is_empty())
    }
}

#[cfg(test)]
mod sso_group_rule_fake_tests {
    use super::*;

    fn test_rule(group_name: &str, role: Role) -> SsoGroupRule {
        let now = Utc::now();
        SsoGroupRule {
            id: SsoGroupRuleId::new(),
            group_name: group_name.to_owned(),
            role,
            created_at: now,
            updated_at: now,
        }
    }

    /// The fake enforces the same one-rule-per-group-name rule as the
    /// Postgres unique index, and lists in the repository's stable order.
    #[tokio::test]
    async fn duplicate_group_name_is_a_conflict_and_list_is_stably_ordered() {
        let repo = InMemorySsoGroupRuleRepository::new();

        assert!(!repo.any_exist().await.unwrap());
        let zeta = test_rule("zeta", Role::Admin);
        let alpha = test_rule("alpha", Role::Staff);
        repo.create(zeta.clone()).await.unwrap();
        repo.create(alpha.clone()).await.unwrap();
        assert!(repo.any_exist().await.unwrap());

        let duplicate = test_rule("zeta", Role::ReadOnly);
        let error = repo.create(duplicate).await.expect_err("duplicate name");
        assert!(matches!(error, RepositoryError::Conflict(_)));

        let listed = repo.list().await.unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|rule| rule.group_name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );

        // Renaming onto a taken name conflicts too; deleting frees the name.
        let renamed = SsoGroupRule {
            group_name: "zeta".to_owned(),
            ..alpha.clone()
        };
        let error = repo.update(renamed).await.expect_err("taken name");
        assert!(matches!(error, RepositoryError::Conflict(_)));

        repo.delete(alpha.id).await.unwrap();
        assert!(repo.find_by_id(alpha.id).await.unwrap().is_none());
        let error = repo.delete(alpha.id).await.expect_err("already deleted");
        assert!(matches!(error, RepositoryError::NotFound));
    }
}
