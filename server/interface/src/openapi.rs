//! OpenAPI document for the Minerva API, assembled from the
//! `#[utoipa::path]` annotations on the handlers in `auth`, `goals`,
//! `milestones`, `redirect`, `sso_rules`, `tasks`, and `users`. Served as JSON at `/api-docs/openapi.json` and
//! rendered by Swagger UI at `/api-docs/swagger-ui/` (wired up in `main.rs`).
//!
//! The `*Doc` schema types mirror the wire shape of domain status types:
//! `domain` is pure logic and cannot depend on utoipa, so the documentation
//! restates those JSON shapes here instead of deriving them from the domain.
// The `*Doc` types are only referenced through utoipa proc macros (which emit
// their variant/field names as strings), so rustc's dead-code analysis never
// sees a runtime use of them.
#![allow(dead_code)]

use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::auth::COOKIE_NAME;

/// Registers the `session_cookie` security scheme: the session cookie as an
/// API key, so Swagger UI offers an Authorize button that fills it in. The
/// cookie name comes from [`COOKIE_NAME`] — the same constant the auth code
/// reads and writes — so the document cannot drift from the implementation.
struct SessionCookieSecurity;

impl Modify for SessionCookieSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "session_cookie",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(COOKIE_NAME))),
            );
        }
    }
}

/// Wire shape of [`domain::TaskStatus`]: a snake_case string.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusDoc {
    Backlog,
    ToDo,
    InProgress,
    Done,
}

/// Wire shape of [`domain::Role`]: a snake_case string.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleDoc {
    Admin,
    Staff,
    ReadOnly,
}

/// Wire shape of [`domain::AccountTokenStatus`]: a snake_case string.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatusDoc {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

/// Wire shape of [`application::account_links::LinkPurpose`]: a snake_case
/// string.
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkPurposeDoc {
    Invite,
    PasswordReset,
}

/// Wire shape of [`domain::Status`].
#[derive(ToSchema)]
pub enum StatusDoc {
    OnTrack,
    AtRisk,
    OffTrack,
    Complete,
}

/// Wire shape of [`domain::StatusSource`].
#[derive(ToSchema)]
pub enum StatusSourceDoc {
    Computed,
    ManualOverride,
}

/// Wire shape of [`domain::GoalStatus`]: a status together with where it came from.
#[derive(ToSchema)]
pub struct GoalStatusDoc {
    pub status: StatusDoc,
    pub source: StatusSourceDoc,
}

/// The OpenAPI 3 document for the Minerva API.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Minerva API",
        version = "0.1.0",
        description = "REST API for Minerva, a goal/milestone-first project-management platform for schools. Any signed-in user can read; Staff and Admin can create, edit and delete; Admin manages users. A 401 means the request has no valid session; a 403 means the session's role lacks the permission.",
    ),
    modifiers(&SessionCookieSecurity),
    tags(
        (name = "auth", description = "Authentication: login, logout, the current user, and account links (invites, password resets)."),
        (name = "goals", description = "Goals: the top-level outcomes a school works toward."),
        (name = "invites", description = "Invites: one-time links that create new accounts. Admin only."),
        (name = "milestones", description = "Milestones: dated checkpoints within a goal."),
        (name = "sso", description = "SSO group-to-role rules: which IdP group maps to which role while any rule exists. Admin only."),
        (name = "tasks", description = "Tasks: day-to-day work, optionally attached to a milestone."),
        (name = "users", description = "User administration: roles and active state. Admin only."),
    ),
    paths(
        crate::auth::login,
        crate::auth::logout,
        crate::auth::me,
        crate::auth::list_auth_providers,
        crate::account_links::inspect_token,
        crate::account_links::accept_invite,
        crate::account_links::reset_password,
        crate::redirect::redirect_login,
        crate::redirect::redirect_callback,
        crate::goals::create_goal,
        crate::goals::list_goals,
        crate::goals::get_goal,
        crate::goals::update_goal,
        crate::goals::delete_goal,
        crate::milestones::create_milestone,
        crate::milestones::list_milestones,
        crate::milestones::get_milestone,
        crate::milestones::update_milestone,
        crate::milestones::delete_milestone,
        crate::sso_rules::list_group_rules,
        crate::sso_rules::create_group_rule,
        crate::sso_rules::update_group_rule,
        crate::sso_rules::delete_group_rule,
        crate::tasks::create_task,
        crate::tasks::list_tasks,
        crate::tasks::get_task,
        crate::tasks::update_task,
        crate::tasks::delete_task,
        crate::users::list_users,
        crate::users::change_user_role,
        crate::users::deactivate_user,
        crate::users::reactivate_user,
        crate::users::create_password_reset,
        crate::invites::create_invite,
        crate::invites::list_invites,
        crate::invites::revoke_invite,
        crate::invites::reissue_invite,
    ),
    components(schemas(
        crate::error::ApiError,
        crate::auth::LoginRequest,
        crate::auth::UserResponse,
        crate::auth::AuthProvidersResponse,
        crate::auth::ProviderInfoResponse,
        crate::redirect::ProviderPath,
        crate::redirect::RedirectLoginQuery,
        crate::goals::GoalRequest,
        crate::goals::GoalResponse,
        crate::milestones::MilestoneRequest,
        crate::milestones::MilestoneResponse,
        crate::tasks::TaskListQuery,
        crate::tasks::TaskRequest,
        crate::tasks::TaskResponse,
        crate::sso_rules::SsoGroupRuleResponse,
        crate::sso_rules::GroupRuleRequest,
        crate::users::UserAdminResponse,
        crate::users::ChangeRoleRequest,
        crate::users::PasswordResetCreatedResponse,
        crate::invites::CreateInviteRequest,
        crate::invites::InviteResponse,
        crate::invites::InviteCreatedResponse,
        crate::account_links::InspectTokenRequest,
        crate::account_links::LinkInfoResponse,
        crate::account_links::AcceptInviteRequest,
        crate::account_links::ResetPasswordRequest,
        GoalStatusDoc,
        InviteStatusDoc,
        LinkPurposeDoc,
        RoleDoc,
        StatusDoc,
        StatusSourceDoc,
        TaskStatusDoc,
    ))
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::Method;
    use utoipa::openapi::path::{Operation, PathItem};
    use utoipa::openapi::security::SecurityRequirement;

    use crate::public_routes::PUBLIC_OPERATIONS;
    use crate::rate_limit_tests::RATE_LIMITED_OPERATIONS;

    /// The (method, operation) pairs a path item declares.
    fn operations(item: &PathItem) -> impl Iterator<Item = (Method, &Operation)> {
        [
            (Method::GET, item.get.as_ref()),
            (Method::POST, item.post.as_ref()),
            (Method::PUT, item.put.as_ref()),
            (Method::DELETE, item.delete.as_ref()),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method, operation)))
    }

    /// The document must match the access rules: every operation is either in
    /// the shared public allowlist or requires the session cookie and
    /// documents a 401; every protected write outside /api/auth/ also
    /// documents a 403. A new `#[utoipa::path]` without security (or an
    /// allowlist entry) fails here.
    #[test]
    fn openapi_document_matches_the_access_rules() {
        let doc = ApiDoc::openapi();

        // Every allowlisted operation exists in the document, so a renamed or
        // removed path cannot hide behind the allowlist.
        // Every allowlisted or rate-limited operation exists in the document,
        // so a renamed or removed path cannot hide behind either list.
        for (method, path) in PUBLIC_OPERATIONS
            .iter()
            .chain(RATE_LIMITED_OPERATIONS.iter())
        {
            let item = doc
                .paths
                .paths
                .get(*path)
                .unwrap_or_else(|| panic!("{method} {path} missing from document"));
            assert!(
                operations(item).any(|(m, _)| m == *method),
                "{method} {path} documented"
            );
        }

        for (path, item) in &doc.paths.paths {
            for (method, operation) in operations(item) {
                let public = PUBLIC_OPERATIONS
                    .iter()
                    .any(|(m, p)| *m == method && p == path);
                if public {
                    assert!(
                        operation.security.is_none(),
                        "{method} {path} is public; must not declare security"
                    );
                    continue;
                }
                let session = SecurityRequirement::new("session_cookie", Vec::<&str>::new());
                assert!(
                    operation
                        .security
                        .as_ref()
                        .is_some_and(|security| security.contains(&session)),
                    "{method} {path} must require the session cookie"
                );
                assert!(
                    operation.responses.responses.contains_key("401"),
                    "{method} {path} must document a 401"
                );
                if method != Method::GET && !path.starts_with("/api/auth/") {
                    assert!(
                        operation.responses.responses.contains_key("403"),
                        "{method} {path} must document a 403"
                    );
                }
                // Exactly the rate-limited operations document a 429: a new
                // limiter check without a documented response (or a stale one)
                // fails here.
                let rate_limited = RATE_LIMITED_OPERATIONS
                    .iter()
                    .any(|(m, p)| *m == method && p == path);
                assert!(
                    operation.responses.responses.contains_key("429") == rate_limited,
                    "{method} {path}: a 429 is documented exactly for the rate-limited operations"
                );
            }
        }
    }
}
