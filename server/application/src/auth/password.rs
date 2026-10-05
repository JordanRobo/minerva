//! The email-and-password sign-in method behind [`CredentialProvider`]: the
//! original `/api/auth/login` logic, unchanged.

use std::sync::Arc;

use domain::User;

use super::normalize_email;
use super::provider::{AuthError, AuthProvider, CredentialProvider, Credentials};
use crate::ports::{PasswordHasher, UserRepository};

/// The registry id of the email-and-password provider.
pub const PASSWORD_PROVIDER_ID: &str = "password";

/// Signs users in with an email address and a password. Every rejection —
/// unknown email, deactivated account, passwordless account, wrong password —
/// is [`AuthError::InvalidCredentials`], so the answer never reveals which
/// check failed; only repository or hasher failures surface as
/// [`AuthError::Internal`]. Every attempt runs exactly one Argon2
/// verification (real when the account has a stored hash, dummy otherwise),
/// so response time reveals nothing either.
pub struct PasswordAuthProvider {
    users: Arc<dyn UserRepository>,
    hasher: Arc<dyn PasswordHasher>,
}

impl PasswordAuthProvider {
    pub fn new(users: Arc<dyn UserRepository>, hasher: Arc<dyn PasswordHasher>) -> Self {
        Self { users, hasher }
    }
}

impl AuthProvider for PasswordAuthProvider {
    fn id(&self) -> &str {
        PASSWORD_PROVIDER_ID
    }

    fn display_name(&self) -> &str {
        "Email and password"
    }
}

#[async_trait::async_trait]
impl CredentialProvider for PasswordAuthProvider {
    async fn authenticate(&self, credentials: Credentials) -> Result<User, AuthError> {
        let user = self
            .users
            .find_by_email(normalize_email(&credentials.identifier))
            .await?;
        // Exactly one verification per login — real when the account has a
        // stored hash, dummy otherwise — so response time does not reveal
        // whether the email exists or has a password. The deactivated and
        // passwordless conditions are only evaluated after it.
        let valid = match &user {
            Some(user) => match user.password_hash.as_deref() {
                Some(hash) => self.hasher.verify(&credentials.secret, hash).await,
                None => self.hasher.verify_dummy(&credentials.secret).await,
            },
            None => self.hasher.verify_dummy(&credentials.secret).await,
        }
        .map_err(|error| AuthError::Internal(error.to_string()))?;
        // "No such user", "account has no password" and "wrong password" all
        // fall through to the same error: the answer must not reveal that a
        // passwordless account (one that can only sign in via an external
        // identity provider) exists.
        let Some(user) = user else {
            return Err(AuthError::InvalidCredentials);
        };
        // A deactivated account is refused like any other bad credential.
        if !user.is_active() {
            return Err(AuthError::InvalidCredentials);
        }
        if user.password_hash.is_none() {
            return Err(AuthError::InvalidCredentials);
        }
        if !valid {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::auth::fakes::{FailingUserRepository, FakePasswordHasher, InMemoryUserRepository};
    use crate::ports::PasswordHashError;
    use chrono::Utc;
    use domain::{Role, UserId};

    fn user(email: &str, password_hash: Option<&str>) -> User {
        User {
            id: UserId::new(),
            email: email.to_owned(),
            password_hash: password_hash.map(str::to_owned),
            display_name: "Test user".to_owned(),
            role: Role::Admin,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn credentials(identifier: &str, secret: &str) -> Credentials {
        Credentials {
            identifier: identifier.to_owned(),
            secret: secret.to_owned(),
        }
    }

    /// A provider over an in-memory user store; `FakePasswordHasher` accepts
    /// a password exactly when the stored hash is `"hash-of-{password}"`.
    fn provider(users: Arc<InMemoryUserRepository>) -> PasswordAuthProvider {
        PasswordAuthProvider::new(users, Arc::new(FakePasswordHasher))
    }

    /// A store holding one user whose password is `"s3cret"`.
    async fn seeded() -> (PasswordAuthProvider, User) {
        let users = InMemoryUserRepository::new();
        let user = user("user@example.com", Some("hash-of-s3cret"));
        users.create(user.clone()).await.unwrap();
        (provider(Arc::new(users)), user)
    }

    /// A [`PasswordHasher`] that counts every call and accepts a password
    /// exactly when the stored hash is `"hash-of-{password}"`, like
    /// `FakePasswordHasher`.
    struct CountingHasher {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl PasswordHasher for CountingHasher {
        async fn hash(&self, password: &str) -> Result<String, PasswordHashError> {
            Ok(format!("hash-of-{password}"))
        }

        async fn verify(&self, password: &str, hash: &str) -> Result<bool, PasswordHashError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(hash == format!("hash-of-{password}"))
        }

        async fn verify_dummy(&self, _password: &str) -> Result<bool, PasswordHashError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        }
    }

    /// A provider over an in-memory user store whose hasher counts calls.
    fn counting_provider(
        users: Arc<InMemoryUserRepository>,
    ) -> (PasswordAuthProvider, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = PasswordAuthProvider::new(
            users,
            Arc::new(CountingHasher {
                calls: calls.clone(),
            }),
        );
        (provider, calls)
    }

    #[tokio::test]
    async fn correct_password_returns_the_user() {
        let (provider, user) = seeded().await;
        let found = provider
            .authenticate(credentials("user@example.com", "s3cret"))
            .await
            .unwrap();
        assert_eq!(found.id, user.id);
    }

    #[tokio::test]
    async fn wrong_password_is_invalid_credentials() {
        let (provider, _user) = seeded().await;
        let err = provider
            .authenticate(credentials("user@example.com", "nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[tokio::test]
    async fn unknown_email_is_invalid_credentials() {
        let (provider, _user) = seeded().await;
        let err = provider
            .authenticate(credentials("ghost@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[tokio::test]
    async fn passwordless_account_is_invalid_credentials() {
        let users = InMemoryUserRepository::new();
        users
            .create(user("sso-only@example.com", None))
            .await
            .unwrap();
        let provider = provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("sso-only@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[tokio::test]
    async fn a_deactivated_account_is_invalid_credentials() {
        let users = InMemoryUserRepository::new();
        let mut deactivated = user("off@example.com", Some("hash-of-s3cret"));
        deactivated.deactivated_at = Some(Utc::now());
        users.create(deactivated).await.unwrap();
        let provider = provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("off@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    // Timing equalisation: every rejection must cost exactly one hasher call
    // (real verify or dummy) and return the same error value — the unit
    // variant `InvalidCredentials` a wrong password produces.

    #[tokio::test]
    async fn wrong_password_costs_exactly_one_verify() {
        let users = InMemoryUserRepository::new();
        users
            .create(user("user@example.com", Some("hash-of-s3cret")))
            .await
            .unwrap();
        let (provider, calls) = counting_provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("user@example.com", "nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_email_costs_exactly_one_dummy_verify() {
        let users = InMemoryUserRepository::new();
        users
            .create(user("user@example.com", Some("hash-of-s3cret")))
            .await
            .unwrap();
        let (provider, calls) = counting_provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("ghost@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn passwordless_account_costs_exactly_one_dummy_verify() {
        let users = InMemoryUserRepository::new();
        users
            .create(user("sso-only@example.com", None))
            .await
            .unwrap();
        let (provider, calls) = counting_provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("sso-only@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deactivated_account_costs_exactly_one_verify() {
        let users = InMemoryUserRepository::new();
        let mut deactivated = user("off@example.com", Some("hash-of-s3cret"));
        deactivated.deactivated_at = Some(Utc::now());
        users.create(deactivated).await.unwrap();
        let (provider, calls) = counting_provider(Arc::new(users));
        let err = provider
            .authenticate(credentials("off@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn email_is_trimmed_and_case_insensitive() {
        let (provider, user) = seeded().await;
        let found = provider
            .authenticate(credentials("  USER@Example.COM ", "s3cret"))
            .await
            .unwrap();
        assert_eq!(found.id, user.id);
    }

    #[tokio::test]
    async fn repository_failure_is_internal() {
        let provider = PasswordAuthProvider::new(
            Arc::new(FailingUserRepository),
            Arc::new(FakePasswordHasher),
        );
        let err = provider
            .authenticate(credentials("user@example.com", "s3cret"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Internal(_)));
    }
}
