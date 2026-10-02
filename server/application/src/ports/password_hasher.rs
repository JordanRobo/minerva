//! Port for hashing and verifying user passwords.

/// An error from hashing or verifying a password.
#[derive(Debug)]
pub enum PasswordHashError {
    /// The hash or verify operation failed (e.g. an unparseable stored hash).
    OperationFailed(String),
}

impl std::fmt::Display for PasswordHashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordHashError::OperationFailed(detail) => {
                write!(f, "password hash operation failed: {detail}")
            }
        }
    }
}

impl std::error::Error for PasswordHashError {}

/// Hashes and verifies user passwords. The operations are async because the
/// implementations run CPU-bound work off the async runtime's worker threads.
#[async_trait::async_trait]
pub trait PasswordHasher: Send + Sync {
    /// Hash a plaintext password into its stored representation.
    async fn hash(&self, password: &str) -> Result<String, PasswordHashError>;

    /// Check a plaintext password against a stored hash.
    async fn verify(&self, password: &str, hash: &str) -> Result<bool, PasswordHashError>;
}
