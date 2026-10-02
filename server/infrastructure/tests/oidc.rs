//! End-to-end discovery test against a real OpenID Connect provider — any
//! conforming IdP works, not just Authentik.
//!
//! Skipped unless `MINERVA_OIDC__ISSUER_URL` is set; see
//! `minerva.example.toml` at the repo root for what each variable does. This
//! test is one of the sanctioned exceptions that reads the environment
//! directly: it drives a real IdP, so its credentials come from the runner.

use application::ports::OidcProvider;
use infrastructure::oidc::{OidcConfig, OpenIdConnectProvider};

#[tokio::test]
async fn discovery_builds_authorization_url() {
    let issuer = std::env::var("MINERVA_OIDC__ISSUER_URL").unwrap_or_default();
    if issuer.trim().is_empty() {
        eprintln!("skipping: MINERVA_OIDC__ISSUER_URL not set");
        return;
    }

    let config = OidcConfig {
        issuer_url: url::Url::parse(&issuer).expect("MINERVA_OIDC__ISSUER_URL is not a URL"),
        client_id: std::env::var("MINERVA_OIDC__CLIENT_ID")
            .expect("MINERVA_OIDC__ISSUER_URL is set, so MINERVA_OIDC__CLIENT_ID must be too"),
        client_secret: std::env::var("MINERVA_OIDC__CLIENT_SECRET")
            .expect("MINERVA_OIDC__ISSUER_URL is set, so MINERVA_OIDC__CLIENT_SECRET must be too"),
        redirect_url: url::Url::parse(
            &std::env::var("MINERVA_OIDC__REDIRECT_URL").expect(
                "MINERVA_OIDC__ISSUER_URL is set, so MINERVA_OIDC__REDIRECT_URL must be too",
            ),
        )
        .expect("MINERVA_OIDC__REDIRECT_URL is not a URL"),
        // The discovery test only exercises the provider call; the display
        // settings stay at their defaults.
        display_name: "Single sign-on".to_owned(),
        scopes: vec!["openid".into(), "email".into(), "profile".into()],
        groups_claim: "groups".to_owned(),
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
