//! Task comments: flat, plain-text notes on a task (roadmap 3.12, D20).

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::task::TaskId;
use crate::user::UserId;

/// Identifier for a [`TaskComment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TaskCommentId(pub Uuid);

impl TaskCommentId {
    /// Create a fresh identifier for a comment that does not exist yet.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskCommentId {
    fn default() -> Self {
        Self::new()
    }
}

/// The longest comment body, counted in Unicode characters (D20).
pub const MAX_COMMENT_BODY_CHARS: usize = 4000;

/// An error from validating a comment body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentBodyError {
    /// The body was blank after trimming.
    Blank,
    /// The trimmed body was longer than [`MAX_COMMENT_BODY_CHARS`] characters.
    TooLong(usize),
}

impl std::fmt::Display for CommentBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentBodyError::Blank => write!(f, "the comment body must not be blank"),
            CommentBodyError::TooLong(len) => {
                write!(
                    f,
                    "the comment body must be at most {MAX_COMMENT_BODY_CHARS} characters, got {len}"
                )
            }
        }
    }
}

impl std::error::Error for CommentBodyError {}

/// Trim a comment body and enforce the D20 rules: non-blank after trimming,
/// at most [`MAX_COMMENT_BODY_CHARS`] Unicode characters (not bytes). Interior
/// newlines are kept — the UI shows them as line breaks; there is no markdown
/// or HTML in v1. The domain does not check that the task or author exists;
/// that is an application concern.
pub fn normalize_comment_body(body: &str) -> Result<String, CommentBodyError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(CommentBodyError::Blank);
    }
    let len = trimmed.chars().count();
    if len > MAX_COMMENT_BODY_CHARS {
        return Err(CommentBodyError::TooLong(len));
    }
    Ok(trimmed.to_owned())
}

/// A flat, plain-text comment on a task (roadmap 3.12, D20).
///
/// No threading, mentions, reactions or attachments in v1. Editing replaces
/// the body in place — no edit history (D9) — and commenting never touches
/// the task itself (not its `updated_at`, not its status).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskComment {
    pub id: TaskCommentId,
    /// The task this comment belongs to; it is deleted with the task.
    pub task_id: TaskId,
    /// Who wrote the comment, if that can still be told — a user deleted
    /// after commenting leaves `None` (ON DELETE SET NULL, like D17).
    pub author_id: Option<UserId>,
    /// The body as stored: already trimmed and within
    /// [`MAX_COMMENT_BODY_CHARS`] characters.
    pub body: String,
    pub created_at: DateTime<Utc>,
    /// Equal to `created_at` until the body is edited in place.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_strips_surrounding_whitespace_including_newlines() {
        assert_eq!(
            normalize_comment_body("  hello\nworld  \n").unwrap(),
            "hello\nworld"
        );
    }

    #[test]
    fn interior_newlines_are_preserved() {
        assert_eq!(normalize_comment_body("a\nb\nc").unwrap(), "a\nb\nc");
    }

    #[test]
    fn blank_bodies_are_rejected() {
        for body in ["", "   ", "\n\t \n\r"] {
            assert!(
                matches!(normalize_comment_body(body), Err(CommentBodyError::Blank)),
                "{body:?} must be rejected as blank"
            );
        }
    }

    #[test]
    fn the_length_bound_is_exactly_max_comment_body_chars() {
        let at_limit = "a".repeat(MAX_COMMENT_BODY_CHARS);
        assert_eq!(normalize_comment_body(&at_limit).unwrap(), at_limit);

        let over = "a".repeat(MAX_COMMENT_BODY_CHARS + 1);
        assert_eq!(
            normalize_comment_body(&over),
            Err(CommentBodyError::TooLong(MAX_COMMENT_BODY_CHARS + 1))
        );

        // Trimming happens before the length check: padding does not count.
        let padded = format!(" {}\n", "a".repeat(MAX_COMMENT_BODY_CHARS));
        assert_eq!(normalize_comment_body(&padded).unwrap(), at_limit);
    }

    #[test]
    fn multi_byte_characters_are_counted_as_chars_not_bytes() {
        // é is two bytes in UTF-8, so 4000 of them are 8000 bytes; the limit
        // is in characters, not bytes.
        let at_limit = "é".repeat(MAX_COMMENT_BODY_CHARS);
        assert_eq!(at_limit.len(), MAX_COMMENT_BODY_CHARS * 2);
        assert_eq!(normalize_comment_body(&at_limit).unwrap(), at_limit);

        let over = "é".repeat(MAX_COMMENT_BODY_CHARS + 1);
        assert_eq!(
            normalize_comment_body(&over),
            Err(CommentBodyError::TooLong(MAX_COMMENT_BODY_CHARS + 1))
        );
    }
}
