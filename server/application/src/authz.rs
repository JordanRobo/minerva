//! Authorization: the single place an access decision is made.
//!
//! Every request that needs more than "is there a session?" goes through
//! [`authorize`] with the permission its route requires. Future checks (a
//! deactivated account, ownership rules) land here too, so handlers and
//! extractors never compare roles themselves.

use domain::{Permission, Role, TaskComment, User};

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

/// Whether `actor` may edit `comment` (D20): only the author, regardless of
/// role. A comment whose author has since been deleted can be edited by
/// nobody — not even an Admin; it can only be deleted.
pub fn authorize_comment_edit(actor: &User, comment: &TaskComment) -> Result<(), AuthzError> {
    if comment.author_id == Some(actor.id) {
        Ok(())
    } else {
        Err(AuthzError::Forbidden)
    }
}

/// Whether `actor` may delete `comment` (D20): the author, or any Admin —
/// including a comment whose author has since been deleted. Every role is
/// listed explicitly, like [`domain::Role::allows`]: a role added later must
/// make its decision here instead of inheriting one.
pub fn authorize_comment_delete(actor: &User, comment: &TaskComment) -> Result<(), AuthzError> {
    let allowed = match actor.role {
        Role::Admin => true,
        Role::Staff | Role::ReadOnly => comment.author_id == Some(actor.id),
    };
    if allowed {
        Ok(())
    } else {
        Err(AuthzError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use domain::{Role, TaskComment, TaskCommentId, TaskId, UserId};

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

    /// A comment on a fixed task by the given author (or none).
    fn comment_with(author: Option<UserId>) -> TaskComment {
        let now = Utc::now();
        TaskComment {
            id: TaskCommentId::new(),
            task_id: TaskId::new(),
            author_id: author,
            body: "a comment".into(),
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

    /// D20's edit rule: only the author may edit a comment, whatever their
    /// role; a comment with no author can be edited by nobody.
    #[test]
    fn only_the_author_may_edit_a_comment() {
        let author = user_with(Role::Staff);
        let comment = comment_with(Some(author.id));

        assert!(authorize_comment_edit(&author, &comment).is_ok());
        for role in [Role::Staff, Role::Admin, Role::ReadOnly] {
            assert!(
                matches!(
                    authorize_comment_edit(&user_with(role), &comment),
                    Err(AuthzError::Forbidden)
                ),
                "{role:?} must not edit someone else's comment"
            );
        }
        // An authorless comment is editable by nobody, Admin included.
        let authorless = comment_with(None);
        for role in [Role::Admin, Role::Staff, Role::ReadOnly] {
            assert!(
                matches!(
                    authorize_comment_edit(&user_with(role), &authorless),
                    Err(AuthzError::Forbidden)
                ),
                "{role:?} must not edit an authorless comment"
            );
        }
    }

    /// D20's delete rule: the author or any Admin — including a comment whose
    /// author has since been deleted; no one else may.
    #[test]
    fn the_author_or_an_admin_may_delete_a_comment() {
        let author = user_with(Role::Staff);
        let comment = comment_with(Some(author.id));

        assert!(authorize_comment_delete(&author, &comment).is_ok());
        assert!(authorize_comment_delete(&user_with(Role::Admin), &comment).is_ok());
        // An Admin may delete an authorless comment too...
        assert!(authorize_comment_delete(&user_with(Role::Admin), &comment_with(None)).is_ok());
        // ...but no one else may, whether the comment has an author or not.
        for role in [Role::Staff, Role::ReadOnly] {
            for author_id in [Some(author.id), None] {
                assert!(
                    matches!(
                        authorize_comment_delete(&user_with(role), &comment_with(author_id)),
                        Err(AuthzError::Forbidden)
                    ),
                    "{role:?} must not delete a comment with author {author_id:?}"
                );
            }
        }
    }
}
