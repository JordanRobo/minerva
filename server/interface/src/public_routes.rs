//! The public operations: reachable without a session cookie.
//!
//! Single source of truth shared by the live access tests (`access_tests`)
//! and the OpenAPI document test (`openapi`): a new route means either an
//! entry here (public) or an access extractor, and if the two ever disagree
//! about what is public, one of those tests fails.

use actix_web::http::Method;

/// Method + path of every public operation, with paths exactly as they appear
/// in the OpenAPI document (`{provider}` templated).
pub(crate) const PUBLIC_OPERATIONS: &[(Method, &str)] = &[
    (Method::POST, "/api/auth/login"),
    (Method::POST, "/api/auth/logout"),
    (Method::GET, "/api/auth/providers"),
    (Method::GET, "/api/auth/{provider}/login"),
    (Method::GET, "/api/auth/{provider}/callback"),
];
