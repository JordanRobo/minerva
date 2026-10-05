//! Roles and permissions: what a user is allowed to do in Minerva.
//!
//! Every [`User`](crate::user::User) has a [`Role`], and [`Role::allows`]
//! answers whether that role may perform a [`Permission`]. This module only
//! defines the rules; checking them on real requests happens later (roadmap
//! 2.4), so nothing here touches I/O.

use serde::{Deserialize, Serialize};

/// The role assigned to a [`User`](crate::user::User).
///
/// The JSON names are snake_case strings (`admin`, `staff`, `read_only`);
/// the database stores the same strings in `users.role`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full access, including managing other users.
    Admin,
    /// Can view and edit content, but not manage users.
    Staff,
    /// Can only view content.
    ReadOnly,
}

impl std::fmt::Display for Role {
    /// The role's canonical string, the same one JSON and the database use.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Role::Admin => "admin",
            Role::Staff => "staff",
            Role::ReadOnly => "read_only",
        };
        f.write_str(name)
    }
}

/// An action a user may be allowed to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// View goals, milestones, tasks and their progress.
    ViewContent,
    /// Create and edit content.
    EditContent,
    /// Create, modify or remove user accounts.
    ManageUsers,
}

impl Role {
    /// Whether this role may perform `permission`.
    ///
    /// Every (role, permission) pair is listed: a new role or permission
    /// added later must make its decision here explicitly instead of
    /// inheriting one.
    pub fn allows(self, permission: Permission) -> bool {
        match (self, permission) {
            (Role::Admin, Permission::ViewContent) => true,
            (Role::Admin, Permission::EditContent) => true,
            (Role::Admin, Permission::ManageUsers) => true,
            (Role::Staff, Permission::ViewContent) => true,
            (Role::Staff, Permission::EditContent) => true,
            (Role::Staff, Permission::ManageUsers) => false,
            (Role::ReadOnly, Permission::ViewContent) => true,
            (Role::ReadOnly, Permission::EditContent) => false,
            (Role::ReadOnly, Permission::ManageUsers) => false,
        }
    }

    /// The role's position in the permissive ordering, least permissive
    /// first: `ReadOnly` < `Staff` < `Admin`.
    ///
    /// Every variant is listed, like [`allows`]: a role added later must
    /// place itself here explicitly. The SSO group-to-role mapping (roadmap
    /// 2.7) uses it to keep the least permissive of several matched roles.
    pub fn rank(self) -> u8 {
        match self {
            Role::ReadOnly => 0,
            Role::Staff => 1,
            Role::Admin => 2,
        }
    }
}

/// The role given to newly created accounts: SSO auto-creation today. The
/// SSO group-to-role mapping (roadmap 2.7) has its own fallback,
/// [`SSO_FALLBACK_ROLE`].
pub const DEFAULT_NEW_USER_ROLE: Role = Role::ReadOnly;

/// The role an SSO sign-in falls back to when no group rule matches — or
/// the groups claim is missing or malformed (roadmap 2.7, D15). Deliberately
/// a separate constant from [`DEFAULT_NEW_USER_ROLE`]: the two mean
/// different things and may diverge.
pub const SSO_FALLBACK_ROLE: Role = Role::ReadOnly;

#[cfg(test)]
mod tests {
    use super::*;

    /// The full 3x3 matrix of what `allows` must answer for every role and
    /// permission, so a flipped arm fails the test.
    #[test]
    fn allows_covers_the_full_role_permission_matrix() {
        let roles = [Role::Admin, Role::Staff, Role::ReadOnly];
        let permissions = [
            Permission::ViewContent,
            Permission::EditContent,
            Permission::ManageUsers,
        ];
        // Rows: Admin, Staff, ReadOnly. Columns: View, Edit, ManageUsers.
        let expected = [
            [true, true, true],
            [true, true, false],
            [true, false, false],
        ];
        for (row, role) in roles.iter().enumerate() {
            for (column, permission) in permissions.iter().enumerate() {
                assert_eq!(
                    role.allows(*permission),
                    expected[row][column],
                    "{role:?} / {permission:?}"
                );
            }
        }
    }

    #[test]
    fn new_accounts_default_to_read_only() {
        assert_eq!(DEFAULT_NEW_USER_ROLE, Role::ReadOnly);
    }

    #[test]
    fn rank_orders_roles_from_least_to_most_permissive() {
        assert!(Role::ReadOnly.rank() < Role::Staff.rank());
        assert!(Role::Staff.rank() < Role::Admin.rank());
    }

    #[test]
    fn sso_fallback_is_read_only() {
        assert_eq!(SSO_FALLBACK_ROLE, Role::ReadOnly);
    }

    #[test]
    fn display_matches_the_json_and_database_strings() {
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Staff.to_string(), "staff");
        assert_eq!(Role::ReadOnly.to_string(), "read_only");
    }
}
