//! OIDC sign-in as a [`RedirectProvider`]: wraps the protocol-level
//! [`OidcProvider`] port and applies the login/link/create policy so the HTTP
//! layer only ever sees "redirect here" and "complete with these parameters".
//!
//! The one-time state/nonce/PKCE verification stays in the infrastructure's
//! `OidcProvider` implementation; this provider decides *who* is logging in
//! (the same rules as before the abstraction, see [`decide_login`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use domain::{AccountTokenKind, User, UserId};

use crate::auth::normalize_email;
use crate::auth::provider::{
    AuthError, AuthProvider, CallbackParams, PendingLogin, RedirectProvider, RedirectStart,
};
use crate::oidc_login::{
    LoginDecision, LoginPolicy, decide_login, identity_from_claims, new_user_from_claims,
};
use crate::ports::{
    AccountTokenRepository, OidcClaims, OidcError, OidcProvider, PendingOidcLogin, RepositoryError,
    UserIdentityRepository, UserRepository,
};

/// Keys under which the one-time OIDC values ride in [`PendingLogin`].
const CSRF_STATE_KEY: &str = "csrf_state";
const NONCE_KEY: &str = "nonce";
const PKCE_VERIFIER_KEY: &str = "pkce_verifier";

pub struct OidcAuthProvider {
    oidc: Arc<dyn OidcProvider>,
    users: Arc<dyn UserRepository>,
    identities: Arc<dyn UserIdentityRepository>,
    invites: Arc<dyn AccountTokenRepository>,
    policy: LoginPolicy,
}

impl OidcAuthProvider {
    pub fn new(
        oidc: Arc<dyn OidcProvider>,
        users: Arc<dyn UserRepository>,
        identities: Arc<dyn UserIdentityRepository>,
        invites: Arc<dyn AccountTokenRepository>,
        policy: LoginPolicy,
    ) -> Self {
        Self {
            oidc,
            users,
            identities,
            invites,
            policy,
        }
    }

    /// Create the (issuer, subject) -> user link. A `Conflict` means a
    /// concurrent callback for the same provider identity won the race;
    /// re-look it up and carry on.
    async fn create_identity(
        &self,
        user_id: UserId,
        claims: &OidcClaims,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        match self
            .identities
            .create(identity_from_claims(user_id, claims, now))
            .await
        {
            Ok(_) => Ok(()),
            Err(RepositoryError::Conflict(_)) => {
                let found = self
                    .identities
                    .find_by_issuer_and_subject(claims.issuer.clone(), claims.subject.clone())
                    .await?;
                if found.is_some() {
                    Ok(())
                } else {
                    Err(AuthError::Internal(
                        "identity creation raced and the re-lookup failed".to_owned(),
                    ))
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl AuthProvider for OidcAuthProvider {
    fn id(&self) -> &str {
        "oidc"
    }

    fn display_name(&self) -> &str {
        self.oidc.display_name()
    }
}

#[async_trait::async_trait]
impl RedirectProvider for OidcAuthProvider {
    async fn begin(&self) -> Result<RedirectStart, AuthError> {
        let request = self
            .oidc
            .authorization_request()
            .await
            .map_err(map_oidc_error)?;
        let mut pending = BTreeMap::new();
        pending.insert(CSRF_STATE_KEY.to_owned(), request.pending.csrf_state);
        pending.insert(NONCE_KEY.to_owned(), request.pending.nonce);
        pending.insert(PKCE_VERIFIER_KEY.to_owned(), request.pending.pkce_verifier);
        Ok(RedirectStart {
            redirect_url: request.authorization_url,
            pending: PendingLogin(pending),
        })
    }

    async fn complete(
        &self,
        callback: CallbackParams,
        pending: PendingLogin,
    ) -> Result<User, AuthError> {
        // The provider reported a failure before we ever got a code.
        if let Some(error) = callback.get("error") {
            let detail = callback
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("no description");
            return Err(AuthError::Failed {
                code: "oidc_provider_error",
                detail: format!("provider error {error}: {detail}"),
            });
        }

        let (Some(code), Some(returned_state)) = (callback.get("code"), callback.get("state"))
        else {
            return Err(AuthError::Failed {
                code: "oidc_login_failed",
                detail: "callback is missing the code or state parameter".to_owned(),
            });
        };

        let (csrf_state, nonce, pkce_verifier) = match (
            pending.0.get(CSRF_STATE_KEY),
            pending.0.get(NONCE_KEY),
            pending.0.get(PKCE_VERIFIER_KEY),
        ) {
            (Some(csrf_state), Some(nonce), Some(pkce_verifier)) => {
                (csrf_state.clone(), nonce.clone(), pkce_verifier.clone())
            }
            _ => {
                return Err(AuthError::Internal(
                    "pending login is missing its OIDC state".to_owned(),
                ));
            }
        };

        let claims = self
            .oidc
            .complete_login(
                code.clone(),
                returned_state.clone(),
                PendingOidcLogin {
                    csrf_state,
                    nonce,
                    pkce_verifier,
                },
            )
            .await
            .map_err(map_oidc_error)?;

        // Who is logging in? The identity lookup always runs; the email lookup
        // only when it could matter (no known identity and a verified, non-blank
        // email claim), mirroring decide_login's rule order.
        let identity = self
            .identities
            .find_by_issuer_and_subject(claims.issuer.clone(), claims.subject.clone())
            .await?;

        let user_with_email = match (&identity, &claims.email) {
            (None, Some(email)) if claims.email_verified => {
                let email = normalize_email(email);
                if email.is_empty() {
                    None
                } else {
                    self.users.find_by_email(email).await?
                }
            }
            _ => None,
        };

        let now = Utc::now();
        let user = match decide_login(
            self.policy,
            &claims,
            identity.as_ref(),
            user_with_email.as_ref(),
        ) {
            LoginDecision::ExistingIdentity { user_id } => {
                self.users.find_by_id(user_id).await?.ok_or_else(|| {
                    AuthError::Internal(format!(
                        "user {} behind a known identity no longer exists",
                        user_id.0
                    ))
                })?
            }
            LoginDecision::LinkToExistingUser { user_id } => {
                // decide_login only links when the verified email matched a user.
                let user = user_with_email.expect("a link decision requires the matched user");
                self.create_identity(user_id, &claims, now).await?;
                user
            }
            LoginDecision::CreateUser => {
                let mut user = new_user_from_claims(&claims, now);
                // decide_login only reaches CreateUser with a verified,
                // non-blank email claim. That IdP-verified email is proof this
                // is the invited person: a pending (unexpired, unrevoked,
                // unconsumed) invite for it upgrades the new account to the
                // invite's role and is consumed below so it shows as accepted
                // in the invites list. Expired or revoked invites are not
                // returned by the lookup, so they are ignored (the default role
                // stands). An invite never enables SSO signup on its own — with
                // auto_create_users off, decide_login rejects before we get
                // here.
                let email = claims
                    .email
                    .clone()
                    .expect("a create decision requires an email claim");
                let pending_invite = self
                    .invites
                    .find_pending_invite_for_email(normalize_email(&email), now)
                    .await?;
                if let Some(invite) = &pending_invite
                    && let AccountTokenKind::Invite { role, .. } = &invite.kind
                {
                    user.role = *role;
                }
                let user = self.users.create(user).await?;
                // ponytail: creating the user and its identity is two repository
                // calls with no cross-repo transaction; if the second fails, a
                // passwordless user stays behind and the next login re-links it
                // via its verified email. Belongs in an application-layer unit of
                // work when one exists.
                self.create_identity(user.id, &claims, now).await?;
                if let Some(invite) = pending_invite {
                    // ponytail: user creation, identity creation and this token
                    // consumption are separate repository calls with no cross-repo
                    // transaction (consistent with the note above); a failed
                    // consume leaves a harmless pending invite whose accept will
                    // return account_exists. A `false` result means a concurrent
                    // accept won the token; by then the user already existed, so
                    // the create above would have conflicted and this path ends in
                    // the existing error handling.
                    let _ = self.invites.consume(invite.id, now).await;
                }
                user
            }
            LoginDecision::Reject(rejection) => {
                return Err(AuthError::Rejected {
                    code: rejection.code(),
                });
            }
        };

        // A deactivated account cannot sign in via SSO either.
        if !user.is_active() {
            return Err(AuthError::Rejected {
                code: "deactivated",
            });
        }
        Ok(user)
    }
}

/// Every OIDC protocol failure becomes a provider-level [`AuthError::Failed`]:
/// an unreachable IdP keeps its own code, everything else is a generic login
/// failure. The detail is for the server log only.
fn map_oidc_error(error: OidcError) -> AuthError {
    let code = if matches!(error, OidcError::Unavailable) {
        "oidc_unavailable"
    } else {
        "oidc_login_failed"
    };
    AuthError::Failed {
        code,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::{
        InMemoryAccountTokenRepository, InMemoryUserIdentityRepository, InMemoryUserRepository,
    };
    use crate::ports::OidcAuthRequest;
    use chrono::Duration;
    use domain::{
        AccountToken, AccountTokenId, AccountTokenKind, AccountTokenStatus, DEFAULT_NEW_USER_ROLE,
        Role, UserIdentity,
    };
    use std::sync::Mutex;

    /// A [`OidcProvider`] for tests: a fixed authorization URL and pending
    /// state; `complete_login` succeeds with `claims` unless the returned
    /// state differs from the pending one (StateMismatch) or `complete_error`
    /// is set.
    struct FakeOidcProvider {
        claims: OidcClaims,
        complete_error: Option<CompleteError>,
    }

    #[derive(Clone, Copy)]
    enum CompleteError {
        Unavailable,
        ExchangeFailed,
    }

    impl FakeOidcProvider {
        fn new(claims: OidcClaims) -> Self {
            Self {
                claims,
                complete_error: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl OidcProvider for FakeOidcProvider {
        fn display_name(&self) -> &str {
            "Fake IdP"
        }

        async fn authorization_request(&self) -> Result<OidcAuthRequest, OidcError> {
            Ok(OidcAuthRequest {
                authorization_url: "https://idp.example/authorize?client_id=minerva".to_owned(),
                pending: PendingOidcLogin {
                    csrf_state: "csrf-1".to_owned(),
                    nonce: "nonce-1".to_owned(),
                    pkce_verifier: "verifier-1".to_owned(),
                },
            })
        }

        async fn complete_login(
            &self,
            _code: String,
            returned_state: String,
            pending: PendingOidcLogin,
        ) -> Result<OidcClaims, OidcError> {
            if let Some(error) = self.complete_error {
                return Err(match error {
                    CompleteError::Unavailable => OidcError::Unavailable,
                    CompleteError::ExchangeFailed => {
                        OidcError::ExchangeFailed("the IdP refused the code".to_owned())
                    }
                });
            }
            if returned_state != pending.csrf_state {
                return Err(OidcError::StateMismatch);
            }
            Ok(self.claims.clone())
        }
    }

    /// Models a lost identity-creation race: lookups miss until an identity
    /// creation is attempted, creations always lose to a concurrent callback,
    /// and after the first attempt the "winner's" row is what lookups find.
    struct RacingIdentityRepository {
        winner: UserIdentity,
        create_attempted: Mutex<bool>,
    }

    impl RacingIdentityRepository {
        fn new(winner: UserIdentity) -> Self {
            Self {
                winner,
                create_attempted: Mutex::new(false),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserIdentityRepository for RacingIdentityRepository {
        async fn create(&self, _identity: UserIdentity) -> Result<UserIdentity, RepositoryError> {
            *self.create_attempted.lock().unwrap() = true;
            Err(RepositoryError::Conflict(
                "another callback won the race".to_owned(),
            ))
        }

        async fn find_by_issuer_and_subject(
            &self,
            issuer: String,
            subject: String,
        ) -> Result<Option<UserIdentity>, RepositoryError> {
            let attempted = *self.create_attempted.lock().unwrap();
            Ok(
                (attempted && self.winner.issuer == issuer && self.winner.subject == subject)
                    .then(|| self.winner.clone()),
            )
        }

        async fn list_for_user(
            &self,
            user_id: UserId,
        ) -> Result<Vec<UserIdentity>, RepositoryError> {
            Ok(if self.winner.user_id == user_id {
                vec![self.winner.clone()]
            } else {
                Vec::new()
            })
        }
    }

    fn claims(subject: &str, email: Option<&str>, verified: bool) -> OidcClaims {
        OidcClaims {
            issuer: "https://idp.example".to_owned(),
            subject: subject.to_owned(),
            email: email.map(str::to_owned),
            email_verified: verified,
            display_name: None,
            groups: Vec::new(),
        }
    }

    fn user(email: &str) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: email.to_owned(),
            password_hash: Some("hash-of-password".to_owned()),
            display_name: "Existing user".to_owned(),
            role: Role::Admin,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn identity_for(user_id: UserId, subject: &str) -> UserIdentity {
        UserIdentity::new(
            user_id,
            "https://idp.example".to_owned(),
            subject.to_owned(),
            None,
            Utc::now(),
        )
    }

    /// The pending state [`FakeOidcProvider::begin`] produces.
    fn pending() -> PendingLogin {
        let mut map = BTreeMap::new();
        map.insert(CSRF_STATE_KEY.to_owned(), "csrf-1".to_owned());
        map.insert(NONCE_KEY.to_owned(), "nonce-1".to_owned());
        map.insert(PKCE_VERIFIER_KEY.to_owned(), "verifier-1".to_owned());
        PendingLogin(map)
    }

    fn callback(code: &str, state: &str) -> CallbackParams {
        let mut params = BTreeMap::new();
        params.insert("code".to_owned(), code.to_owned());
        params.insert("state".to_owned(), state.to_owned());
        params
    }

    fn provider(
        oidc: FakeOidcProvider,
        users: Arc<InMemoryUserRepository>,
        identities: Arc<InMemoryUserIdentityRepository>,
        auto_create_users: bool,
    ) -> OidcAuthProvider {
        // The pre-invite tests do not care about tokens: an empty store.
        provider_with_invites(
            oidc,
            users.clone(),
            identities,
            Arc::new(InMemoryAccountTokenRepository::new(users)),
            auto_create_users,
        )
    }

    fn provider_with_invites(
        oidc: FakeOidcProvider,
        users: Arc<InMemoryUserRepository>,
        identities: Arc<InMemoryUserIdentityRepository>,
        invites: Arc<InMemoryAccountTokenRepository>,
        auto_create_users: bool,
    ) -> OidcAuthProvider {
        OidcAuthProvider::new(
            Arc::new(oidc),
            users,
            identities,
            invites,
            LoginPolicy { auto_create_users },
        )
    }

    /// A pending invite token for tests; seed it with [`InMemoryAccountTokenRepository::insert`].
    fn invite_token(email: &str, role: Role) -> AccountToken {
        let now = Utc::now();
        AccountToken {
            id: AccountTokenId::new(),
            kind: AccountTokenKind::Invite {
                email: email.to_owned(),
                role,
            },
            token_hash: format!("invite-hash-{email}"),
            created_by: None,
            created_at: now,
            expires_at: now + Duration::hours(48),
            consumed_at: None,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn begin_returns_the_authorization_url_and_pending_state() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let start = provider.begin().await.unwrap();
        assert_eq!(
            start.redirect_url,
            "https://idp.example/authorize?client_id=minerva"
        );
        assert_eq!(start.pending.0.get("csrf_state").unwrap(), "csrf-1");
        assert_eq!(start.pending.0.get("nonce").unwrap(), "nonce-1");
        assert_eq!(start.pending.0.get("pkce_verifier").unwrap(), "verifier-1");
    }

    #[tokio::test]
    async fn complete_creates_a_user_and_its_identity() {
        let users = Arc::new(InMemoryUserRepository::new());
        let identities = Arc::new(InMemoryUserIdentityRepository::new());
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("  Alice@Example.COM "), true)),
            users.clone(),
            identities.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.password_hash, None);
        assert_eq!(user.display_name, "alice");

        let stored_user = users.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(stored_user.id, user.id);
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), "sub-123".to_owned())
            .await
            .unwrap()
            .expect("identity stored");
        assert_eq!(identity.user_id, user.id);
    }

    #[tokio::test]
    async fn a_pending_invite_upgrades_the_created_users_role_and_is_consumed() {
        let users = Arc::new(InMemoryUserRepository::new());
        let identities = Arc::new(InMemoryUserIdentityRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let invite = invite_token("alice@example.com", Role::Staff);
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            identities,
            invites.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(
            user.role,
            Role::Staff,
            "the invite's role wins over the default"
        );

        // The invite is consumed: it shows as accepted in the invites list.
        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Accepted);
    }

    #[tokio::test]
    async fn without_an_invite_the_created_user_gets_the_default_role() {
        let users = Arc::new(InMemoryUserRepository::new());
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.role, DEFAULT_NEW_USER_ROLE);
    }

    #[tokio::test]
    async fn an_expired_invite_is_ignored() {
        let users = Arc::new(InMemoryUserRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let mut invite = invite_token("alice@example.com", Role::Staff);
        invite.expires_at = Utc::now() - Duration::hours(1);
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            invites.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(
            user.role, DEFAULT_NEW_USER_ROLE,
            "an expired invite grants nothing"
        );

        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Expired);
    }

    #[tokio::test]
    async fn a_revoked_invite_is_ignored() {
        let users = Arc::new(InMemoryUserRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let mut invite = invite_token("alice@example.com", Role::Staff);
        invite.revoked_at = Some(Utc::now());
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            invites.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(
            user.role, DEFAULT_NEW_USER_ROLE,
            "a revoked invite grants nothing"
        );

        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Revoked);
    }

    #[tokio::test]
    async fn a_pending_invite_does_not_enable_sso_signup_when_auto_create_is_off() {
        let users = Arc::new(InMemoryUserRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let invite = invite_token("alice@example.com", Role::Staff);
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            invites.clone(),
            false,
        );

        let error = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Rejected {
                    code: "oidc_signup_disabled"
                }
            ),
            "got {error:?}"
        );

        // The invite is untouched: it can still be accepted the normal way.
        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Pending);
    }

    #[tokio::test]
    async fn linking_to_an_existing_user_leaves_a_pending_invite_untouched() {
        let users = Arc::new(InMemoryUserRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        let existing = user("alice@example.com");
        users.create(existing.clone()).await.unwrap();
        let invite = invite_token("alice@example.com", Role::Staff);
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            invites.clone(),
            true,
        );

        let logged_in = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(logged_in.id, existing.id, "logs in as the existing user");

        // An existing account means the invite could not have been created
        // (or was superseded): it is left exactly as it was.
        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Pending);
    }

    #[tokio::test]
    async fn invite_email_matching_is_case_and_whitespace_insensitive() {
        let users = Arc::new(InMemoryUserRepository::new());
        let invites = Arc::new(InMemoryAccountTokenRepository::new(users.clone()));
        // The invite stored the normalized form; the IdP reports a messy one.
        let invite = invite_token("alice@example.com", Role::Staff);
        invites.insert(invite.clone());

        let provider = provider_with_invites(
            FakeOidcProvider::new(claims("sub-123", Some("  Alice@Example.COM "), true)),
            users,
            Arc::new(InMemoryUserIdentityRepository::new()),
            invites.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.role, Role::Staff);

        let stored = invites.find_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.status(Utc::now()), AccountTokenStatus::Accepted);
    }

    #[tokio::test]
    async fn complete_logs_in_a_known_identity_as_is_even_without_an_email() {
        let users = Arc::new(InMemoryUserRepository::new());
        let identities = Arc::new(InMemoryUserIdentityRepository::new());
        // No email on the user and none in the claims: a fresh login would be
        // rejected, but the known identity logs in as-is.
        let existing = User {
            id: UserId::new(),
            email: "someone@example.com".to_owned(),
            password_hash: None,
            display_name: "Known user".to_owned(),
            role: Role::Admin,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        users.create(existing.clone()).await.unwrap();
        identities
            .create(identity_for(existing.id, "sub-123"))
            .await
            .unwrap();

        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", None, false)),
            users.clone(),
            identities.clone(),
            false,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.id, existing.id);
        assert_eq!(users.list().await.unwrap().len(), 1, "no new user");
    }

    #[tokio::test]
    async fn complete_rejects_a_deactivated_user() {
        let users = Arc::new(InMemoryUserRepository::new());
        let identities = Arc::new(InMemoryUserIdentityRepository::new());
        let mut deactivated = user("off@example.com");
        deactivated.deactivated_at = Some(Utc::now());
        users.create(deactivated.clone()).await.unwrap();
        identities
            .create(identity_for(deactivated.id, "sub-123"))
            .await
            .unwrap();

        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("off@example.com"), true)),
            users,
            identities,
            false,
        );

        let error = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Rejected {
                    code: "deactivated"
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn complete_links_to_an_existing_user_by_verified_email() {
        let users = Arc::new(InMemoryUserRepository::new());
        let identities = Arc::new(InMemoryUserIdentityRepository::new());
        let existing = user("alice@example.com");
        users.create(existing.clone()).await.unwrap();

        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("alice@example.com"), true)),
            users.clone(),
            identities.clone(),
            true,
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.id, existing.id, "logs in as the existing user");
        assert_eq!(user.password_hash, Some("hash-of-password".to_owned()));
        let identity = identities
            .find_by_issuer_and_subject("https://idp.example".to_owned(), "sub-123".to_owned())
            .await
            .unwrap()
            .expect("identity linked");
        assert_eq!(identity.user_id, existing.id);
        assert_eq!(users.list().await.unwrap().len(), 1, "no new user");
    }

    #[tokio::test]
    async fn complete_rejects_an_unverified_email() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), false)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let error = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Rejected {
                    code: "oidc_email_not_verified"
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn complete_rejects_a_missing_email() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", None, true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let error = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Rejected {
                    code: "oidc_email_missing"
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn complete_rejects_an_unknown_user_when_signup_is_disabled() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            false,
        );

        let error = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Rejected {
                    code: "oidc_signup_disabled"
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_provider_error_parameter_fails_with_its_code() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let mut params = BTreeMap::new();
        params.insert("error".to_owned(), "access_denied".to_owned());
        params.insert(
            "error_description".to_owned(),
            "the user cancelled".to_owned(),
        );
        let error = provider.complete(params, pending()).await.unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Failed {
                    code: "oidc_provider_error",
                    ..
                }
            ),
            "got {error:?}"
        );
        assert!(error.to_string().contains("access_denied"));
        assert!(error.to_string().contains("the user cancelled"));

        // Without a description the failure still carries the error name.
        let mut params = BTreeMap::new();
        params.insert("error".to_owned(), "access_denied".to_owned());
        let error = provider.complete(params, pending()).await.unwrap_err();
        assert!(error.to_string().contains("no description"));
    }

    #[tokio::test]
    async fn a_callback_missing_code_or_state_fails() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        for params in [BTreeMap::new(), callback("abc", "")] {
            let error = provider.complete(params, pending()).await.unwrap_err();
            assert!(
                matches!(
                    &error,
                    AuthError::Failed {
                        code: "oidc_login_failed",
                        ..
                    }
                ),
                "got {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_mismatched_state_fails() {
        let provider = provider(
            FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true)),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );

        let error = provider
            .complete(callback("abc", "wrong-state"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Failed {
                    code: "oidc_login_failed",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_unavailable_provider_keeps_its_code() {
        // An unreachable IdP keeps its own code...
        let mut oidc = FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true));
        oidc.complete_error = Some(CompleteError::Unavailable);
        let auth_provider = provider(
            oidc,
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );
        let error = auth_provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Failed {
                    code: "oidc_unavailable",
                    ..
                }
            ),
            "got {error:?}"
        );

        // ...while any other protocol failure is a generic login failure.
        let mut oidc = FakeOidcProvider::new(claims("sub-123", Some("a@example.com"), true));
        oidc.complete_error = Some(CompleteError::ExchangeFailed);
        let auth_provider = provider(
            oidc,
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            true,
        );
        let error = auth_provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                AuthError::Failed {
                    code: "oidc_login_failed",
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_lost_identity_creation_race_re_looks_up_and_continues() {
        let users = Arc::new(InMemoryUserRepository::new());
        // The winner's row: what the re-lookup after our lost create finds.
        let winner_user_id = UserId::new();
        let identities = Arc::new(RacingIdentityRepository::new(identity_for(
            winner_user_id,
            "sub-123",
        )));

        // The first lookup misses (no row yet), so this takes the CreateUser
        // path; the identity create then loses the race and must carry on.
        let provider = OidcAuthProvider::new(
            Arc::new(FakeOidcProvider::new(claims(
                "sub-123",
                Some("a@example.com"),
                true,
            ))),
            users.clone(),
            identities,
            Arc::new(InMemoryAccountTokenRepository::new(users.clone())),
            LoginPolicy {
                auto_create_users: true,
            },
        );

        let user = provider
            .complete(callback("abc", "csrf-1"), pending())
            .await
            .unwrap();
        assert_eq!(user.email, "a@example.com");
        assert_eq!(users.list().await.unwrap().len(), 1);
    }

    #[test]
    fn the_provider_id_and_display_name_come_from_the_protocol_provider() {
        let provider = OidcAuthProvider::new(
            Arc::new(FakeOidcProvider::new(claims("sub-123", None, false))),
            Arc::new(InMemoryUserRepository::new()),
            Arc::new(InMemoryUserIdentityRepository::new()),
            Arc::new(InMemoryAccountTokenRepository::new(Arc::new(
                InMemoryUserRepository::new(),
            ))),
            LoginPolicy {
                auto_create_users: true,
            },
        );
        assert_eq!(provider.id(), "oidc");
        assert_eq!(provider.display_name(), "Fake IdP");
    }
}
