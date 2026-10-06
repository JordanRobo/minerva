//! Shared helpers for building HTTP error responses, so every endpoint group
//! translates failures the same way.
//!
//! Every `/api/*` error response uses one envelope shape:
//! `{"error": {"code": ..., "message": ...}}`, where `code` is a stable
//! snake_case identifier clients can branch on and `message` is
//! human-readable.

use actix_web::{HttpResponse, ResponseError, http};
use application::account_links::AccountLinkError;
use application::ports::RepositoryError;
use application::status_override::StatusOverrideError;
use serde::Serialize;
use std::time::Duration;
use utoipa::ToSchema;

/// The standard API error envelope. Handlers return it as the `Err` of a
/// `Result<HttpResponse, ApiError>`; the [`ResponseError`] impl turns it into
/// a JSON response with the matching status code.
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiError {
    /// Stable snake_case error code (e.g. `"not_found"`).
    pub code: String,
    /// Human-readable explanation of what went wrong.
    pub message: String,
    /// How long the client should wait before retrying; set only on 429
    /// responses and rendered as the `Retry-After` header, never in the body.
    #[serde(skip)]
    #[schema(ignore)]
    retry_after: Option<Duration>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl ApiError {
    /// 400 — the client sent invalid input.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "bad_request".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 401 — the request carries no valid credentials (a failed login, or a
    /// missing/expired session cookie). Deliberately generic: it must not
    /// reveal whether an account with the given email exists.
    pub fn unauthorized() -> Self {
        Self {
            code: "unauthorized".to_owned(),
            message: "invalid credentials".to_owned(),
            retry_after: None,
        }
    }

    /// 403 — the request is authenticated, but the user's role does not allow
    /// the action.
    pub fn forbidden() -> Self {
        Self {
            code: "forbidden".to_owned(),
            message: "you do not have permission to do this".to_owned(),
            retry_after: None,
        }
    }

    /// 404 — the requested resource does not exist.
    pub fn not_found() -> Self {
        Self {
            code: "not_found".to_owned(),
            message: "not found".to_owned(),
            retry_after: None,
        }
    }

    /// 409 — the operation conflicts with the current state (e.g. a duplicate).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: "conflict".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 409 — an account with the referenced email already exists.
    pub fn account_exists(message: impl Into<String>) -> Self {
        Self {
            code: "account_exists".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 409 — a group rule for this IdP group name already exists.
    pub fn group_rule_exists(message: impl Into<String>) -> Self {
        Self {
            code: "group_rule_exists".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 409 — the user's role is managed by SSO group rules and cannot be
    /// changed by hand while any rule exists.
    pub fn role_managed_by_sso(message: impl Into<String>) -> Self {
        Self {
            code: "role_managed_by_sso".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 400 — the one-time link in the request is unknown or no longer usable.
    /// One code and one message for all of them, so the answer never hints
    /// which.
    pub fn invalid_token(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_token".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 400 — the request referenced an entity that does not exist.
    pub fn invalid_reference(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_reference".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// 500 — something went wrong server-side. The message is generic on
    /// purpose: internals are not leaked to clients.
    pub fn internal_error() -> Self {
        Self {
            code: "internal_error".to_owned(),
            message: "internal server error".to_owned(),
            retry_after: None,
        }
    }

    /// 429 — the client has made too many attempts in a short period. The
    /// message is deliberately generic: it says neither which limit was hit
    /// nor how many attempts remain, and `retry_after` (rendered as the
    /// `Retry-After` header) reaches the end of the current window.
    pub fn rate_limited(retry_after: Duration) -> Self {
        Self {
            code: "rate_limited".to_owned(),
            message: "Too many attempts. Try again later.".to_owned(),
            retry_after: Some(retry_after),
        }
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> http::StatusCode {
        match self.code.as_str() {
            "unauthorized" => http::StatusCode::UNAUTHORIZED,
            "forbidden" => http::StatusCode::FORBIDDEN,
            "not_found" => http::StatusCode::NOT_FOUND,
            "rate_limited" => http::StatusCode::TOO_MANY_REQUESTS,
            "conflict" | "account_exists" | "group_rule_exists" | "role_managed_by_sso" => {
                http::StatusCode::CONFLICT
            }
            "internal_error" => http::StatusCode::INTERNAL_SERVER_ERROR,
            // bad_request, invalid_reference, invalid_token, and any unknown code
            _ => http::StatusCode::BAD_REQUEST,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut res = HttpResponse::build(self.status_code());
        if let Some(retry_after) = self.retry_after {
            // Whole seconds, rounded up: a client that waits this long is
            // never still inside the window.
            res.insert_header((
                http::header::RETRY_AFTER,
                retry_after.as_millis().div_ceil(1000).max(1) as u32,
            ));
        }
        res.json(serde_json::json!({ "error": self }))
    }
}

/// Translate a [`RepositoryError`] into the [`ApiError`] it renders as:
/// `NotFound` -> 404, `Conflict` -> 409 (detail surfaced),
/// `InvalidReference` -> 400 (detail surfaced), `Unexpected` -> 500
/// (generic message — internals are not leaked to clients).
pub fn repo_error_response(err: RepositoryError) -> ApiError {
    match err {
        RepositoryError::NotFound => ApiError::not_found(),
        RepositoryError::Conflict(detail) => ApiError::conflict(detail),
        RepositoryError::InvalidReference(detail) => ApiError::invalid_reference(detail),
        RepositoryError::Unexpected(_) => ApiError::internal_error(),
    }
}

/// Translate an [`AccountLinkError`] into the [`ApiError`] it renders as:
/// an unusable link -> 400 `invalid_token` (one answer for unknown, expired,
/// used and revoked alike), bad input -> 400 `bad_request`, a registered
/// email -> 409 `account_exists`, the remaining policy rejections -> 409
/// `conflict` with the service's message, a missing id -> 404. Repository
/// failures map like any other; a hash failure is a 500 whose detail is
/// logged server-side only (never a token or a link).
pub fn account_link_error_response(error: AccountLinkError) -> ApiError {
    match error {
        AccountLinkError::InvalidToken => ApiError::invalid_token(error.to_string()),
        AccountLinkError::InvalidEmail(_) | AccountLinkError::PasswordTooShort => {
            ApiError::bad_request(error.to_string())
        }
        AccountLinkError::EmailAlreadyRegistered => ApiError::account_exists(error.to_string()),
        AccountLinkError::NotFound => ApiError::not_found(),
        AccountLinkError::Repository(err) => repo_error_response(err),
        AccountLinkError::Hash(err) => {
            eprintln!("password hashing failed: {err}");
            ApiError::internal_error()
        }
        other => ApiError::conflict(other.to_string()),
    }
}

/// Translate a [`StatusOverrideError`] into the [`ApiError`] it renders as:
/// a missing goal or milestone -> 404, repository failures map like any
/// other.
pub fn status_override_error_response(error: StatusOverrideError) -> ApiError {
    match error {
        StatusOverrideError::NotFound => ApiError::not_found(),
        StatusOverrideError::Repository(err) => repo_error_response(err),
    }
}
