//! SSO group-to-role rules (roadmap 2.7, D15): the admin-maintained mapping
//! from IdP group names to roles. While at least one rule exists, every SSO
//! sign-in recomputes the user's role from the groups claim; with no rules,
//! roles are never touched.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::role::{Role, SSO_FALLBACK_ROLE};

/// Identifier for an [`SsoGroupRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SsoGroupRuleId(pub Uuid);

impl SsoGroupRuleId {
    /// Create a fresh identifier for a rule that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SsoGroupRuleId {
    fn default() -> Self {
        Self::new()
    }
}

/// A mapping from one IdP group name to a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsoGroupRule {
    pub id: SsoGroupRuleId,
    /// The group name as the IdP sends it. Matching trims surrounding
    /// whitespace but is otherwise exact and case-sensitive (D15).
    pub group_name: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The role for an SSO sign-in given the current rules and the IdP's groups
/// claim (D15): the least permissive role among the rules whose name matches
/// one of `groups` exactly after trimming whitespace, or
/// [`SSO_FALLBACK_ROLE`] when nothing matches.
pub fn resolve_role(rules: &[SsoGroupRule], groups: &[String]) -> Role {
    // The least permissive matched role wins (D15); no match falls back.
    rules
        .iter()
        .filter(|rule| {
            let name = rule.group_name.trim();
            groups.iter().any(|group| group.trim() == name)
        })
        .map(|rule| rule.role)
        .min_by_key(|role| role.rank())
        .unwrap_or(SSO_FALLBACK_ROLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(group_name: &str, role: Role) -> SsoGroupRule {
        let now = Utc::now();
        SsoGroupRule {
            id: SsoGroupRuleId::new(),
            group_name: group_name.to_owned(),
            role,
            created_at: now,
            updated_at: now,
        }
    }

    fn groups(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn no_rules_give_the_fallback_role() {
        assert_eq!(resolve_role(&[], &groups(&["teachers"])), SSO_FALLBACK_ROLE);
    }

    #[test]
    fn no_matching_rule_gives_the_fallback_role() {
        let rules = vec![rule("teachers", Role::Staff)];
        assert_eq!(
            resolve_role(&rules, &groups(&["students"])),
            SSO_FALLBACK_ROLE
        );
        // An absent groups claim is the same as a non-matching one.
        assert_eq!(resolve_role(&rules, &[]), SSO_FALLBACK_ROLE);
    }

    #[test]
    fn a_single_match_gives_that_rules_role() {
        let rules = vec![rule("teachers", Role::Staff), rule("admins", Role::Admin)];
        assert_eq!(resolve_role(&rules, &groups(&["teachers"])), Role::Staff);
    }

    #[test]
    fn several_matches_choose_the_least_permissive_role() {
        let rules = vec![
            rule("all-staff", Role::Admin),
            rule("teachers", Role::ReadOnly),
        ];
        assert_eq!(
            resolve_role(&rules, &groups(&["all-staff", "teachers"])),
            Role::ReadOnly
        );
        // ...no matter the order of the rules.
        let rules = vec![
            rule("teachers", Role::ReadOnly),
            rule("all-staff", Role::Admin),
        ];
        assert_eq!(
            resolve_role(&rules, &groups(&["all-staff", "teachers"])),
            Role::ReadOnly
        );
    }

    #[test]
    fn matching_trims_surrounding_whitespace() {
        let rules = vec![rule("  teachers  ", Role::Staff)];
        assert_eq!(resolve_role(&rules, &groups(&["teachers"])), Role::Staff);
        // Whitespace on the claim side is trimmed too.
        let rules = vec![rule("teachers", Role::Staff)];
        assert_eq!(resolve_role(&rules, &groups(&["  teachers "])), Role::Staff);
    }

    #[test]
    fn matching_is_case_sensitive() {
        let rules = vec![rule("Teachers", Role::Staff)];
        assert_eq!(
            resolve_role(&rules, &groups(&["teachers"])),
            SSO_FALLBACK_ROLE
        );
    }
}
