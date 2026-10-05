//! SSO group-rule management (roadmap 2.7, D15): the admin-only use cases
//! behind `/api/sso/group-rules`. The rules themselves — and the role they
//! resolve to at sign-in — live in [`domain::sso_group_rule`]; this service
//! validates input and turns repository failures into typed errors.

use std::sync::Arc;

use chrono::Utc;
use domain::{Role, SsoGroupRule, SsoGroupRuleId};

use crate::ports::{RepositoryError, SsoGroupRuleRepository};

/// The longest group name a client may send. A policy limit (the column is
/// unbounded `text`), kept in one place for the validation and its message.
const MAX_GROUP_NAME_LEN: usize = 255;

/// A failed group-rule operation.
#[derive(Debug)]
pub enum SsoGroupRuleError {
    /// The group name is empty or longer than 255 characters.
    InvalidGroupName,
    /// A rule for this group name already exists.
    GroupRuleExists,
    /// No rule has this id.
    NotFound,
    /// Something unexpected went wrong in storage.
    Repository(RepositoryError),
}

impl std::fmt::Display for SsoGroupRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SsoGroupRuleError::InvalidGroupName => {
                write!(f, "group_name must be 1 to {MAX_GROUP_NAME_LEN} characters")
            }
            SsoGroupRuleError::GroupRuleExists => {
                write!(f, "a rule for this group already exists")
            }
            SsoGroupRuleError::NotFound => write!(f, "no such group rule"),
            SsoGroupRuleError::Repository(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SsoGroupRuleError {}

/// Admin-only management of the SSO group-to-role rules.
pub struct SsoGroupRuleService {
    rules: Arc<dyn SsoGroupRuleRepository>,
}

impl SsoGroupRuleService {
    pub fn new(rules: Arc<dyn SsoGroupRuleRepository>) -> Self {
        Self { rules }
    }

    /// Every rule, in the repository's stable order.
    pub async fn list(&self) -> Result<Vec<SsoGroupRule>, SsoGroupRuleError> {
        self.rules
            .list()
            .await
            .map_err(SsoGroupRuleError::Repository)
    }

    /// Add a rule for `group_name`. Surrounding whitespace is trimmed, so the
    /// stored name never has any.
    pub async fn create(
        &self,
        group_name: String,
        role: Role,
    ) -> Result<SsoGroupRule, SsoGroupRuleError> {
        let now = Utc::now();
        let rule = SsoGroupRule {
            id: SsoGroupRuleId::new(),
            group_name: Self::validate_group_name(&group_name)?,
            role,
            created_at: now,
            updated_at: now,
        };
        self.rules.create(rule).await.map_err(Self::map_error)
    }

    /// Change a rule's group name and/or role. The id and `created_at` are
    /// kept; `updated_at` moves to now.
    pub async fn update(
        &self,
        id: SsoGroupRuleId,
        group_name: String,
        role: Role,
    ) -> Result<SsoGroupRule, SsoGroupRuleError> {
        let existing = self
            .rules
            .find_by_id(id)
            .await
            .map_err(Self::map_error)?
            .ok_or(SsoGroupRuleError::NotFound)?;
        let rule = SsoGroupRule {
            group_name: Self::validate_group_name(&group_name)?,
            role,
            updated_at: Utc::now(),
            ..existing
        };
        self.rules.update(rule).await.map_err(Self::map_error)
    }

    /// Remove a rule. Deleting the last one is what switches SSO sign-ins off
    /// role recomputation (D15); that is the caller's decision to make.
    pub async fn delete(&self, id: SsoGroupRuleId) -> Result<(), SsoGroupRuleError> {
        self.rules.delete(id).await.map_err(Self::map_error)
    }

    fn validate_group_name(group_name: &str) -> Result<String, SsoGroupRuleError> {
        let trimmed = group_name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_GROUP_NAME_LEN {
            return Err(SsoGroupRuleError::InvalidGroupName);
        }
        Ok(trimmed.to_owned())
    }

    fn map_error(error: RepositoryError) -> SsoGroupRuleError {
        match error {
            // The unique index on group_name is the duplicate guard; surface
            // it as a typed conflict rather than a raw storage error.
            RepositoryError::Conflict(_) => SsoGroupRuleError::GroupRuleExists,
            RepositoryError::NotFound => SsoGroupRuleError::NotFound,
            other => SsoGroupRuleError::Repository(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::fakes::InMemorySsoGroupRuleRepository;

    fn service() -> SsoGroupRuleService {
        SsoGroupRuleService::new(Arc::new(InMemorySsoGroupRuleRepository::new()))
    }

    #[tokio::test]
    async fn create_trims_the_name_and_lists_it_back() {
        let service = service();
        let rule = service
            .create("  teachers ".to_owned(), Role::Staff)
            .await
            .unwrap();
        assert_eq!(rule.group_name, "teachers");
        let listed = service.list().await.unwrap();
        assert_eq!(listed, vec![rule]);
    }

    #[tokio::test]
    async fn an_empty_or_blank_name_is_rejected() {
        let service = service();
        for name in ["", "   "] {
            assert!(
                matches!(
                    service.create(name.to_owned(), Role::Staff).await,
                    Err(SsoGroupRuleError::InvalidGroupName)
                ),
                "{name:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_name_longer_than_255_chars_is_rejected() {
        let service = service();
        assert!(matches!(
            service.create("a".repeat(256), Role::Staff).await,
            Err(SsoGroupRuleError::InvalidGroupName)
        ));
        // ...and 255 itself is fine.
        service.create("a".repeat(255), Role::Staff).await.unwrap();
    }

    #[tokio::test]
    async fn a_duplicate_name_is_a_conflict_even_across_whitespace() {
        let service = service();
        service
            .create("teachers".to_owned(), Role::Staff)
            .await
            .unwrap();
        // The service trims before the repository sees it, so " teachers "
        // is the same name.
        assert!(matches!(
            service.create(" teachers ".to_owned(), Role::Admin).await,
            Err(SsoGroupRuleError::GroupRuleExists)
        ));
    }

    #[tokio::test]
    async fn update_changes_name_and_role_and_keeps_created_at() {
        let service = service();
        let rule = service
            .create("teachers".to_owned(), Role::Staff)
            .await
            .unwrap();
        let updated = service
            .update(rule.id, "  staff ".to_owned(), Role::Admin)
            .await
            .unwrap();
        assert_eq!(updated.group_name, "staff");
        assert_eq!(updated.role, Role::Admin);
        assert_eq!(updated.id, rule.id);
        assert_eq!(updated.created_at, rule.created_at);
    }

    #[tokio::test]
    async fn update_rejects_an_unknown_id_and_bad_names() {
        let service = service();
        assert!(matches!(
            service
                .update(SsoGroupRuleId::new(), "x".to_owned(), Role::Staff)
                .await,
            Err(SsoGroupRuleError::NotFound)
        ));
        let rule = service
            .create("teachers".to_owned(), Role::Staff)
            .await
            .unwrap();
        assert!(matches!(
            service.update(rule.id, "".to_owned(), Role::Staff).await,
            Err(SsoGroupRuleError::InvalidGroupName)
        ));
    }

    #[tokio::test]
    async fn renaming_onto_a_taken_name_is_a_conflict() {
        let service = service();
        let first = service
            .create("teachers".to_owned(), Role::Staff)
            .await
            .unwrap();
        let second = service
            .create("students".to_owned(), Role::ReadOnly)
            .await
            .unwrap();
        assert!(matches!(
            service
                .update(second.id, "teachers".to_owned(), Role::Admin)
                .await,
            Err(SsoGroupRuleError::GroupRuleExists)
        ));
        // Neither rule changed: both are still there, in list order.
        let listed = service.list().await.unwrap();
        assert_eq!(listed, vec![second.clone(), first.clone()]);
    }

    #[tokio::test]
    async fn delete_removes_the_rule_and_not_found_when_gone() {
        let service = service();
        let rule = service
            .create("teachers".to_owned(), Role::Staff)
            .await
            .unwrap();
        service.delete(rule.id).await.unwrap();
        assert!(service.list().await.unwrap().is_empty());
        assert!(matches!(
            service.delete(rule.id).await,
            Err(SsoGroupRuleError::NotFound)
        ));
    }
}
