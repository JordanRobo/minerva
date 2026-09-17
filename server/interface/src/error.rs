//! Shared mapping from application-layer repository errors to HTTP responses,
//! so every endpoint group translates [`RepositoryError`] the same way.

use actix_web::HttpResponse;
use application::ports::RepositoryError;

/// Translate a [`RepositoryError`] into an HTTP error response:
/// `NotFound` -> 404, `Conflict` -> 409 (detail surfaced), `Unexpected` -> 500
/// (generic message — internals are not leaked to clients).
pub fn repo_error_response(err: RepositoryError) -> HttpResponse {
    match err {
        RepositoryError::NotFound => {
            HttpResponse::NotFound().json(serde_json::json!({ "error": "not found" }))
        }
        RepositoryError::Conflict(detail) => {
            HttpResponse::Conflict().json(serde_json::json!({ "error": detail }))
        }
        RepositoryError::Unexpected(_) => HttpResponse::InternalServerError()
            .json(serde_json::json!({ "error": "internal server error" })),
    }
}
