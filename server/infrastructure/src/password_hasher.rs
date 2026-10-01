//! Argon2id implementation of [`PasswordHasher`].

use application::ports::{PasswordHashError, PasswordHasher};
use argon2::password_hash::{PasswordHash, PasswordVerifier, SaltString, rand_core::OsRng};
// The hashing side of the argon2 API; imported anonymously because its name
// collides with the application-layer port trait.
use argon2::Argon2;
use argon2::password_hash::PasswordHasher as _;

/// [`PasswordHasher`] backed by the `argon2` crate.
///
/// Uses `Argon2::default()` — Argon2id with the crate's default cost
/// parameters (19 MiB, 2 iterations), the OWASP-recommended baseline.
pub struct Argon2PasswordHasher;

impl PasswordHasher for Argon2PasswordHasher {
    fn hash(&self, password: &str) -> Result<String, PasswordHashError> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hashed| hashed.to_string())
            .map_err(|err| PasswordHashError::OperationFailed(err.to_string()))
    }

    fn verify(&self, password: &str, hash: &str) -> Result<bool, PasswordHashError> {
        let parsed = PasswordHash::new(hash).map_err(|err| {
            PasswordHashError::OperationFailed(format!("unparseable stored hash: {err}"))
        })?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }
}
