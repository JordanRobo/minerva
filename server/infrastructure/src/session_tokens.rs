//! SHA-256 implementation of [`SessionTokens`].

use application::ports::SessionTokens;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// [`SessionTokens`] with random UUIDv4 raw tokens and lowercase-hex SHA-256
/// hashes. One-way hashing is all that is needed: the threat is a leaked
/// database revealing live tokens, not an attacker computing hashes to compare.
pub struct Sha256SessionTokens;

impl SessionTokens for Sha256SessionTokens {
    fn generate(&self) -> String {
        Uuid::new_v4().to_string()
    }

    fn hash(&self, token: &str) -> String {
        let digest = Sha256::digest(token.as_bytes());
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
