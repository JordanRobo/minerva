//! In-memory fakes for unit-testing the auth services without Postgres or
//! Redis. Test-only: compiled under `#[cfg(test)]` in [`super`].

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use domain::{Role, Session, SessionId, User, UserId, UserIdentity};

use crate::ports::{
    AccessChange, AccessChangeError, PasswordHashError, PasswordHasher, RepositoryError,
    SessionRepository, SessionTokens, UserIdentityRepository, UserRepository,
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
    ) -> Result<(), RepositoryError> {
        if let Some(session) = self.locked().get_mut(&id) {
            session.last_seen_at = last_seen_at;
        }
        Ok(())
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
            AccessChange::Deactivate => {
                if !user.is_active() {
                    return Ok(user.clone());
                }
                if user.role == Role::Admin && other_active_admins == 0 {
                    return Err(AccessChangeError::LastAdmin);
                }
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
