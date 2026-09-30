//! End-to-end discovery test against a real OpenID Connect provider.
//!
//! Skipped unless the OIDC_* variables are set; in local dev, point them at
//! the self-hosted Authentik (see deploy/docker-compose.yml for the names).

use application::ports::OidcProvider;
use infrastructure::oidc::{OidcConfig, OpenIdConnectProvider};

#[tokio::test]
async fn discovery_builds_authorization_url() {
    let Some(config) = OidcConfig::from_env().expect("OIDC config should parse") else {
        eprintln!("skipping: OIDC_ISSUER_URL not set");
        return;
    };

    let provider = OpenIdConnectProvider::connect(config)
        .await
        .expect("discovery should succeed against the configured provider");

    let request = provider
        .authorization_request()
        .await
        .expect("authorization request should build");

    // The URL is not printed on failure: it carries per-login state, nonce,
    // and PKCE values.
    for param in [
        "code_challenge=",
        "code_challenge_method=S256",
        "state=",
        "nonce=",
    ] {
        assert!(
            request.authorization_url.contains(param),
            "authorization URL is missing {param}"
        );
    }
}
