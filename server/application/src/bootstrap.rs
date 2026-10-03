//! First-admin bootstrap (roadmap 2.5): the use case that creates the very
//! first admin account while the system has no users at all. Once any user
//! exists, this path can never create anyone — new accounts come from SSO
//! and, later, invites (roadmap 2.6).

use chrono::{DateTime, Utc};
use domain::{Role, User, UserId};

use crate::auth::normalize_email;
use crate::ports::{PasswordHashError, PasswordHasher, RepositoryError, UserRepository};

/// The minimum length of the first admin's password.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// The input for [`bootstrap_admin`].
pub struct BootstrapAdmin {
    pub email: String,
    pub display_name: String,
    /// Plaintext only until [`PasswordHasher`] has hashed it: never stored
    /// and never logged (see the manual `Debug`).
    pub password: String,
}

impl std::fmt::Debug for BootstrapAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The password must not leak through logs or panic messages.
        f.debug_struct("BootstrapAdmin")
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// What [`bootstrap_admin`] did.
#[derive(Debug)]
pub enum BootstrapOutcome {
    /// The store was empty and the first admin was created.
    Created(User),
    /// Some user already exists, so nothing was created.
    UsersAlreadyExist,
}

/// A failure of [`bootstrap_admin`].
#[derive(Debug)]
pub enum BootstrapError {
    /// Hashing the password failed.
    Hash(PasswordHashError),
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Hash(err) => write!(f, "could not hash the password: {err}"),
            BootstrapError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

/// Create the very first admin account.
///
/// Whether this may create anyone is decided by
/// [`UserRepository::create_if_no_users`], atomically: two concurrent
/// bootstraps cannot both win. The email is normalized (trimmed, lowercased),
/// a blank display name falls back to "Administrator", and the password is
/// hashed — the plaintext lives only in this call's stack frame and is never
/// stored or logged.
pub async fn bootstrap_admin(
    users: &dyn UserRepository,
    hasher: &dyn PasswordHasher,
    admin: BootstrapAdmin,
    now: DateTime<Utc>,
) -> Result<BootstrapOutcome, BootstrapError> {
    let password_hash = hasher
        .hash(&admin.password)
        .await
        .map_err(BootstrapError::Hash)?;
    let display_name = admin.display_name.trim();
    let display_name = if display_name.is_empty() {
        "Administrator"
    } else {
        display_name
    };
    let user = User {
        id: UserId::new(),
        email: normalize_email(&admin.email),
        password_hash: Some(password_hash),
        display_name: display_name.to_owned(),
        role: Role::Admin,
        deactivated_at: None,
        created_at: now,
        updated_at: now,
    };
    match users
        .create_if_no_users(user)
        .await
        .map_err(BootstrapError::Repository)?
    {
        Some(user) => Ok(BootstrapOutcome::Created(user)),
        None => Ok(BootstrapOutcome::UsersAlreadyExist),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::{FailingUserRepository, FakePasswordHasher, InMemoryUserRepository};

    fn admin(email: &str, display_name: &str, password: &str) -> BootstrapAdmin {
        BootstrapAdmin {
            email: email.to_owned(),
            display_name: display_name.to_owned(),
            password: password.to_owned(),
        }
    }

    /// A user already in the store; `password_hash` and `deactivated_at`
    /// vary per test.
    fn existing(email: &str, password_hash: Option<&str>, deactivated: bool) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: email.to_owned(),
            password_hash: password_hash.map(str::to_owned),
            display_name: "Existing".to_owned(),
            role: Role::ReadOnly,
            deactivated_at: deactivated.then_some(now),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn creates_an_admin_with_a_hashed_password_when_the_store_is_empty() {
        let users = InMemoryUserRepository::new();
        let now = Utc::now();
        let outcome = bootstrap_admin(
            &users,
            &FakePasswordHasher,
            admin("root@example.com", "Root", "correct-horse-battery"),
            now,
        )
        .await
        .unwrap();
        let BootstrapOutcome::Created(user) = outcome else {
            panic!("expected the first admin to be created");
        };
        assert_eq!(user.role, Role::Admin);
        assert!(user.deactivated_at.is_none());
        assert_eq!(user.created_at, now);
        assert_eq!(user.updated_at, now);
        // The stored hash is the hasher's output — never the plaintext.
        assert_eq!(
            user.password_hash.as_deref(),
            Some("hash-of-correct-horse-battery")
        );
        let stored = users.list().await.unwrap();
        let [stored] = stored.as_slice() else {
            panic!("expected exactly one user");
        };
        assert_ne!(
            stored.password_hash.as_deref(),
            Some("correct-horse-battery")
        );
    }

    #[tokio::test]
    async fn email_is_trimmed_and_lowercased() {
        let users = InMemoryUserRepository::new();
        let outcome = bootstrap_admin(
            &users,
            &FakePasswordHasher,
            admin("  Root@Example.COM ", "Root", "correct-horse"),
            Utc::now(),
        )
        .await
        .unwrap();
        let BootstrapOutcome::Created(user) = outcome else {
            panic!("expected the first admin to be created");
        };
        assert_eq!(user.email, "root@example.com");
    }

    #[tokio::test]
    async fn blank_display_name_falls_back_to_administrator() {
        let users = InMemoryUserRepository::new();
        let outcome = bootstrap_admin(
            &users,
            &FakePasswordHasher,
            admin("root@example.com", "   ", "correct-horse"),
            Utc::now(),
        )
        .await
        .unwrap();
        let BootstrapOutcome::Created(user) = outcome else {
            panic!("expected the first admin to be created");
        };
        assert_eq!(user.display_name, "Administrator");
    }

    #[tokio::test]
    async fn any_existing_user_blocks_the_bootstrap() {
        // An active user, a deactivated one and a passwordless one all count
        // as "users exist".
        for (password_hash, deactivated) in [
            (Some("hash-of-x"), false),
            (Some("hash-of-x"), true),
            (None, false),
        ] {
            let users = InMemoryUserRepository::new();
            users
                .create(existing("taken@example.com", password_hash, deactivated))
                .await
                .unwrap();
            let outcome = bootstrap_admin(
                &users,
                &FakePasswordHasher,
                admin("root@example.com", "Root", "correct-horse"),
                Utc::now(),
            )
            .await
            .unwrap();
            assert!(matches!(outcome, BootstrapOutcome::UsersAlreadyExist));
            // Nothing was inserted: the store still holds exactly the seeded user.
            let stored = users.list().await.unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].email, "taken@example.com");
        }
    }

    #[tokio::test]
    async fn repository_failure_surfaces_as_bootstrap_error() {
        let err = bootstrap_admin(
            &FailingUserRepository,
            &FakePasswordHasher,
            admin("root@example.com", "Root", "correct-horse"),
            Utc::now(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BootstrapError::Repository(_)));
    }

    #[test]
    fn debug_does_not_contain_the_password() {
        let admin = admin("root@example.com", "Root", "hunter2-secret");
        let debug = format!("{admin:?}");
        assert!(!debug.contains("hunter2-secret"));
        assert!(debug.contains("<redacted>"));
    }
}
