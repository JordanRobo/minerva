//! Shared helpers for building HTTP error responses, so every endpoint group
//! translates failures the same way.
//!
//! Every `/api/*` error response uses one envelope shape:
//! `{"error": {"code": ..., "message": ...}}`, where `code` is a stable
//! snake_case identifier clients can branch on and `message` is
//! human-readable.

use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use application::ports::RepositoryError;
use serde::Serialize;
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
        }
    }

    /// 404 — the requested resource does not exist.
    pub fn not_found() -> Self {
        Self {
            code: "not_found".to_owned(),
            message: "not found".to_owned(),
        }
    }

    /// 409 — the operation conflicts with the current state (e.g. a duplicate).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: "conflict".to_owned(),
            message: message.into(),
        }
    }

    /// 400 — the request referenced an entity that does not exist.
    pub fn invalid_reference(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_reference".to_owned(),
            message: message.into(),
        }
    }

    /// 500 — something went wrong server-side. The message is generic on
    /// purpose: internals are not leaked to clients.
    pub fn internal_error() -> Self {
        Self {
            code: "internal_error".to_owned(),
            message: "internal server error".to_owned(),
        }
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self.code.as_str() {
            "not_found" => StatusCode::NOT_FOUND,
            "conflict" => StatusCode::CONFLICT,
            "internal_error" => StatusCode::INTERNAL_SERVER_ERROR,
            // bad_request, invalid_reference, and any unknown code
            _ => StatusCode::BAD_REQUEST,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut res = HttpResponse::build(self.status_code());
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
