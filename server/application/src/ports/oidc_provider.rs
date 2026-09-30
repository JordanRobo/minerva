//! Port for an external OpenID Connect (OIDC) identity provider.
//!
//! Lets users sign in through a provider (a self-hosted Authentik in local
//! dev) alongside the built-in email/password login. The infrastructure layer
//! implements it; no OIDC wire details leak into this port.

/// Claims about an authenticated user, extracted from a verified ID token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcClaims {
    /// The provider that issued the ID token (the OIDC `iss` claim).
    pub issuer: String,
    /// The user's unique identifier at the provider (the OIDC `sub` claim).
    pub subject: String,
    /// The email the provider reported for the identity, if any.
    pub email: Option<String>,
    /// Whether the provider has verified that email; false when the claim is absent.
    pub email_verified: bool,
    /// A human-readable name from the provider (`name`, falling back to
    /// `preferred_username`), if any.
    pub display_name: Option<String>,
    /// The groups the user belongs to at the provider. Parsed now and unused
    /// for now; a future group-to-role mapping can read it without a port change.
    pub groups: Vec<String>,
}

/// Per-login state that must be kept between the redirect to the IdP and its
/// callback, so [`OidcProvider::complete_login`] can verify what comes back.
/// Plain strings on purpose: how it is persisted across requests (cookie,
/// session store, ...) is the interface layer's decision.
#[derive(Debug, Clone)]
pub struct PendingOidcLogin {
    /// The `state` value sent to the IdP; the callback must return it unchanged.
    pub csrf_state: String,
    /// The `nonce` sent to the IdP; the ID token must carry it back.
    pub nonce: String,
    /// The PKCE verifier whose S256 challenge was sent to the IdP.
    pub pkce_verifier: String,
}

/// Everything the interface layer needs to send a browser to the IdP.
#[derive(Debug, Clone)]
pub struct OidcAuthRequest {
    /// The full URL to redirect the user's browser to.
    pub authorization_url: String,
    /// The per-login state to persist until the callback arrives.
    pub pending: PendingOidcLogin,
}

/// An error from an OIDC operation.
#[derive(Debug)]
pub enum OidcError {
    /// The IdP is unreachable or discovery has not succeeded yet.
    Unavailable,
    /// The `state` returned by the IdP does not match the one we sent.
    StateMismatch,
    /// The authorization-code to token exchange failed.
    ExchangeFailed(String),
    /// The ID token failed validation (signature, issuer, audience, expiry, nonce).
    InvalidIdToken(String),
    /// The provider is misconfigured.
    Misconfigured(String),
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Unavailable => write!(f, "identity provider is unavailable"),
            OidcError::StateMismatch => write!(f, "the returned state did not match"),
            OidcError::ExchangeFailed(detail) => write!(f, "token exchange failed: {detail}"),
            OidcError::InvalidIdToken(detail) => write!(f, "invalid ID token: {detail}"),
            OidcError::Misconfigured(detail) => write!(f, "OIDC is misconfigured: {detail}"),
        }
    }
}

impl std::error::Error for OidcError {}

/// An external OpenID Connect identity provider.
#[async_trait::async_trait]
pub trait OidcProvider: Send + Sync {
    /// A human-readable name for this provider (e.g. shown on the login screen).
    fn display_name(&self) -> &str;

    /// Build the URL to redirect a browser to, minting fresh state/nonce/PKCE values.
    async fn authorization_request(&self) -> Result<OidcAuthRequest, OidcError>;

    /// Finish a login started by [`OidcProvider::authorization_request`]: check the
    /// returned `state`, exchange the callback's `code` for tokens, and validate
    /// the ID token against the pending state.
    async fn complete_login(
        &self,
        code: String,
        returned_state: String,
        pending: PendingOidcLogin,
    ) -> Result<OidcClaims, OidcError>;
}
