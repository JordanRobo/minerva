//! Persistence port for [`SsoGroupRule`]s: the admin-maintained SSO
//! group-to-role mapping (roadmap 2.7, D15).

use domain::{SsoGroupRule, SsoGroupRuleId};

use crate::ports::RepositoryError;

#[async_trait::async_trait]
pub trait SsoGroupRuleRepository: Send + Sync {
    /// All rules, in stable order: group name, then id.
    async fn list(&self) -> Result<Vec<SsoGroupRule>, RepositoryError>;

    /// Returns `None` if no rule has this id.
    async fn find_by_id(&self, id: SsoGroupRuleId)
    -> Result<Option<SsoGroupRule>, RepositoryError>;

    /// Insert a new rule. A rule for the same group name already existing is
    /// a `Conflict`.
    async fn create(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError>;

    /// Replace an existing rule's group name and role (and stamp
    /// `updated_at`). `NotFound` when no rule has this id; a group name taken
    /// by another rule is a `Conflict`.
    async fn update(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError>;

    /// Delete a rule. `NotFound` when no rule has this id.
    async fn delete(&self, id: SsoGroupRuleId) -> Result<(), RepositoryError>;

    /// Whether any rule exists: while true, SSO sign-ins recompute roles from
    /// the rules (D15); while false, roles are never touched.
    async fn any_exist(&self) -> Result<bool, RepositoryError>;
}
