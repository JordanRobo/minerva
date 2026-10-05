//! SSO group-to-role mapping at login (roadmap 2.7, D15).
//!
//! While at least one group rule exists, the role of every user signing in
//! via SSO is recomputed from the IdP's groups; while none exists, roles and
//! flags are never touched. The rules themselves are admin-managed behind
//! [`SsoGroupRuleRepository`]; this service only applies them to the user an
//! `OidcAuthProvider` has resolved or created, after the deactivated-account
//! rejection.

use std::sync::Arc;

use domain::{User, resolve_role};

use crate::ports::{
    AccessChange, AccessChangeError, RepositoryError, SsoGroupRuleRepository, UserRepository,
};

/// Applies the D15 group-to-role rules to a user at SSO login.
pub struct SsoRoleService {
    rules: Arc<dyn SsoGroupRuleRepository>,
    users: Arc<dyn UserRepository>,
}

impl SsoRoleService {
    pub fn new(rules: Arc<dyn SsoGroupRuleRepository>, users: Arc<dyn UserRepository>) -> Self {
        Self { rules, users }
    }

    /// The account an SSO login creates (D15): while any rule exists, the
    /// role is computed from `groups` and marked SSO-managed — a pending
    /// invite's role, if one was applied first, is overridden; without rules
    /// the user comes back exactly as given (the invite's role or the
    /// default).
    pub async fn for_new_user(
        &self,
        user: User,
        groups: &[String],
    ) -> Result<User, RepositoryError> {
        let rules = self.rules.list().await?;
        if rules.is_empty() {
            return Ok(user);
        }
        Ok(User {
            role: resolve_role(&rules, groups),
            role_managed_by_sso: true,
            ..user
        })
    }

    /// Re-resolve an existing user's role from `groups` (D15). Returns the
    /// user to log in as: unchanged when nothing must change (exempt, no
    /// rules, already up to date) or when applying would demote the last
    /// active administrator (a warning is logged); otherwise the row as
    /// stored after the locked write.
    pub async fn recompute(&self, user: User, groups: &[String]) -> Result<User, RepositoryError> {
        if user.sso_role_exempt {
            return Ok(user);
        }
        let rules = self.rules.list().await?;
        if rules.is_empty() {
            return Ok(user);
        }
        let computed = resolve_role(&rules, groups);
        if user.role == computed && user.role_managed_by_sso {
            return Ok(user);
        }
        match self
            .users
            .apply_access_change(user.id, AccessChange::RoleManagedBySso(computed))
            .await
        {
            Ok(updated) => Ok(updated),
            // The backstop: the login still succeeds with the old role; an
            // admin has to fix the groups or add another administrator.
            Err(AccessChangeError::LastAdmin) => {
                eprintln!(
                    "warning: SSO login for {} would demote the last active administrator \
                     to {}; no change was made",
                    user.email, computed
                );
                Ok(user)
            }
            Err(AccessChangeError::NotFound) => Err(RepositoryError::NotFound),
            Err(AccessChangeError::Repository(error)) => Err(error),
        }
    }
}
