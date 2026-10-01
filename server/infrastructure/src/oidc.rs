//! OpenID Connect provider adapter.
//!
//! Implements [`OidcProvider`] on top of the `openidconnect` crate using the
//! authorization-code flow with PKCE (S256) and a per-login nonce; the ID
//! token's signature, issuer, audience, expiry, and nonce are all checked by
//! the crate before any claim is read. Provider metadata comes from the
//! issuer's well-known discovery URL: configuration errors (a 4xx answer, a
//! malformed document, an issuer mismatch) fail [`OpenIdConnectProvider::connect`]
//! so startup aborts like an unreachable database does, while network errors
//! (connection failure, timeout, 5xx) only warn — discovery is retried lazily
//! on the next auth call until it succeeds.
//!
//! All configuration comes from environment variables. The client secret and
//! all tokens/codes stay out of logs and error messages: errors carry only
//! status codes, URLs, and provider-side text.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::ports::{OidcAuthRequest, OidcClaims, OidcError, OidcProvider, PendingOidcLogin};
use async_trait::async_trait;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm, CoreProviderMetadata,
};
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, DiscoveryError,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpClientError, IdToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default for `OIDC_DISPLAY_NAME`.
const DEFAULT_DISPLAY_NAME: &str = "Single sign-on";
/// Default for `OIDC_SCOPES`.
const DEFAULT_SCOPES: &str = "openid email profile";
/// Default for `OIDC_GROUPS_CLAIM`.
const DEFAULT_GROUPS_CLAIM: &str = "groups";
/// HTTP timeout for discovery and token-exchange calls.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// OIDC configuration, read entirely from the environment.
#[derive(Debug, Clone, PartialEq)]
pub struct OidcConfig {
    /// The provider's issuer URL (`OIDC_ISSUER_URL`); also the discovery base.
    pub issuer_url: url::Url,
    /// `OIDC_CLIENT_ID`.
    pub client_id: String,
    /// `OIDC_CLIENT_SECRET`. Never logged.
    pub client_secret: String,
    /// `OIDC_REDIRECT_URL`; must match a redirect URI registered at the provider.
    pub redirect_url: url::Url,
    /// `OIDC_DISPLAY_NAME`, defaulting to "Single sign-on".
    pub display_name: String,
    /// Requested scopes (`OIDC_SCOPES`, space-separated); always includes `openid`.
    pub scopes: Vec<String>,
    /// ID-token claim holding the user's groups (`OIDC_GROUPS_CLAIM`).
    pub groups_claim: String,
}

impl OidcConfig {
    /// Read the OIDC configuration from environment variables.
    ///
    /// `OIDC_ISSUER_URL` unset or empty means "OIDC disabled" and yields
    /// `Ok(None)`. Once the issuer is set, the other required variables must
    /// be too; a missing one — or an unparseable URL — is an `Err` that names
    /// the offending variable. (Compose passes unset vars through as empty
    /// strings, so an empty string means unset throughout.)
    pub fn from_env() -> Result<Option<OidcConfig>, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// The real parser, over an injectable lookup so tests never touch the
    /// process environment (and thus cannot race each other).
    pub(crate) fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Option<OidcConfig>, String> {
        let Some(issuer_raw) = non_empty(&mut lookup, "OIDC_ISSUER_URL") else {
            return Ok(None);
        };

        Ok(Some(OidcConfig {
            issuer_url: parse_url("OIDC_ISSUER_URL", &issuer_raw)?,
            client_id: required(&mut lookup, "OIDC_CLIENT_ID")?,
            client_secret: required(&mut lookup, "OIDC_CLIENT_SECRET")?,
            redirect_url: parse_url(
                "OIDC_REDIRECT_URL",
                &required(&mut lookup, "OIDC_REDIRECT_URL")?,
            )?,
            display_name: non_empty(&mut lookup, "OIDC_DISPLAY_NAME")
                .unwrap_or_else(|| DEFAULT_DISPLAY_NAME.to_owned()),
            scopes: parse_scopes(
                non_empty(&mut lookup, "OIDC_SCOPES")
                    .as_deref()
                    .unwrap_or(DEFAULT_SCOPES),
            ),
            groups_claim: non_empty(&mut lookup, "OIDC_GROUPS_CLAIM")
                .unwrap_or_else(|| DEFAULT_GROUPS_CLAIM.to_owned()),
        }))
    }
}

/// A variable that must be set once OIDC is enabled.
fn required(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Result<String, String> {
    non_empty(lookup, key).ok_or_else(|| format!("{key} must be set when OIDC_ISSUER_URL is set"))
}

/// A variable whose empty string means "unset".
fn non_empty(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Option<String> {
    lookup(key).filter(|value| !value.trim().is_empty())
}

fn parse_url(key: &str, raw: &str) -> Result<url::Url, String> {
    url::Url::parse(raw).map_err(|err| format!("{key} is not a valid URL ({raw:?}): {err}"))
}

/// Split a space-separated scope list, making sure `openid` is always present.
fn parse_scopes(raw: &str) -> Vec<String> {
    let mut scopes: Vec<String> = raw.split_whitespace().map(str::to_owned).collect();
    if !scopes.iter().any(|scope| scope == "openid") {
        scopes.insert(0, "openid".to_owned());
    }
    scopes
}

/// The client type `from_provider_metadata` produces: the authorization
/// endpoint is always present in provider metadata, the token and userinfo
/// endpoints optional.
type DiscoveredClient = CoreClient<
    EndpointSet,      // authorization endpoint
    EndpointNotSet,   // device authorization endpoint
    EndpointNotSet,   // introspection endpoint
    EndpointNotSet,   // revocation endpoint
    EndpointMaybeSet, // token endpoint
    EndpointMaybeSet, // userinfo endpoint
>;

/// A successful discovery: the ready-to-use client, shared by `Arc` so a lazy
/// retry rebuilds it at most once per process.
struct Discovered {
    client: DiscoveredClient,
}

enum Discovery {
    /// Discovery has not succeeded yet; the next auth call retries it.
    Undiscovered,
    Ready(Arc<Discovered>),
}

/// [`OidcProvider`] backed by the `openidconnect` crate.
pub struct OpenIdConnectProvider {
    config: OidcConfig,
    http: reqwest::Client,
    discovery: Mutex<Discovery>,
}

impl OpenIdConnectProvider {
    /// Build the provider and attempt discovery once.
    ///
    /// Configuration errors are returned so startup can fail fast; network
    /// errors return `Ok` with a loud warning instead, because the IdP may be
    /// temporarily down and discovery is retried on the next auth call.
    pub async fn connect(config: OidcConfig) -> Result<Self, OidcError> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            // Nowhere to redirect to: an IdP that bounces the discovery URL is
            // misbehaving, and following would send the request to other hosts.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| OidcError::Misconfigured(format!("HTTP client: {err}")))?;

        let provider = Self {
            config,
            http,
            discovery: Mutex::new(Discovery::Undiscovered),
        };
        match provider.ensure_discovered().await {
            Ok(_) => Ok(provider),
            Err(OidcError::Unavailable) => {
                eprintln!(
                    "warning: OIDC discovery failed at startup; identity-provider sign-in \
                     stays unavailable until a retry succeeds"
                );
                Ok(provider)
            }
            Err(err) => Err(err),
        }
    }

    /// Return the discovered client, discovering first if needed.
    ///
    /// The lock is held only to check and store state, never across the
    /// network call; concurrent first-time callers may discover twice, which
    /// is harmless (both results are equivalent).
    async fn ensure_discovered(&self) -> Result<Arc<Discovered>, OidcError> {
        if let Discovery::Ready(ref discovered) =
            *self.discovery.lock().expect("discovery lock poisoned")
        {
            return Ok(Arc::clone(discovered));
        }

        let discovered = Arc::new(self.discover_once().await?);
        let mut guard = self.discovery.lock().expect("discovery lock poisoned");
        if matches!(*guard, Discovery::Undiscovered) {
            *guard = Discovery::Ready(Arc::clone(&discovered));
        }
        Ok(discovered)
    }

    /// Fetch provider metadata once and build the client from it.
    async fn discover_once(&self) -> Result<Discovered, OidcError> {
        // Both URLs were validated at config time, so these cannot fail.
        let issuer = IssuerUrl::new(self.config.issuer_url.as_str().to_owned())
            .expect("validated issuer URL to re-parse");

        let metadata = CoreProviderMetadata::discover_async(issuer, &self.http)
            .await
            .map_err(|err| classify_discovery_error(&err))?;

        let client = DiscoveredClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            Some(ClientSecret::new(self.config.client_secret.clone())),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_url.as_str().to_owned())
                .expect("validated redirect URL to re-parse"),
        );

        Ok(Discovered { client })
    }
}

/// Map a discovery failure onto the port's error taxonomy.
///
/// The `Response` detail strings carry only status codes and URLs — never the
/// response body — so they are safe to surface in startup panics.
fn classify_discovery_error(err: &DiscoveryError<HttpClientError<reqwest::Error>>) -> OidcError {
    match err {
        // The provider answered, and what it said is unusable: a 4xx status, or
        // a 2xx/3xx that is not a usable discovery document (wrong content
        // type, a redirect we do not follow, ...). Retrying will not help.
        DiscoveryError::Response(status, _, detail) => {
            if status.is_server_error() {
                OidcError::Unavailable
            } else {
                OidcError::Misconfigured(detail.clone())
            }
        }
        // The provider could not be reached: transient.
        DiscoveryError::Request(_) => OidcError::Unavailable,
        // Malformed document or issuer mismatch: we (or the operator) got it wrong.
        DiscoveryError::Parse(_)
        | DiscoveryError::Validation(_)
        | DiscoveryError::UrlParse(_)
        | DiscoveryError::Other(_) => OidcError::Misconfigured(err.to_string()),
        // `DiscoveryError` is non_exhaustive; treat unknown failures as
        // transient so a crate upgrade cannot turn into a startup panic.
        _ => OidcError::Unavailable,
    }
}

/// Additional ID token claims: everything the standard claim set does not name,
/// kept as raw JSON so the configured groups claim can be read by its name.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ExtraIdTokenClaims {
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl AdditionalClaims for ExtraIdTokenClaims {}

/// The ID token re-parsed with [`ExtraIdTokenClaims`].
type GroupsIdToken = IdToken<
    ExtraIdTokenClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// Read the groups claim, tolerating an absent claim, a single string, or an
/// array of strings (non-string items are dropped).
fn extract_groups(claims: &ExtraIdTokenClaims, claim_name: &str) -> Vec<String> {
    match claims.extra.get(claim_name) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(Value::String(group)) => vec![group.clone()],
        _ => Vec::new(),
    }
}

#[async_trait]
impl OidcProvider for OpenIdConnectProvider {
    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    async fn authorization_request(&self) -> Result<OidcAuthRequest, OidcError> {
        let discovered = self.ensure_discovered().await?;

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, csrf_token, nonce) = discovered
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scopes(self.config.scopes.iter().cloned().map(Scope::new))
            .set_pkce_challenge(challenge)
            .url();

        Ok(OidcAuthRequest {
            authorization_url: url.to_string(),
            pending: PendingOidcLogin {
                csrf_state: csrf_token.secret().to_owned(),
                nonce: nonce.secret().to_owned(),
                pkce_verifier: verifier.secret().to_owned(),
            },
        })
    }

    async fn complete_login(
        &self,
        code: String,
        returned_state: String,
        pending: PendingOidcLogin,
    ) -> Result<OidcClaims, OidcError> {
        let discovered = self.ensure_discovered().await?;

        // ponytail: plain comparison, not constant-time. The state is a fresh
        // 128-bit random per login; byte-level timing over the network is not
        // a realistic channel for it.
        if returned_state != pending.csrf_state {
            return Err(OidcError::StateMismatch);
        }

        let request = discovered
            .client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|err| OidcError::Misconfigured(err.to_string()))?;
        // The error text may quote the provider's response body, so it is
        // returned to the caller but never logged here.
        let token_response = request
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|err| OidcError::ExchangeFailed(err.to_string()))?;

        let id_token = token_response
            .id_token()
            .ok_or_else(|| OidcError::InvalidIdToken("no ID token in the token response".into()))?;

        // Re-parse the same JWT with a claims type that keeps every non-standard
        // claim, so the configured groups claim is reachable by name. The
        // verification below re-checks signature/issuer/audience/expiry/nonce
        // on this parse.
        let raw = serde_json::to_value(id_token)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| OidcError::InvalidIdToken("ID token is not a compact JWT".into()))?;
        let groups_token: GroupsIdToken = raw
            .parse()
            .map_err(|err| OidcError::InvalidIdToken(format!("unparsable ID token: {err}")))?;

        let claims = groups_token
            .claims(
                &discovered.client.id_token_verifier(),
                &Nonce::new(pending.nonce),
            )
            .map_err(|err| OidcError::InvalidIdToken(err.to_string()))?;

        Ok(OidcClaims {
            // The verifier already checked `iss` against the discovery issuer.
            issuer: self.config.issuer_url.as_str().to_owned(),
            subject: claims.subject().as_str().to_owned(),
            email: claims.email().map(|email| email.as_str().to_owned()),
            email_verified: claims.email_verified().unwrap_or(false),
            display_name: claims
                .name()
                .and_then(|name| name.get(None))
                .map(|name| name.as_str().to_owned())
                .or_else(|| {
                    claims
                        .preferred_username()
                        .map(|username| username.as_str().to_owned())
                }),
            groups: extract_groups(claims.additional_claims(), &self.config.groups_claim),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    const FULL_ENV: [(&str, &str); 4] = [
        ("OIDC_ISSUER_URL", "https://idp.example"),
        ("OIDC_CLIENT_ID", "client-id"),
        ("OIDC_CLIENT_SECRET", "secret"),
        ("OIDC_REDIRECT_URL", "https://app.example/oidc/callback"),
    ];

    #[test]
    fn from_lookup_disabled_when_issuer_unset_or_empty() {
        assert_eq!(OidcConfig::from_lookup(lookup_from(&[])), Ok(None));
        assert_eq!(
            OidcConfig::from_lookup(lookup_from(&[("OIDC_ISSUER_URL", "")])),
            Ok(None)
        );
        assert_eq!(
            OidcConfig::from_lookup(lookup_from(&[("OIDC_ISSUER_URL", "   ")])),
            Ok(None)
        );
    }

    #[test]
    fn from_lookup_requires_every_variable_once_issuer_is_set() {
        for missing in ["OIDC_CLIENT_ID", "OIDC_CLIENT_SECRET", "OIDC_REDIRECT_URL"] {
            let pairs: Vec<(&str, &str)> = FULL_ENV
                .iter()
                .copied()
                .filter(|(key, _)| *key != missing)
                .collect();
            let err = OidcConfig::from_lookup(lookup_from(&pairs)).unwrap_err();
            assert!(err.contains(missing), "error should name {missing}: {err}");
        }
    }

    #[test]
    fn from_lookup_rejects_unparseable_urls() {
        let pairs = [
            ("OIDC_ISSUER_URL", "not a url"),
            ("OIDC_CLIENT_ID", "client-id"),
            ("OIDC_CLIENT_SECRET", "secret"),
            ("OIDC_REDIRECT_URL", "https://app.example/oidc/callback"),
        ];
        let err = OidcConfig::from_lookup(lookup_from(&pairs)).unwrap_err();
        assert!(err.contains("OIDC_ISSUER_URL"), "{err}");

        let pairs = [
            ("OIDC_ISSUER_URL", "https://idp.example"),
            ("OIDC_CLIENT_ID", "client-id"),
            ("OIDC_CLIENT_SECRET", "secret"),
            ("OIDC_REDIRECT_URL", "not a url"),
        ];
        let err = OidcConfig::from_lookup(lookup_from(&pairs)).unwrap_err();
        assert!(err.contains("OIDC_REDIRECT_URL"), "{err}");
    }

    #[test]
    fn from_lookup_applies_defaults() {
        let config = OidcConfig::from_lookup(lookup_from(&FULL_ENV))
            .unwrap()
            .expect("config");
        assert_eq!(config.display_name, DEFAULT_DISPLAY_NAME);
        assert_eq!(
            config.scopes,
            vec![
                "openid".to_owned(),
                "email".to_owned(),
                "profile".to_owned()
            ]
        );
        assert_eq!(config.groups_claim, DEFAULT_GROUPS_CLAIM);
    }

    #[test]
    fn from_lookup_always_keeps_openid_scope() {
        let pairs: [(&str, &str); 5] = [
            FULL_ENV[0],
            FULL_ENV[1],
            FULL_ENV[2],
            FULL_ENV[3],
            ("OIDC_SCOPES", "email   profile"),
        ];
        let config = OidcConfig::from_lookup(lookup_from(&pairs))
            .unwrap()
            .expect("config");
        assert_eq!(
            config.scopes,
            vec![
                "openid".to_owned(),
                "email".to_owned(),
                "profile".to_owned()
            ]
        );

        let pairs: [(&str, &str); 5] = [
            FULL_ENV[0],
            FULL_ENV[1],
            FULL_ENV[2],
            FULL_ENV[3],
            ("OIDC_SCOPES", "openid email"),
        ];
        let config = OidcConfig::from_lookup(lookup_from(&pairs))
            .unwrap()
            .expect("config");
        assert_eq!(config.scopes, vec!["openid".to_owned(), "email".to_owned()]);
    }

    fn extra(claim: &str, value: Value) -> ExtraIdTokenClaims {
        let mut claims = ExtraIdTokenClaims::default();
        claims.extra.insert(claim.to_owned(), value);
        claims
    }

    #[test]
    fn extract_groups_handles_array_string_and_absent() {
        assert_eq!(
            extract_groups(
                &extra(
                    "groups",
                    Value::Array(vec![Value::String("a".into()), Value::String("b".into())])
                ),
                "groups"
            ),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert_eq!(
            extract_groups(&extra("groups", Value::String("solo".into())), "groups"),
            vec!["solo".to_owned()]
        );
        assert!(extract_groups(&ExtraIdTokenClaims::default(), "groups").is_empty());
    }

    #[test]
    fn extract_groups_uses_configured_claim_name_and_drops_non_strings() {
        let claims = extra(
            "app_roles",
            Value::Array(vec![Value::String("admin".into()), Value::Number(1.into())]),
        );
        assert_eq!(
            extract_groups(&claims, "app_roles"),
            vec!["admin".to_owned()]
        );
        assert!(extract_groups(&claims, "groups").is_empty());
    }
}
