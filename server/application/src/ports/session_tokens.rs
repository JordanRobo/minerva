//! Port for generating raw session tokens and computing their stored form.

/// Generates fresh raw session tokens and computes the form that is stored
/// and looked up server-side.
pub trait SessionTokens: Send + Sync {
    /// A fresh random raw token, safe to show to the client exactly once.
    fn generate(&self) -> String;

    /// The stored/lookup form of a raw token (one-way).
    fn hash(&self, token: &str) -> String;
}
