//! User administration: the admin-only use cases behind `/api/users`.
//!
//! [`UserAdminService`] is the single place the access rules live: an admin
//! cannot change their own role or deactivate themselves, and the last
//! active admin can never be demoted or deactivated. The last-admin rule is
//! enforced by the repository inside one locked transaction (see
//! [`UserRepository::apply_access_change`]), not by a check-then-write here.

use std::sync::Arc;

use domain::{Role, User, UserId};

use crate::auth::SessionService;
use crate::ports::{
    AccessChange, AccessChangeError, RepositoryError, SsoGroupRuleRepository, UserRepository,
};

/// A failed user-administration operation.
#[derive(Debug)]
pub enum UserAdminError {
    /// No user has this id.
    NotFound,
    /// An admin cannot change their own role or deactivate themselves.
    CannotModifySelf,
    /// The change would leave the system without an active admin.
    LastAdmin,
    /// The target's role was recomputed from SSO groups (D15) and a group
    /// rule still exists, so hand edits are locked out.
    RoleManagedBySso,
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl std::fmt::Display for UserAdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserAdminError::NotFound => write!(f, "no such user"),
            UserAdminError::CannotModifySelf => {
                write!(f, "you cannot change your own role or deactivate yourself")
            }
            UserAdminError::LastAdmin => {
                write!(f, "there must always be at least one active administrator")
            }
            UserAdminError::RoleManagedBySso => write!(
                f,
                "This user's role is set by your single sign-on groups; change it in your identity provider"
            ),
            UserAdminError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for UserAdminError {}

/// Admin-only user management: listing accounts, changing roles and toggling
/// the active state.
pub struct UserAdminService {
    users: Arc<dyn UserRepository>,
    sso_rules: Arc<dyn SsoGroupRuleRepository>,
    sessions: SessionService,
}

impl UserAdminService {
    pub fn new(
        users: Arc<dyn UserRepository>,
        sso_rules: Arc<dyn SsoGroupRuleRepository>,
        sessions: SessionService,
    ) -> Self {
        Self {
            users,
            sso_rules,
            sessions,
        }
    }

    /// Every user, for the admin list view.
    pub async fn list(&self) -> Result<Vec<User>, UserAdminError> {
        self.users.list().await.map_err(UserAdminError::Repository)
    }

    /// Change `target`'s role. Re-applying the current role is a successful
    /// no-op; taking it from the last active admin is a [`UserAdminError::LastAdmin`];
    /// while any group rule exists, a role SSO recomputed (D15) is locked:
    /// [`UserAdminError::RoleManagedBySso`].
    pub async fn change_role(
        &self,
        actor: &User,
        target_id: UserId,
        role: Role,
    ) -> Result<User, UserAdminError> {
        // Check order is part of the API contract: NotFound, then the
        // self-edit lock, then the SSO-managed lock, and only then the
        // last-admin guard inside the repository's locked transaction. The
        // SSO check reads two committed facts (the flag, the rule count), so
        // a stale read can only delay the lock, never break it.
        let target = self
            .users
            .find_by_id(target_id)
            .await
            .map_err(UserAdminError::Repository)?
            .ok_or(UserAdminError::NotFound)?;
        if target_id == actor.id {
            return Err(UserAdminError::CannotModifySelf);
        }
        if target.role_managed_by_sso && self.sso_rules_exist().await? {
            return Err(UserAdminError::RoleManagedBySso);
        }
        self.apply(target_id, AccessChange::Role(role)).await
    }

    /// Whether any SSO group-to-role rule exists (D15): the condition that
    /// makes `role_managed_by_sso` roles locked against hand edits. Each HTTP
    /// request asks once and applies the answer to every user it reports.
    pub async fn sso_rules_exist(&self) -> Result<bool, UserAdminError> {
        self.sso_rules
            .any_exist()
            .await
            .map_err(UserAdminError::Repository)
    }

    /// Deactivate `target`: the role is kept, but the account can no longer
    /// sign in and every session of it is revoked. Re-deactivating is a
    /// successful no-op; deactivating the last active admin is a
    /// [`UserAdminError::LastAdmin`].
    pub async fn deactivate(
        &self,
        actor: &User,
        target_id: UserId,
    ) -> Result<User, UserAdminError> {
        if target_id == actor.id {
            return Err(UserAdminError::CannotModifySelf);
        }
        let user = self.apply(target_id, AccessChange::Deactivate).await?;
        // The database change is committed before the sessions go: a failure
        // here cannot leave the account active, and the session extractor
        // independently rejects deactivated users.
        self.sessions
            .revoke_all_for_user(target_id)
            .await
            .map_err(UserAdminError::Repository)?;
        Ok(user)
    }

    /// Reactivate a deactivated account. Re-activating an active account is a
    /// successful no-op.
    pub async fn reactivate(&self, target_id: UserId) -> Result<User, UserAdminError> {
        self.apply(target_id, AccessChange::Reactivate).await
    }

    async fn apply(&self, target_id: UserId, change: AccessChange) -> Result<User, UserAdminError> {
        self.users
            .apply_access_change(target_id, change)
            .await
            .map_err(|error| match error {
                AccessChangeError::NotFound => UserAdminError::NotFound,
                AccessChangeError::LastAdmin => UserAdminError::LastAdmin,
                AccessChangeError::Repository(error) => UserAdminError::Repository(error),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::SessionService;
    use crate::auth::fakes::{
        InMemorySessionRepository, InMemorySsoGroupRuleRepository, InMemoryUserRepository,
    };
    use crate::ports::SessionTokens;
    use chrono::Utc;
    use domain::{Role, SsoGroupRule, SsoGroupRuleId, UserId};

    /// A deterministic token generator for the session service.
    #[derive(Default)]
    struct Tokens;

    impl SessionTokens for Tokens {
        fn generate(&self) -> String {
            "token".to_owned()
        }
        fn hash(&self, token: &str) -> String {
            format!("hash-of-{token}")
        }
    }

    fn user(email: &str, role: Role) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: email.to_owned(),
            password_hash: None,
            display_name: "Test".into(),
            role,
            deactivated_at: None,
            role_managed_by_sso: false,
            sso_role_exempt: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// A user whose role SSO recomputed (D15): the flag that locks it
    /// against hand edits while any group rule exists.
    fn managed_user(email: &str, role: Role) -> User {
        let mut user = user(email, role);
        user.role_managed_by_sso = true;
        user
    }

    /// A group rule for the D15 lock tests, built like the API would.
    fn rule() -> SsoGroupRule {
        let now = Utc::now();
        SsoGroupRule {
            id: SsoGroupRuleId::new(),
            group_name: "teachers".to_owned(),
            role: Role::Admin,
            created_at: now,
            updated_at: now,
        }
    }

    /// An admin actor and an SSO-managed staff target over in-memory fakes;
    /// `with_rule` decides whether the group-rule store starts with a rule.
    async fn sso_managed(
        with_rule: bool,
    ) -> (
        UserAdminService,
        User,
        User,
        Arc<InMemorySsoGroupRuleRepository>,
        Arc<InMemoryUserRepository>,
    ) {
        let users = Arc::new(InMemoryUserRepository::new());
        let sessions = Arc::new(InMemorySessionRepository::new());
        let rules = Arc::new(InMemorySsoGroupRuleRepository::new());
        if with_rule {
            rules.create(rule()).await.unwrap();
        }
        let service = UserAdminService::new(
            users.clone(),
            rules.clone(),
            SessionService::new(
                sessions,
                Arc::new(Tokens),
                SessionService::DEFAULT_SESSION_TTL,
            ),
        );
        let admin = user("a@example.com", Role::Admin);
        let managed = managed_user("managed@example.com", Role::Staff);
        users.create(admin.clone()).await.unwrap();
        users.create(managed.clone()).await.unwrap();
        (service, admin, managed, rules, users)
    }

    /// A service over in-memory fakes with two admins and one staff member.
    async fn seeded() -> (UserAdminService, User, User, User) {
        let users = Arc::new(InMemoryUserRepository::new());
        let sessions = Arc::new(InMemorySessionRepository::new());
        let service = UserAdminService::new(
            users.clone(),
            Arc::new(InMemorySsoGroupRuleRepository::new()),
            SessionService::new(
                sessions,
                Arc::new(Tokens),
                SessionService::DEFAULT_SESSION_TTL,
            ),
        );
        let admin_a = user("a@example.com", Role::Admin);
        let admin_b = user("b@example.com", Role::Admin);
        let staff = user("staff@example.com", Role::Staff);
        users.create(admin_a.clone()).await.unwrap();
        users.create(admin_b.clone()).await.unwrap();
        users.create(staff.clone()).await.unwrap();
        (service, admin_a, admin_b, staff)
    }

    #[tokio::test]
    async fn an_admin_cannot_modify_themselves() {
        let (service, admin, _other, _staff) = seeded().await;
        assert!(matches!(
            service.change_role(&admin, admin.id, Role::Staff).await,
            Err(UserAdminError::CannotModifySelf)
        ));
        assert!(matches!(
            service.deactivate(&admin, admin.id).await,
            Err(UserAdminError::CannotModifySelf)
        ));
    }

    #[tokio::test]
    async fn the_last_active_admin_cannot_be_demoted_or_deactivated() {
        let (service, admin_a, admin_b, _staff) = seeded().await;
        // With two active admins, demoting one is fine...
        service
            .change_role(&admin_b, admin_a.id, Role::Staff)
            .await
            .unwrap();
        // ...but B is now the only active admin: both demotion and deactivation
        // of B are refused. (The actor's own role is enforced by the HTTP
        // layer, not here.)
        assert!(matches!(
            service
                .change_role(&admin_a, admin_b.id, Role::ReadOnly)
                .await,
            Err(UserAdminError::LastAdmin)
        ));
        assert!(matches!(
            service.deactivate(&admin_a, admin_b.id).await,
            Err(UserAdminError::LastAdmin)
        ));
    }

    #[tokio::test]
    async fn a_deactivated_admin_does_not_count_as_active() {
        let (service, admin_a, admin_b, _staff) = seeded().await;
        // Deactivate B: two admins exist, but A is now the only ACTIVE one.
        service.deactivate(&admin_a, admin_b.id).await.unwrap();
        // Demoting the deactivated admin is allowed — they do not count.
        service
            .change_role(&admin_a, admin_b.id, Role::Staff)
            .await
            .unwrap();
        // But A is still the last active admin: demoting or deactivating A is refused.
        assert!(matches!(
            service.change_role(&admin_b, admin_a.id, Role::Staff).await,
            Err(UserAdminError::LastAdmin)
        ));
        assert!(matches!(
            service.deactivate(&admin_b, admin_a.id).await,
            Err(UserAdminError::LastAdmin)
        ));
    }

    #[tokio::test]
    async fn reapplying_the_current_state_is_a_no_op() {
        let (service, admin_a, admin_b, staff) = seeded().await;
        // Same role.
        let same = service
            .change_role(&admin_a, admin_b.id, Role::Admin)
            .await
            .unwrap();
        assert_eq!(same.role, Role::Admin);
        // Already deactivated / already active.
        service.deactivate(&admin_a, staff.id).await.unwrap();
        let still_off = service.deactivate(&admin_a, staff.id).await.unwrap();
        assert!(!still_off.is_active());
        let back_on = service.reactivate(staff.id).await.unwrap();
        assert!(back_on.is_active());
        let still_on = service.reactivate(staff.id).await.unwrap();
        assert!(still_on.is_active());
    }

    #[tokio::test]
    async fn deactivating_revokes_every_session_of_the_user() {
        let users = Arc::new(InMemoryUserRepository::new());
        let sessions = Arc::new(InMemorySessionRepository::new());
        let admin = user("a@example.com", Role::Admin);
        let staff = user("staff@example.com", Role::Staff);
        users.create(admin.clone()).await.unwrap();
        users.create(staff.clone()).await.unwrap();
        let service = UserAdminService::new(
            users,
            Arc::new(InMemorySsoGroupRuleRepository::new()),
            SessionService::new(
                sessions.clone(),
                Arc::new(Tokens),
                SessionService::DEFAULT_SESSION_TTL,
            ),
        );
        // A session issued before the deactivation...
        let issuer = SessionService::new(
            sessions,
            Arc::new(Tokens),
            SessionService::DEFAULT_SESSION_TTL,
        );
        let issued = issuer.issue(staff.id).await.unwrap();

        service.deactivate(&admin, staff.id).await.unwrap();
        assert!(
            issuer.resolve(&issued.token).await.unwrap().is_none(),
            "the old session must be gone"
        );
    }

    #[tokio::test]
    async fn an_unknown_user_is_not_found() {
        let (service, admin, _b, _staff) = seeded().await;
        assert!(matches!(
            service
                .change_role(&admin, UserId::new(), Role::Staff)
                .await,
            Err(UserAdminError::NotFound)
        ));
        assert!(matches!(
            service.deactivate(&admin, UserId::new()).await,
            Err(UserAdminError::NotFound)
        ));
        assert!(matches!(
            service.reactivate(UserId::new()).await,
            Err(UserAdminError::NotFound)
        ));
    }

    #[tokio::test]
    async fn an_sso_managed_role_is_locked_while_any_rule_exists() {
        let (service, admin, managed, _rules, _users) = sso_managed(true).await;
        assert!(matches!(
            service.change_role(&admin, managed.id, Role::Admin).await,
            Err(UserAdminError::RoleManagedBySso)
        ));
    }

    #[tokio::test]
    async fn an_sso_managed_role_unlocks_when_the_rules_are_deleted() {
        let (service, admin, managed, rules, _users) = sso_managed(true).await;
        // Deleting the last rule frees the role for hand edits again.
        let listed = rules.list().await.unwrap();
        let [rule] = listed.as_slice() else {
            panic!("expected exactly one rule");
        };
        rules.delete(rule.id).await.unwrap();
        let changed = service
            .change_role(&admin, managed.id, Role::Admin)
            .await
            .unwrap();
        assert_eq!(changed.role, Role::Admin);
    }

    #[tokio::test]
    async fn an_unmanaged_role_changes_fine_while_rules_exist() {
        let (service, admin, _managed, _rules, users) = sso_managed(true).await;
        // The lock follows the flag: a user SSO never recomputed is editable.
        let plain = user("plain@example.com", Role::Staff);
        users.create(plain.clone()).await.unwrap();
        let changed = service
            .change_role(&admin, plain.id, Role::Admin)
            .await
            .unwrap();
        assert_eq!(changed.role, Role::Admin);
    }

    #[tokio::test]
    async fn deactivating_an_sso_managed_user_is_unaffected_by_the_lock() {
        let (service, admin, managed, _rules, _users) = sso_managed(true).await;
        // The lock covers role changes only: deactivation and reactivation
        // still go through while a rule exists.
        let off = service.deactivate(&admin, managed.id).await.unwrap();
        assert!(!off.is_active());
        let back_on = service.reactivate(managed.id).await.unwrap();
        assert!(back_on.is_active());
    }

    #[tokio::test]
    async fn not_found_and_self_edit_precede_the_sso_lock() {
        let (service, admin, _managed, _rules, users) = sso_managed(true).await;
        // Unknown id: NotFound, even with a rule present.
        assert!(matches!(
            service
                .change_role(&admin, UserId::new(), Role::Staff)
                .await,
            Err(UserAdminError::NotFound)
        ));
        // The self-edit lock beats the SSO lock: an admin who is themselves
        // SSO-managed still gets CannotModifySelf.
        let self_managed = managed_user("self@example.com", Role::Admin);
        users.create(self_managed.clone()).await.unwrap();
        assert!(matches!(
            service
                .change_role(&self_managed, self_managed.id, Role::Staff)
                .await,
            Err(UserAdminError::CannotModifySelf)
        ));
    }

    #[tokio::test]
    async fn the_sso_lock_precedes_the_last_admin_guard() {
        // One active admin, SSO-managed, with a rule present: demoting them
        // would both break the lock and strand the system without an admin —
        // the lock reports first. (The actor's role is enforced by the HTTP
        // layer, not here.)
        let users = Arc::new(InMemoryUserRepository::new());
        let sessions = Arc::new(InMemorySessionRepository::new());
        let rules = Arc::new(InMemorySsoGroupRuleRepository::new());
        rules.create(rule()).await.unwrap();
        let service = UserAdminService::new(
            users.clone(),
            rules,
            SessionService::new(
                sessions,
                Arc::new(Tokens),
                SessionService::DEFAULT_SESSION_TTL,
            ),
        );
        let last_admin = managed_user("last@example.com", Role::Admin);
        let actor = user("actor@example.com", Role::Staff);
        users.create(last_admin.clone()).await.unwrap();
        users.create(actor.clone()).await.unwrap();
        assert!(matches!(
            service
                .change_role(&actor, last_admin.id, Role::Staff)
                .await,
            Err(UserAdminError::RoleManagedBySso)
        ));
    }
}
