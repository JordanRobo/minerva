//! Pure decision logic for OIDC sign-in: given what the IdP asserted, who is
//! logging in?
//!
//! No I/O and no async — the use case that calls these functions fetches the
//! relevant rows (the identity for the claimed issuer/subject pair, the user
//! with the claimed email) and hands them in as options. Keeping the rules
//! here, out of any handler or repository, makes them trivially testable and
//! leaves room for group-to-role rules later without touching callers.

use chrono::{DateTime, Utc};
use domain::{DEFAULT_NEW_USER_ROLE, User, UserId, UserIdentity};

use crate::ports::OidcClaims;

/// Policy knobs for OIDC sign-in decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginPolicy {
    /// When no user exists for the claimed email, create one instead of
    /// rejecting the login.
    pub auto_create_users: bool,
}

/// Why an OIDC login was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginRejection {
    /// The ID token carried no usable email claim.
    EmailMissing,
    /// The provider has not verified the claimed email.
    EmailNotVerified,
    /// No user exists for the claimed email and auto-creation is disabled.
    SignupDisabled,
}

impl LoginRejection {
    /// Stable snake_case code for API responses and logs.
    pub fn code(&self) -> &'static str {
        match self {
            LoginRejection::EmailMissing => "oidc_email_missing",
            LoginRejection::EmailNotVerified => "oidc_email_not_verified",
            LoginRejection::SignupDisabled => "oidc_signup_disabled",
        }
    }
}

/// What to do about an OIDC login attempt.
///
/// Callers outside the application crate should keep a wildcard match arm:
/// more decisions may be added as group-to-role rules land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginDecision {
    /// The (issuer, subject) pair is already linked; log that user in as-is.
    ExistingIdentity { user_id: UserId },
    /// No link yet, but a user with the claimed email exists; link the
    /// identity to it and log them in.
    LinkToExistingUser { user_id: UserId },
    /// No link and no user; create one from the claims.
    CreateUser,
    /// The login must not proceed.
    Reject(LoginRejection),
}

/// Decide what to do about an OIDC login attempt.
///
/// `existing_identity` is the identity row for the claims' (issuer, subject)
/// pair, if any; `user_with_claimed_email` is the user with the claimed email
/// (trimmed and lowercased), if any. The rules, in order:
///
/// 1. A known identity logs its user in as-is — no email checks at all, so an
///    email change at the IdP cannot lock a user out.
/// 2. No (non-blank) email claim: reject.
/// 3. Unverified email: reject — including for new users, so nobody can squat
///    an address the provider has not verified.
/// 4. A user with that email exists: link the identity to it.
/// 5. Otherwise create a user if the policy allows, else reject.
pub fn decide_login(
    policy: LoginPolicy,
    claims: &OidcClaims,
    existing_identity: Option<&UserIdentity>,
    user_with_claimed_email: Option<&User>,
) -> LoginDecision {
    if let Some(identity) = existing_identity {
        return LoginDecision::ExistingIdentity {
            user_id: identity.user_id,
        };
    }

    let has_email = claims
        .email
        .as_deref()
        .map(str::trim)
        .is_some_and(|email| !email.is_empty());
    if !has_email {
        return LoginDecision::Reject(LoginRejection::EmailMissing);
    }

    if !claims.email_verified {
        return LoginDecision::Reject(LoginRejection::EmailNotVerified);
    }

    if let Some(user) = user_with_claimed_email {
        return LoginDecision::LinkToExistingUser { user_id: user.id };
    }

    if policy.auto_create_users {
        LoginDecision::CreateUser
    } else {
        LoginDecision::Reject(LoginRejection::SignupDisabled)
    }
}

/// Build a new passwordless [`User`] from OIDC claims.
///
/// This is the single place where new SSO users are constructed, so changing
/// the role they start with later means editing one line.
pub fn new_user_from_claims(claims: &OidcClaims, now: DateTime<Utc>) -> User {
    // `decide_login` guarantees a verified, non-blank email before this runs.
    let email = claims
        .email
        .as_deref()
        .expect("a user from OIDC claims requires an email claim")
        .trim()
        .to_lowercase();

    // Fall back to the email's local part when the provider gave no usable name.
    let display_name = claims
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| email.split('@').next().map(str::to_owned))
        .expect("an email always has a local part");

    User {
        id: UserId::new(),
        email,
        password_hash: None,
        display_name,
        role: DEFAULT_NEW_USER_ROLE,
        created_at: now,
        updated_at: now,
    }
}

/// Build the [`UserIdentity`] row that links `user_id` to the claims'
/// provider identity. The claimed email is stored as the provider reported
/// it; the canonical (trimmed, lowercased) form lives on the user row.
pub fn identity_from_claims(
    user_id: UserId,
    claims: &OidcClaims,
    now: DateTime<Utc>,
) -> UserIdentity {
    UserIdentity::new(
        user_id,
        claims.issuer.clone(),
        claims.subject.clone(),
        claims.email.clone(),
        now,
    )
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use domain::Role;

    use super::*;

    fn test_now() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 9, 30)
            .expect("valid date")
            .and_hms_opt(12, 0, 0)
            .expect("valid time")
            .and_utc()
    }

    fn claims(email: Option<&str>, verified: bool) -> OidcClaims {
        OidcClaims {
            issuer: "https://idp.example".into(),
            subject: "sub-123".into(),
            email: email.map(str::to_owned),
            email_verified: verified,
            display_name: None,
            groups: Vec::new(),
        }
    }

    fn user(id: UserId, email: &str) -> User {
        let now = test_now();
        User {
            id,
            email: email.into(),
            password_hash: Some("hash".into()),
            display_name: "Existing user".into(),
            role: Role::Admin,
            created_at: now,
            updated_at: now,
        }
    }

    fn identity(user_id: UserId) -> UserIdentity {
        UserIdentity::new(
            user_id,
            "https://idp.example".into(),
            "sub-123".into(),
            None,
            test_now(),
        )
    }

    const AUTO_CREATE: LoginPolicy = LoginPolicy {
        auto_create_users: true,
    };
    const NO_AUTO_CREATE: LoginPolicy = LoginPolicy {
        auto_create_users: false,
    };

    #[test]
    fn known_identity_wins_even_when_email_is_missing_or_unverified() {
        let user_id = UserId::new();
        let existing = identity(user_id);

        // No email at all, and unverified — a fresh login would be rejected,
        // but the known identity logs in as-is.
        assert_eq!(
            decide_login(AUTO_CREATE, &claims(None, false), Some(&existing), None),
            LoginDecision::ExistingIdentity { user_id }
        );
        assert_eq!(
            decide_login(
                NO_AUTO_CREATE,
                &claims(Some("unverified@example.com"), false),
                Some(&existing),
                None
            ),
            LoginDecision::ExistingIdentity { user_id }
        );

        // Even a user row with a *different* email does not divert the login.
        let other = user(UserId::new(), "someone-else@example.com");
        assert_eq!(
            decide_login(
                AUTO_CREATE,
                &claims(None, false),
                Some(&existing),
                Some(&other)
            ),
            LoginDecision::ExistingIdentity { user_id }
        );
    }

    #[test]
    fn missing_email_is_rejected() {
        assert_eq!(
            decide_login(AUTO_CREATE, &claims(None, true), None, None),
            LoginDecision::Reject(LoginRejection::EmailMissing)
        );
    }

    #[test]
    fn blank_email_is_rejected_as_missing() {
        for blank in ["", "   ", "\t"] {
            assert_eq!(
                decide_login(AUTO_CREATE, &claims(Some(blank), true), None, None),
                LoginDecision::Reject(LoginRejection::EmailMissing),
                "blank email {blank:?} should count as missing"
            );
        }
    }

    #[test]
    fn unverified_email_is_rejected_with_and_without_existing_user() {
        assert_eq!(
            decide_login(
                AUTO_CREATE,
                &claims(Some("a@example.com"), false),
                None,
                None
            ),
            LoginDecision::Reject(LoginRejection::EmailNotVerified)
        );

        // The email-verification check comes before the link-to-existing-user
        // rule: an unverified claim must not link to (or create for) anyone.
        let existing = user(UserId::new(), "a@example.com");
        assert_eq!(
            decide_login(
                AUTO_CREATE,
                &claims(Some("a@example.com"), false),
                None,
                Some(&existing)
            ),
            LoginDecision::Reject(LoginRejection::EmailNotVerified)
        );
    }

    #[test]
    fn verified_email_with_existing_user_links_to_it() {
        let existing = user(UserId::new(), "a@example.com");
        assert_eq!(
            decide_login(
                AUTO_CREATE,
                &claims(Some("a@example.com"), true),
                None,
                Some(&existing)
            ),
            LoginDecision::LinkToExistingUser {
                user_id: existing.id
            }
        );
    }

    #[test]
    fn verified_email_without_user_depends_on_auto_create() {
        assert_eq!(
            decide_login(
                AUTO_CREATE,
                &claims(Some("a@example.com"), true),
                None,
                None
            ),
            LoginDecision::CreateUser
        );
        assert_eq!(
            decide_login(
                NO_AUTO_CREATE,
                &claims(Some("a@example.com"), true),
                None,
                None
            ),
            LoginDecision::Reject(LoginRejection::SignupDisabled)
        );
    }

    #[test]
    fn rejection_codes_are_stable_snake_case() {
        assert_eq!(LoginRejection::EmailMissing.code(), "oidc_email_missing");
        assert_eq!(
            LoginRejection::EmailNotVerified.code(),
            "oidc_email_not_verified"
        );
        assert_eq!(
            LoginRejection::SignupDisabled.code(),
            "oidc_signup_disabled"
        );
    }

    #[test]
    fn new_user_from_claims_normalizes_email_and_falls_back_for_name() {
        let now = test_now();
        let claims = OidcClaims {
            issuer: "https://idp.example".into(),
            subject: "sub-123".into(),
            email: Some("  Alice@Example.COM ".into()),
            email_verified: true,
            display_name: None,
            groups: Vec::new(),
        };

        let new_user = new_user_from_claims(&claims, now);
        assert_eq!(new_user.email, "alice@example.com");
        assert_eq!(new_user.display_name, "alice");
        assert_eq!(new_user.password_hash, None);
        // SSO-created accounts start with the default role.
        assert_eq!(new_user.role, DEFAULT_NEW_USER_ROLE);
        assert_eq!(new_user.created_at, now);
        assert_eq!(new_user.updated_at, now);
    }

    #[test]
    fn new_user_from_claims_prefers_the_provider_display_name() {
        let claims = OidcClaims {
            display_name: Some("  Dr. Alice ".into()),
            ..claims(Some("alice@example.com"), true)
        };

        let new_user = new_user_from_claims(&claims, test_now());
        assert_eq!(new_user.display_name, "Dr. Alice");
    }

    #[test]
    fn identity_from_claims_links_the_user_to_the_provider_identity() {
        let user_id = UserId::new();
        let now = test_now();
        let claims = OidcClaims {
            issuer: "https://idp.example".into(),
            subject: "sub-123".into(),
            email: Some("alice@example.com".into()),
            email_verified: true,
            display_name: None,
            groups: Vec::new(),
        };

        let identity = identity_from_claims(user_id, &claims, now);
        assert_eq!(identity.user_id, user_id);
        assert_eq!(identity.issuer, "https://idp.example");
        assert_eq!(identity.subject, "sub-123");
        assert_eq!(identity.email, Some("alice@example.com".into()));
        assert_eq!(identity.created_at, now);
    }
}
