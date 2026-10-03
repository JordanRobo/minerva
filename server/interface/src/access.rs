//! Access-level request extractors.
//!
//! [`AuthenticatedUser`](crate::auth::AuthenticatedUser) answers "is there a
//! valid session?"; these answer "may this user do what this route does?".
//! Each wraps it and asks `application::authz` for the decision:
//!
//! - [`ViewAccess`] — view content (the GET goal/milestone/task routes).
//! - [`EditAccess`] — create or modify content (POST/PUT/DELETE).
//! - [`AdminAccess`] — manage users; today only the `/debug/*` routes.
//!
//! A missing or invalid session is a 401 (from `AuthenticatedUser`); a valid
//! session whose role lacks the permission is a 403 with the standard error
//! envelope. Handlers declare their required level in their signature — no
//! inline role comparisons. The `user` field is exposed for checks that need
//! the identity itself (ownership, deactivation); handlers that only need
//! the decision ignore it.

use actix_web::dev::Payload;
use actix_web::{FromRequest, HttpRequest};
use application::authz::{AuthzError, authorize};
use domain::{Permission, User};
use std::future::Future;
use std::pin::Pin;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;

/// A signed-in user who may view content.
#[allow(dead_code)] // `user` is unused until a check needs the identity
pub struct ViewAccess {
    pub user: User,
}

/// A signed-in user who may create or edit content.
#[allow(dead_code)] // `user` is unused until a check needs the identity
pub struct EditAccess {
    pub user: User,
}

/// A signed-in user who may manage users.
#[allow(dead_code)] // `user` is unused until a check needs the identity
pub struct AdminAccess {
    pub user: User,
}

/// Resolve the session and check `permission` in one step: 401 when there is
/// no valid session, 403 when the role lacks the permission.
async fn resolve(req: &HttpRequest, permission: Permission) -> Result<User, ApiError> {
    let user = AuthenticatedUser::resolve(req).await?;
    if let Err(AuthzError::Forbidden) = authorize(&user, permission) {
        return Err(ApiError::forbidden());
    }
    Ok(user)
}

macro_rules! access_extractor {
    ($name:ident, $permission:expr) => {
        impl FromRequest for $name {
            type Error = ApiError;
            type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

            fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
                // The future must be 'static, so work on an owned clone of the
                // request (cheap: headers + extensions), like AuthenticatedUser.
                let req = req.clone();
                Box::pin(async move {
                    let user = resolve(&req, $permission).await?;
                    Ok($name { user })
                })
            }
        }
    };
}

access_extractor!(ViewAccess, Permission::ViewContent);
access_extractor!(EditAccess, Permission::EditContent);
access_extractor!(AdminAccess, Permission::ManageUsers);
