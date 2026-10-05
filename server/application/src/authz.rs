//! Authorization: the single place an access decision is made.
//!
//! Every request that needs more than "is there a session?" goes through
//! [`authorize`] with the permission its route requires. Future checks (a
//! deactivated account, ownership rules) land here too, so handlers and
//! extractors never compare roles themselves.

use domain::{Permission, User};

/// A failed authorization decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthzError {
    /// The user's role does not allow the requested permission.
    Forbidden,
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthzError::Forbidden => write!(f, "forbidden"),
        }
    }
}

impl std::error::Error for AuthzError {}

/// Whether `user` may perform `permission`.
///
/// The decision is the role's: [`domain::Role::allows`] lists every
/// (role, permission) pair explicitly.
pub fn authorize(user: &User, permission: Permission) -> Result<(), AuthzError> {
    if user.role.allows(permission) {
        Ok(())
    } else {
        Err(AuthzError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use domain::{Role, UserId};

    fn user_with(role: Role) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            email: "test@example.com".into(),
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

    /// `authorize` must agree with the full 3x3 matrix for every role and
    /// permission, so a flipped decision fails the test.
    #[test]
    fn authorize_decides_every_role_permission_pair() {
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
                let allowed = authorize(&user_with(*role), *permission).is_ok();
                assert_eq!(allowed, expected[row][column], "{role:?} / {permission:?}");
            }
        }
    }
}
