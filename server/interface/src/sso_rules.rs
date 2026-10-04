//! SSO group rules HTTP API (roadmap 2.7, D15): the admin-only
//! `/api/sso/group-rules` handlers and their JSON DTOs. While at least one
//! rule exists, every SSO sign-in recomputes the user's role from the IdP's
//! groups claim; these routes are how an admin maintains that mapping.

use actix_web::{HttpResponse, web};
use application::sso_rules::{SsoGroupRuleError, SsoGroupRuleService};
use chrono::{DateTime, Utc};
use domain::{Role, SsoGroupRule, SsoGroupRuleId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::AdminAccess;
use crate::error::{ApiError, repo_error_response};
use crate::openapi::RoleDoc;

/// JSON shape of a group rule in responses.
#[derive(Serialize, ToSchema)]
pub struct SsoGroupRuleResponse {
    pub id: Uuid,
    pub group_name: String,
    #[schema(value_type = RoleDoc)]
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&SsoGroupRule> for SsoGroupRuleResponse {
    fn from(rule: &SsoGroupRule) -> Self {
        Self {
            id: rule.id.0,
            group_name: rule.group_name.clone(),
            role: rule.role,
            created_at: rule.created_at,
            updated_at: rule.updated_at,
        }
    }
}

/// Body for `POST /api/sso/group-rules` and `PUT /api/sso/group-rules/{id}`.
/// The id and timestamps are server-managed; surrounding whitespace in the
/// name is ignored.
#[derive(Deserialize, ToSchema)]
pub struct GroupRuleRequest {
    pub group_name: String,
    #[schema(value_type = RoleDoc)]
    pub role: Role,
}

fn rule_error_response(error: SsoGroupRuleError) -> ApiError {
    match error {
        SsoGroupRuleError::InvalidGroupName => ApiError::bad_request(error.to_string()),
        SsoGroupRuleError::GroupRuleExists => ApiError::group_rule_exists(error.to_string()),
        SsoGroupRuleError::NotFound => ApiError::not_found(),
        SsoGroupRuleError::Repository(err) => repo_error_response(err),
    }
}

/// List Group Rules
///
/// List every SSO group-to-role rule. Requires the Admin role.
#[utoipa::path(
    get,
    path = "/api/sso/group-rules",
    tags = ["sso"],
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "All group rules", body = Vec<SsoGroupRuleResponse>),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError)
    )
)]
pub async fn list_group_rules(
    service: web::Data<SsoGroupRuleService>,
    _access: AdminAccess,
) -> Result<HttpResponse, ApiError> {
    let rules = service.list().await.map_err(rule_error_response)?;
    Ok(HttpResponse::Ok().json(
        rules
            .iter()
            .map(SsoGroupRuleResponse::from)
            .collect::<Vec<_>>(),
    ))
}

/// Create Group Rule
///
/// Add a rule mapping one IdP group name to a role. Requires the Admin role.
#[utoipa::path(
    post,
    path = "/api/sso/group-rules",
    tags = ["sso"],
    security(("session_cookie" = [])),
    request_body = GroupRuleRequest,
    responses(
        (status = 201, description = "The created rule", body = SsoGroupRuleResponse),
        (status = 400, description = "The group name is empty or longer than 255 characters, or the role is unknown", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 409, description = "A rule for this group already exists", body = ApiError)
    )
)]
pub async fn create_group_rule(
    service: web::Data<SsoGroupRuleService>,
    _access: AdminAccess,
    body: web::Json<GroupRuleRequest>,
) -> Result<HttpResponse, ApiError> {
    let rule = service
        .create(body.group_name.clone(), body.role)
        .await
        .map_err(rule_error_response)?;
    Ok(HttpResponse::Created().json(SsoGroupRuleResponse::from(&rule)))
}

/// Update Group Rule
///
/// Change a rule's group name and/or role. Requires the Admin role.
#[utoipa::path(
    put,
    path = "/api/sso/group-rules/{id}",
    tags = ["sso"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Group rule identifier")),
    request_body = GroupRuleRequest,
    responses(
        (status = 200, description = "The updated rule", body = SsoGroupRuleResponse),
        (status = 400, description = "The group name is empty or longer than 255 characters, or the role is unknown", body = ApiError),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No group rule with this id", body = ApiError),
        (status = 409, description = "A rule for this group already exists", body = ApiError)
    )
)]
pub async fn update_group_rule(
    service: web::Data<SsoGroupRuleService>,
    _access: AdminAccess,
    path: web::Path<Uuid>,
    body: web::Json<GroupRuleRequest>,
) -> Result<HttpResponse, ApiError> {
    let rule = service
        .update(SsoGroupRuleId(*path), body.group_name.clone(), body.role)
        .await
        .map_err(rule_error_response)?;
    Ok(HttpResponse::Ok().json(SsoGroupRuleResponse::from(&rule)))
}

/// Delete Group Rule
///
/// Remove a rule. Deleting the last one switches SSO sign-ins off role
/// recomputation (D15). Requires the Admin role.
#[utoipa::path(
    delete,
    path = "/api/sso/group-rules/{id}",
    tags = ["sso"],
    security(("session_cookie" = [])),
    params(("id" = Uuid, Path, description = "Group rule identifier")),
    responses(
        (status = 204, description = "The rule was deleted"),
        (status = 401, description = "Missing or invalid session", body = ApiError),
        (status = 403, description = "Requires the Admin role", body = ApiError),
        (status = 404, description = "No group rule with this id", body = ApiError)
    )
)]
pub async fn delete_group_rule(
    service: web::Data<SsoGroupRuleService>,
    _access: AdminAccess,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    service
        .delete(SsoGroupRuleId(*path))
        .await
        .map_err(rule_error_response)?;
    Ok(HttpResponse::NoContent().finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::dev::Service;
    use actix_web::http::{Method, StatusCode, header};
    use actix_web::test::{TestRequest, init_service, read_body};
    use application::account_links::AccountLinkService;
    use application::auth::SessionService;
    use application::auth::password::PasswordAuthProvider;
    use application::auth::provider::AuthProviders;
    use application::ports::{PasswordHasher, SessionRepository, SessionTokens, UserRepository};
    use chrono::Utc;
    use diesel::prelude::*;
    use domain::{User, UserId};
    use infrastructure::db::PgPool;
    use infrastructure::repositories::{
        PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
        PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresSessionRepository,
        PostgresSsoGroupRuleRepository, PostgresTaskRelationRepository, PostgresTaskRepository,
        PostgresUserRepository,
    };
    use infrastructure::{Argon2PasswordHasher, NoEmailSender, Sha256SessionTokens};
    use std::sync::Arc;

    use crate::auth::{COOKIE_NAME, CookieSettings};
    use crate::routes;

    /// The `DATABASE_URL` the tests run against, or `None` to skip.
    fn database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty());
        // In CI these tests must run: a green build that skipped them proves nothing.
        if url.is_none() && std::env::var_os("CI").is_some() {
            panic!("DATABASE_URL is not set; refusing to skip group-rule API tests in CI");
        }
        url
    }

    /// A one-connection pool (mirrors the access tests: the default pool size
    /// times the parallel test pools would exceed local Postgres's
    /// `max_connections`). Pending migrations are applied once per process.
    fn test_pool(url: &str) -> PgPool {
        let pool = diesel::r2d2::Pool::builder()
            .max_size(1)
            .build(diesel::r2d2::ConnectionManager::<diesel::PgConnection>::new(url))
            .expect("could not create test pool");
        static MIGRATIONS_APPLIED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        MIGRATIONS_APPLIED.get_or_init(|| {
            infrastructure::migrations::run_migrations(&pool)
                .expect("could not apply migrations in tests");
        });
        pool
    }

    /// Build the app with the real route table and every `web::Data` a handler
    /// in it can need, mirroring `main.rs`. A macro because `init_service`'s
    /// service type is opaque.
    macro_rules! test_app {
        ($url:expr) => {{
            let pool = test_pool($url);
            let sessions: Arc<dyn SessionRepository> =
                Arc::new(PostgresSessionRepository::new(pool.clone()));
            let users: Arc<dyn UserRepository> =
                Arc::new(PostgresUserRepository::new(pool.clone()));
            let hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
            let users_data: web::Data<dyn UserRepository> = users.clone().into();
            // The login route resolves providers by id, mirroring `main.rs`.
            let providers = web::Data::new(
                AuthProviders::new(
                    vec![Arc::new(PasswordAuthProvider::new(
                        users.clone(),
                        hasher.clone(),
                    ))],
                    Vec::new(),
                )
                .expect("static provider ids are valid and unique"),
            );
            let session_tokens: Arc<dyn SessionTokens> = Arc::new(Sha256SessionTokens);
            let session_service = SessionService::new(
                sessions,
                session_tokens.clone(),
                SessionService::DEFAULT_SESSION_TTL,
            );
            // Mirrors `main.rs`: the invite and password-reset routes go
            // through the account-link service (no email delivery in tests,
            // so links come back site-relative).
            let account_link_service = AccountLinkService::new(
                Arc::new(PostgresAccountTokenRepository::new(pool.clone())),
                users.clone(),
                hasher.clone(),
                session_tokens,
                session_service.clone(),
                Arc::new(NoEmailSender),
                String::new(),
            );
            init_service(
                App::new()
                    .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                    .app_data(web::Data::new(PostgresMilestoneRepository::new(
                        pool.clone(),
                    )))
                    .app_data(web::Data::new(PostgresGoalMilestoneRepository::new(
                        pool.clone(),
                    )))
                    .app_data(web::Data::new(PostgresTaskRepository::new(pool.clone())))
                    .app_data(web::Data::new(PostgresTaskRelationRepository::new(
                        pool.clone(),
                    )))
                    .app_data(web::Data::new(PostgresProgressSnapshotRepository::new(
                        pool.clone(),
                    )))
                    .app_data(users_data)
                    .app_data(web::Data::new(session_service.clone()))
                    .app_data(web::Data::new(account_link_service))
                    .app_data(web::Data::new(SsoGroupRuleService::new(Arc::new(
                        PostgresSsoGroupRuleRepository::new(pool.clone()),
                    ))))
                    .app_data(providers)
                    .app_data(web::Data::new(CookieSettings { secure: false }))
                    .configure(routes::configure),
            )
            .await
        }};
    }

    /// One request against the test app: `token` is the session cookie value
    /// (or none), `body` a JSON body for POST/PUT.
    macro_rules! request {
        ($app:expr, $method:expr, $uri:expr, $token:expr, $body:expr) => {{
            // `Method` is not `Copy`; callers keep their value for the
            // assertions, so the macro clones it.
            let mut req = TestRequest::with_uri(&$uri).method($method.clone());
            if let Some(body) = $body {
                req = req.set_json(body);
            }
            let token: Option<&str> = $token;
            if let Some(t) = token {
                req = req.insert_header((header::COOKIE, format!("{COOKIE_NAME}={t}")));
            }
            $app.call(req.to_request()).await.unwrap()
        }};
    }

    fn unique_email(prefix: &str) -> String {
        format!("{prefix}-{}@example.com", Uuid::new_v4())
    }

    /// A group name no other test (or run) can collide with.
    fn fresh_group_name() -> String {
        format!("grp-{}", Uuid::new_v4())
    }

    /// A user created directly in the database (no sign-in needed: sessions
    /// are issued straight from the [`SessionService`]).
    async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
        let users = PostgresUserRepository::new(pool.clone());
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            email,
            password_hash: None,
            display_name: "Group-rule API test user".into(),
            role,
            deactivated_at: None,
            created_at: now,
            updated_at: now,
        };
        users.create(user).await.expect("create user")
    }

    /// Delete a user row directly (the repository has no delete); its sessions
    /// cascade-delete with it.
    fn delete_user(pool: &PgPool, user_id: UserId) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::users::table.find(user_id.0))
            .execute(&mut conn)
            .expect("delete user");
    }

    /// A live session token for `user_id`, issued like any sign-in would be.
    async fn issue_token(pool: &PgPool, user_id: UserId) -> String {
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let service = SessionService::new(
            sessions,
            Arc::new(Sha256SessionTokens),
            SessionService::DEFAULT_SESSION_TTL,
        );
        service.issue(user_id).await.expect("issue session").token
    }

    /// An admin with a live session; the caller deletes both at the end.
    async fn seeded_admin(pool: &PgPool) -> (User, String) {
        let admin = create_user(pool, unique_email("sso-rules"), Role::Admin).await;
        let token = issue_token(pool, admin.id).await;
        (admin, token)
    }

    /// Delete a rule row directly, so tests that end on an error still clean up.
    fn delete_rule(pool: &PgPool, id: Uuid) {
        let mut conn = pool.get().expect("pool connection");
        diesel::delete(infrastructure::schema::sso_group_role_rules::table.find(id))
            .execute(&mut conn)
            .expect("delete group rule");
    }

    #[actix_web::test]
    async fn an_admin_can_manage_group_rules() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);
        let (admin, token) = seeded_admin(&pool).await;

        // Create.
        let name = fresh_group_name();
        let created = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": name, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        let id: Uuid = json["id"].as_str().unwrap().parse().unwrap();
        assert_eq!(json["group_name"], name);
        assert_eq!(json["role"], "staff");
        assert!(json.get("created_at").is_some());
        assert!(json.get("updated_at").is_some());

        // List.
        let listed = request!(
            &app,
            Method::GET,
            "/api/sso/group-rules",
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(listed.status(), StatusCode::OK);
        let list: serde_json::Value = serde_json::from_slice(&read_body(listed).await).unwrap();
        assert!(
            list.as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == id.to_string())
        );

        // Update.
        let renamed = fresh_group_name();
        let updated = request!(
            &app,
            Method::PUT,
            format!("/api/sso/group-rules/{id}"),
            Some(&token),
            Some(&serde_json::json!({ "group_name": renamed, "role": "admin" }))
        );
        assert_eq!(updated.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&read_body(updated).await).unwrap();
        assert_eq!(json["id"], id.to_string());
        assert_eq!(json["group_name"], renamed);
        assert_eq!(json["role"], "admin");

        // Delete.
        let deleted = request!(
            &app,
            Method::DELETE,
            format!("/api/sso/group-rules/{id}"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        let list: serde_json::Value = serde_json::from_slice(
            &read_body(request!(
                &app,
                Method::GET,
                "/api/sso/group-rules",
                Some(&token),
                None::<&serde_json::Value>
            ))
            .await,
        )
        .unwrap();
        assert!(
            !list
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == id.to_string())
        );

        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn a_duplicate_group_name_is_a_conflict() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);
        let (admin, token) = seeded_admin(&pool).await;
        let mut cleanup = Vec::new();

        let name = fresh_group_name();
        let created = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": name, "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        cleanup.push(json["id"].as_str().unwrap().parse().unwrap());

        // A second rule for the same name is a typed conflict.
        let duplicate = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": name, "role": "admin" }))
        );
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(duplicate).await).unwrap();
        assert_eq!(json["error"]["code"], "group_rule_exists");

        // ...and so is renaming another rule onto a taken name.
        let other_name = fresh_group_name();
        let other = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": other_name, "role": "staff" }))
        );
        assert_eq!(other.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(other).await).unwrap();
        cleanup.push(json["id"].as_str().unwrap().parse().unwrap());
        let other_id: Uuid = json["id"].as_str().unwrap().parse().unwrap();
        let renamed = request!(
            &app,
            Method::PUT,
            format!("/api/sso/group-rules/{other_id}"),
            Some(&token),
            Some(&serde_json::json!({ "group_name": name, "role": "staff" }))
        );
        assert_eq!(renamed.status(), StatusCode::CONFLICT);
        let json: serde_json::Value = serde_json::from_slice(&read_body(renamed).await).unwrap();
        assert_eq!(json["error"]["code"], "group_rule_exists");

        for id in cleanup {
            delete_rule(&pool, id);
        }
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn names_are_trimmed_before_storage() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);
        let (admin, token) = seeded_admin(&pool).await;
        let mut cleanup = Vec::new();

        let name = fresh_group_name();
        let created = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": format!("  {name}  "), "role": "staff" }))
        );
        assert_eq!(created.status(), StatusCode::CREATED);
        let json: serde_json::Value = serde_json::from_slice(&read_body(created).await).unwrap();
        assert_eq!(
            json["group_name"], name,
            "surrounding whitespace is trimmed"
        );
        cleanup.push(json["id"].as_str().unwrap().parse().unwrap());

        // The trimmed form is what is stored: the un-padded name is taken.
        let duplicate = request!(
            &app,
            Method::POST,
            "/api/sso/group-rules",
            Some(&token),
            Some(&serde_json::json!({ "group_name": name, "role": "staff" }))
        );
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);

        for id in cleanup {
            delete_rule(&pool, id);
        }
        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn invalid_names_and_roles_are_bad_requests() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);
        let (admin, token) = seeded_admin(&pool).await;

        let bodies = [
            serde_json::json!({ "group_name": "", "role": "staff" }),
            serde_json::json!({ "group_name": "   ", "role": "staff" }),
            serde_json::json!({ "group_name": "a".repeat(256), "role": "staff" }),
            serde_json::json!({ "group_name": "teachers", "role": "superadmin" }),
            serde_json::json!({ "role": "staff" }),
        ];
        for body in &bodies {
            let res = request!(
                &app,
                Method::POST,
                "/api/sso/group-rules",
                Some(&token),
                Some(body)
            );
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
            let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
            assert_eq!(json["error"]["code"], "bad_request", "{body}");
        }

        delete_user(&pool, admin.id);
    }

    #[actix_web::test]
    async fn an_unknown_id_is_not_found() {
        let Some(url) = database_url() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = test_pool(&url);
        let app = test_app!(&url);
        let (admin, token) = seeded_admin(&pool).await;

        let id = Uuid::new_v4();
        for method in [Method::PUT, Method::DELETE] {
            let body: Option<serde_json::Value> = if method == Method::PUT {
                Some(serde_json::json!({ "group_name": "teachers", "role": "staff" }))
            } else {
                None
            };
            let res = request!(
                &app,
                method.clone(),
                format!("/api/sso/group-rules/{id}"),
                Some(&token),
                body.as_ref()
            );
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
            let json: serde_json::Value = serde_json::from_slice(&read_body(res).await).unwrap();
            assert_eq!(json["error"]["code"], "not_found", "{method}");
        }

        delete_user(&pool, admin.id);
    }
}
