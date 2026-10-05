//! The auth provider contract: how a sign-in method identifies itself and
//! turns credentials (or a redirect round-trip) into a [`User`].
//!
//! Adding a login method later means implementing one of the traits here and
//! registering it in an [`AuthProviders`] registry — session handling and the
//! existing providers never change.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;

use domain::User;

use crate::ports::RepositoryError;

/// A sign-in method: credential-based ([`CredentialProvider`]) or a redirect
/// round-trip through an external service ([`RedirectProvider`]).
pub trait AuthProvider: Send + Sync {
    /// Stable slug identifying the provider (e.g. `"password"`). Validated by
    /// [`AuthProviders::new`]; it appears in URLs and configuration, so it
    /// never changes once deployed.
    fn id(&self) -> &str;

    /// Human-readable name for UIs that list sign-in methods.
    fn display_name(&self) -> &str;
}

/// An identifier plus the secret that authenticates it (an email address and
/// password today; other providers may use user names or one-time codes).
#[derive(Clone)]
pub struct Credentials {
    pub identifier: String,
    pub secret: String,
}

impl fmt::Debug for Credentials {
    /// The secret is redacted: a formatted `Credentials` must never reach a
    /// log with the password in it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("identifier", &self.identifier)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// A provider that exchanges an identifier and secret for a user in one
/// round-trip.
#[async_trait::async_trait]
pub trait CredentialProvider: AuthProvider {
    /// Authenticate `credentials`, returning the account they belong to.
    /// Every rejection must be [`AuthError::InvalidCredentials`]: the answer
    /// must not reveal whether the identifier exists or which check failed.
    async fn authenticate(&self, credentials: Credentials) -> Result<User, AuthError>;
}

/// Opaque provider-defined state carried between a redirect start and its
/// callback (e.g. an OIDC `state` value). Only the provider that created it
/// interprets it; the registry treats it as opaque.
#[derive(Debug, Clone)]
pub struct PendingLogin(pub BTreeMap<String, String>);

/// Query parameters arriving at a redirect provider's callback.
pub type CallbackParams = BTreeMap<String, String>;

/// The first half of a redirect login: where to send the browser and the
/// state to verify when it comes back.
#[derive(Debug, Clone)]
pub struct RedirectStart {
    pub redirect_url: String,
    pub pending: PendingLogin,
}

/// A provider that signs the user in over a redirect round-trip (e.g. OIDC).
#[async_trait::async_trait]
pub trait RedirectProvider: AuthProvider {
    /// Start the flow: the URL to send the browser to and the state to check
    /// on the callback.
    async fn begin(&self) -> Result<RedirectStart, AuthError>;

    /// Finish the flow with the callback parameters, checking `pending`
    /// against what [`Self::begin`] produced.
    async fn complete(
        &self,
        callback: CallbackParams,
        pending: PendingLogin,
    ) -> Result<User, AuthError>;
}

/// An error from an auth provider. The rejection variants are safe to render
/// as one generic 401; `Failed`'s detail and `Internal` are for server logs
/// only and must never be shown to clients.
#[derive(Debug)]
pub enum AuthError {
    /// The credentials were wrong, or the account has no password.
    InvalidCredentials,
    /// The provider refused for a reason it can name with a stable code (e.g.
    /// an IdP error code), not user-facing text.
    Rejected { code: &'static str },
    /// The provider failed; `detail` is worth logging but never showing.
    Failed { code: &'static str, detail: String },
    /// An internal failure (e.g. the database was unreachable).
    Internal(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::InvalidCredentials => write!(f, "invalid credentials"),
            AuthError::Rejected { code } => write!(f, "rejected: {code}"),
            AuthError::Failed { code, detail } => write!(f, "failed: {code}: {detail}"),
            AuthError::Internal(detail) => write!(f, "internal error: {detail}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<RepositoryError> for AuthError {
    fn from(error: RepositoryError) -> Self {
        AuthError::Internal(error.to_string())
    }
}

/// The registered sign-in methods. Built once at startup; handlers look a
/// provider up by id and never name a concrete one.
#[derive(Default)]
pub struct AuthProviders {
    credentials: Vec<Arc<dyn CredentialProvider>>,
    redirects: Vec<Arc<dyn RedirectProvider>>,
}

impl AuthProviders {
    /// Register the providers. Rejects ids that are not stable slugs
    /// (`[a-z0-9_-]+`) and ids that appear twice across both lists.
    pub fn new(
        credentials: Vec<Arc<dyn CredentialProvider>>,
        redirects: Vec<Arc<dyn RedirectProvider>>,
    ) -> Result<Self, String> {
        let mut seen = HashSet::new();
        for provider in &credentials {
            validate_id(provider.id(), &mut seen)?;
        }
        for provider in &redirects {
            validate_id(provider.id(), &mut seen)?;
        }
        Ok(Self {
            credentials,
            redirects,
        })
    }

    /// The credential provider registered under `id`, if any.
    pub fn credential(&self, id: &str) -> Option<&Arc<dyn CredentialProvider>> {
        self.credentials.iter().find(|provider| provider.id() == id)
    }

    /// The redirect provider registered under `id`, if any.
    pub fn redirect(&self, id: &str) -> Option<&Arc<dyn RedirectProvider>> {
        self.redirects.iter().find(|provider| provider.id() == id)
    }

    /// Every registered provider for clients to list: credential providers
    /// first, then redirect providers, each in registration order.
    pub fn list(&self) -> Vec<ProviderInfo> {
        let mut infos: Vec<ProviderInfo> = self
            .credentials
            .iter()
            .map(|provider| ProviderInfo {
                id: provider.id().to_owned(),
                display_name: provider.display_name().to_owned(),
                kind: ProviderKind::Credentials,
            })
            .collect();
        infos.extend(self.redirects.iter().map(|provider| ProviderInfo {
            id: provider.id().to_owned(),
            display_name: provider.display_name().to_owned(),
            kind: ProviderKind::Redirect,
        }));
        infos
    }
}

/// One registered provider as listed to clients: identity and name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
    pub kind: ProviderKind,
}

/// How a provider signs the user in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// One round-trip with an identifier and secret.
    Credentials,
    /// A redirect to an external service and back.
    Redirect,
}

fn validate_id<'a>(id: &'a str, seen: &mut HashSet<&'a str>) -> Result<(), String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        return Err(format!(
            "invalid provider id {id:?}: must match [a-z0-9_-]+"
        ));
    }
    if !seen.insert(id) {
        return Err(format!("duplicate provider id {id:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::{FakePasswordHasher, InMemoryUserRepository};
    use crate::auth::password::PasswordAuthProvider;
    use chrono::Utc;
    use domain::{Role, UserId};

    /// A throwaway [`CredentialProvider`] standing in for a future login
    /// method: any id, and it always authenticates to one fixed user.
    struct MagicProvider {
        id: &'static str,
    }

    impl AuthProvider for MagicProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn display_name(&self) -> &str {
            "Magic"
        }
    }

    #[async_trait::async_trait]
    impl CredentialProvider for MagicProvider {
        async fn authenticate(&self, _credentials: Credentials) -> Result<User, AuthError> {
            Ok(User {
                id: UserId::new(),
                email: "magic@example.com".to_owned(),
                password_hash: None,
                display_name: "Magic user".to_owned(),
                role: Role::Admin,
                deactivated_at: None,
                role_managed_by_sso: false,
                sso_role_exempt: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
        }
    }

    /// A stand-in [`RedirectProvider`]; the registry tests only register and
    /// look it up, never run a flow.
    struct FakeRedirectProvider {
        id: &'static str,
    }

    impl AuthProvider for FakeRedirectProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn display_name(&self) -> &str {
            "Fake redirect"
        }
    }

    #[async_trait::async_trait]
    impl RedirectProvider for FakeRedirectProvider {
        async fn begin(&self) -> Result<RedirectStart, AuthError> {
            Ok(RedirectStart {
                redirect_url: "https://idp.example/authorize".to_owned(),
                pending: PendingLogin(BTreeMap::new()),
            })
        }

        async fn complete(
            &self,
            _callback: CallbackParams,
            _pending: PendingLogin,
        ) -> Result<User, AuthError> {
            Err(AuthError::Internal(
                "test fake does not complete flows".to_owned(),
            ))
        }
    }

    #[test]
    fn credentials_debug_redacts_the_secret() {
        let credentials = Credentials {
            identifier: "user@example.com".to_owned(),
            secret: "hunter2".to_owned(),
        };
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("hunter2"), "secret leaked into Debug");
        assert!(debug.contains("user@example.com"));
    }

    #[test]
    fn duplicate_id_across_kinds_is_rejected() {
        let providers = AuthProviders::new(
            vec![Arc::new(MagicProvider { id: "magic" })],
            vec![Arc::new(FakeRedirectProvider { id: "magic" })],
        );
        assert!(providers.is_err());
    }

    #[test]
    fn duplicate_id_within_a_list_is_rejected() {
        let providers = AuthProviders::new(
            vec![
                Arc::new(MagicProvider { id: "magic" }),
                Arc::new(MagicProvider { id: "magic" }),
            ],
            Vec::new(),
        );
        assert!(providers.is_err());
    }

    #[test]
    fn invalid_slugs_are_rejected() {
        for id in ["", "Password", "with space", "UPPER", "dot.ted"] {
            let providers = AuthProviders::new(vec![Arc::new(MagicProvider { id })], Vec::new());
            assert!(providers.is_err(), "id {id:?} should be rejected");
        }
    }

    #[test]
    fn lookup_by_kind_returns_the_right_provider() {
        let providers = AuthProviders::new(
            vec![Arc::new(MagicProvider { id: "magic" })],
            vec![Arc::new(FakeRedirectProvider { id: "oidc" })],
        )
        .unwrap();
        assert!(providers.credential("magic").is_some());
        assert!(providers.credential("oidc").is_none());
        assert!(providers.redirect("oidc").is_some());
        assert!(providers.redirect("magic").is_none());
    }

    #[test]
    fn list_is_credentials_first_in_registration_order() {
        let providers = AuthProviders::new(
            vec![
                Arc::new(MagicProvider { id: "second" }),
                Arc::new(MagicProvider { id: "first" }),
            ],
            vec![Arc::new(FakeRedirectProvider { id: "redirect" })],
        )
        .unwrap();
        let infos = providers.list();
        assert_eq!(
            infos
                .iter()
                .map(|info| (info.id.as_str(), info.kind))
                .collect::<Vec<_>>(),
            vec![
                ("second", ProviderKind::Credentials),
                ("first", ProviderKind::Credentials),
                ("redirect", ProviderKind::Redirect),
            ]
        );
    }

    #[tokio::test]
    async fn a_second_credential_provider_is_found_by_id() {
        // The point of the registry: a new login method implements the trait
        // and registers itself; nothing else changes.
        let users = Arc::new(InMemoryUserRepository::new());
        let providers = AuthProviders::new(
            vec![
                Arc::new(PasswordAuthProvider::new(
                    users,
                    Arc::new(FakePasswordHasher),
                )),
                Arc::new(MagicProvider { id: "magic" }),
            ],
            Vec::new(),
        )
        .unwrap();
        let provider = providers.credential("magic").expect("registered by id");
        let user = provider
            .authenticate(Credentials {
                identifier: "anyone".to_owned(),
                secret: "anything".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(user.email, "magic@example.com");
    }
}
