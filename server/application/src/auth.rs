//! Provider-independent session handling: issuing, resolving and revoking
//! cookie sessions over the [`SessionRepository`] and [`SessionTokens`] ports.
//!
//! The raw token is generated once at issue time and only ever returned then;
//! from that moment on only its hash exists server-side, so a leaked database
//! cannot be turned into live sessions.

use chrono::{Duration, Utc};
use domain::{Session, SessionId, UserId};
use std::sync::Arc;

use crate::ports::{RepositoryError, SessionRepository, SessionTokens};

pub mod oidc;
pub mod password;
pub mod provider;

#[cfg(test)]
pub(crate) mod fakes;

/// A freshly issued session: the raw token (shown to the client exactly once)
/// and the session record whose `token_hash` is what got stored.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    /// The raw token; only its hash was persisted.
    pub token: String,
    /// The session as stored (id, user, expiry).
    pub session: Session,
}

/// Issues, resolves and revokes sessions. Every sign-in method ends in
/// [`SessionService::issue`], so sessions are indistinguishable regardless of
/// how the user signed in.
#[derive(Clone)]
pub struct SessionService {
    sessions: Arc<dyn SessionRepository>,
    tokens: Arc<dyn SessionTokens>,
    ttl: Duration,
}

impl SessionService {
    /// How long a session stays valid after it is created, unless the service
    /// is built with a different TTL.
    pub const DEFAULT_SESSION_TTL: Duration = Duration::days(30);

    pub fn new(
        sessions: Arc<dyn SessionRepository>,
        tokens: Arc<dyn SessionTokens>,
        ttl: Duration,
    ) -> Self {
        Self {
            sessions,
            tokens,
            ttl,
        }
    }

    /// Create a session for `user_id`. The raw token is only ever returned
    /// here; only its hash is stored.
    pub async fn issue(&self, user_id: UserId) -> Result<IssuedSession, RepositoryError> {
        let now = Utc::now();
        let token = self.tokens.generate();
        let session = Session {
            id: SessionId::new(),
            user_id,
            token_hash: self.tokens.hash(&token),
            created_at: now,
            expires_at: now + self.ttl,
            last_seen_at: now,
        };
        let session = self.sessions.create(session).await?;
        Ok(IssuedSession { token, session })
    }

    /// The live session behind a raw token, or `None` when the token is
    /// unknown or its session has expired. Callers must treat both the same
    /// (a generic 401): the answer must not hint which.
    pub async fn resolve(&self, raw_token: &str) -> Result<Option<Session>, RepositoryError> {
        let session = self
            .sessions
            .find_by_token_hash(self.tokens.hash(raw_token))
            .await?;
        Ok(session.filter(|session| !session.is_expired(Utc::now())))
    }

    /// Delete the session behind a raw token, expired or not. Idempotent: an
    /// unknown token is not an error, so logging out twice (or with no
    /// session) still succeeds.
    pub async fn revoke(&self, raw_token: &str) -> Result<(), RepositoryError> {
        let session = self
            .sessions
            .find_by_token_hash(self.tokens.hash(raw_token))
            .await?;
        if let Some(session) = session {
            self.sessions.delete(session.id).await?;
        }
        Ok(())
    }

    /// Delete every session of a user at once (e.g. when an admin deactivates
    /// the account). A user with no sessions is a valid state, so this never
    /// errors on an empty result.
    pub async fn revoke_all_for_user(&self, user_id: UserId) -> Result<(), RepositoryError> {
        self.sessions.delete_all_for_user(user_id).await
    }
}

/// Canonical form of an email address for storage and lookup: trimmed and
/// lowercased, so " Foo@X.com " and "foo@x.com" are the same account.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// A minimal email shape check for user-supplied addresses: exactly one '@'
/// with a non-empty local part and domain. Returns the first problem found,
/// if any — the same rules the configuration layer applies to its own input.
pub fn validate_email(raw: &str) -> Option<String> {
    let email = raw.trim();
    if email.matches('@').count() != 1 {
        return Some("exactly one '@' is required".to_owned());
    }
    let (local, domain) = email.split_once('@').expect("checked above");
    if local.is_empty() || domain.is_empty() {
        return Some("the parts before and after the '@' must not be empty".to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::{
        DeterministicTokens, InMemorySessionRepository, InMemoryUserRepository,
    };
    use crate::ports::UserRepository;
    use domain::{Role, User};

    /// A service over in-memory fakes with the given TTL.
    fn service(ttl: Duration) -> (SessionService, Arc<InMemorySessionRepository>) {
        let sessions = Arc::new(InMemorySessionRepository::new());
        let service = SessionService::new(
            sessions.clone(),
            Arc::new(DeterministicTokens::default()),
            ttl,
        );
        (service, sessions)
    }

    #[tokio::test]
    async fn issue_stores_only_the_hash() {
        let (service, sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        let user_id = UserId::new();
        let issued = service.issue(user_id).await.unwrap();

        // The deterministic generator's first token is "token-1".
        assert_eq!(issued.token, "token-1");
        // The stored row carries the hash — never the raw token.
        let stored = sessions
            .find_by_token_hash("hash-of-token-1".to_owned())
            .await
            .unwrap()
            .expect("session stored under its hash");
        assert_eq!(stored.token_hash, "hash-of-token-1");
        assert_ne!(stored.token_hash, issued.token);
        assert_eq!(stored.user_id, user_id);
        let by_raw = sessions
            .find_by_token_hash(issued.token.clone())
            .await
            .unwrap();
        assert!(by_raw.is_none(), "raw token must not be stored");
    }

    #[tokio::test]
    async fn issue_applies_the_ttl() {
        let (service, _sessions) = service(Duration::hours(12));
        let issued = service.issue(UserId::new()).await.unwrap();
        assert_eq!(
            issued.session.expires_at - issued.session.created_at,
            Duration::hours(12)
        );
    }

    #[tokio::test]
    async fn resolve_returns_a_valid_session() {
        let (service, _sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        let user_id = UserId::new();
        let issued = service.issue(user_id).await.unwrap();
        let resolved = service
            .resolve(&issued.token)
            .await
            .unwrap()
            .expect("live session");
        assert_eq!(resolved.id, issued.session.id);
        assert_eq!(resolved.user_id, user_id);
    }

    #[tokio::test]
    async fn resolve_unknown_token_is_none() {
        let (service, _sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        assert!(service.resolve("no-such-token").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn resolve_expired_session_is_none() {
        let (service, sessions) = service(Duration::seconds(1));
        // A session already past its expiry: the repository still returns it,
        // so `None` must come from the service's is_expired check.
        let now = Utc::now();
        sessions
            .create(Session {
                id: SessionId::new(),
                user_id: UserId::new(),
                token_hash: "hash-of-expired".to_owned(),
                created_at: now - Duration::hours(2),
                expires_at: now - Duration::hours(1),
                last_seen_at: now - Duration::hours(2),
            })
            .await
            .unwrap();
        assert!(service.resolve("expired").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn resolved_session_finds_its_user() {
        // The AuthenticatedUser extractor's flow: resolve the session, then
        // load the user it belongs to.
        let (service, _sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        let users = InMemoryUserRepository::new();
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_owned(),
            password_hash: None,
            display_name: "Test user".to_owned(),
            role: Role::ReadOnly,
            deactivated_at: None,
            created_at: now,
            updated_at: now,
        };
        users.create(user.clone()).await.unwrap();

        let issued = service.issue(user.id).await.unwrap();
        let session = service
            .resolve(&issued.token)
            .await
            .unwrap()
            .expect("live session");
        let found = users
            .find_by_id(session.user_id)
            .await
            .unwrap()
            .expect("user exists");
        assert_eq!(found.id, user.id);
    }

    #[tokio::test]
    async fn revoke_removes_the_session() {
        let (service, _sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        let issued = service.issue(UserId::new()).await.unwrap();
        service.revoke(&issued.token).await.unwrap();
        assert!(service.resolve(&issued.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_is_idempotent() {
        let (service, _sessions) = service(SessionService::DEFAULT_SESSION_TTL);
        let issued = service.issue(UserId::new()).await.unwrap();
        service.revoke(&issued.token).await.unwrap();
        // A second revoke, and a revoke of a token that never existed: both Ok.
        service.revoke(&issued.token).await.unwrap();
        service.revoke("no-such-token").await.unwrap();
    }

    #[test]
    fn normalize_email_trims_and_lowercases() {
        assert_eq!(normalize_email("  Foo@Example.COM "), "foo@example.com");
        assert_eq!(normalize_email("a@b.c"), "a@b.c");
        assert_eq!(normalize_email("   "), "");
    }
}
