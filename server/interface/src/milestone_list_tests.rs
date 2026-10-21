//! Handler tests for the milestone list's pagination and filtering (roadmap
//! 3.10, 3.16), against a real Postgres: `GET /api/milestones` answers with
//! the `{items, total, limit, offset}` envelope whose items are the standard
//! milestone shape (effective status plus its source), pages in the fixed
//! order (target date ascending with undated milestones last, then created_at,
//! then id), walks every match exactly once, defaults to 50 per page, honours
//! the status/search/date filters — the status filter matching the effective
//! status, a manual override winning over the automatic value — and answers a
//! 400 `invalid_query` naming the offending parameter for any malformed query
//! value, including one the extractor itself cannot parse.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::milestone_list::MilestoneListService;
use application::pagination::{Page, PageRequest};
use application::ports::{
    MilestoneRepository, RepositoryError, SessionRepository, TaskCounts, TaskListFilter,
    TaskRepository, UserRepository,
};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use domain::{Milestone, MilestoneId, Role, Status, Task, TaskId, TaskStatus, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresMilestoneRepository, PostgresSessionRepository, PostgresTaskRepository,
    PostgresUserRepository,
};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

use crate::auth::{COOKIE_NAME, CookieSettings};
use crate::routes;

/// The `DATABASE_URL` the tests run against, or `None` to skip.
fn database_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    // In CI these tests must run: a green build that skipped them proves nothing.
    if url.is_none() && std::env::var_os("CI").is_some() {
        panic!("DATABASE_URL is not set; refusing to skip milestone-list tests in CI");
    }
    url
}

/// A one-connection pool (mirrors the other handler test modules: the default
/// pool size times the parallel test pools would exceed local Postgres's
/// `max_connections`). Pending migrations are applied once per process first.
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

/// A task attached to a milestone, created directly in the database.
fn test_task(milestone_id: MilestoneId, status: TaskStatus) -> Task {
    let now = Utc::now();
    Task {
        id: TaskId::new(),
        milestone_id: Some(milestone_id),
        title: "List count task".into(),
        description: None,
        status,
        target_date: None,
        assignee_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// Wraps the real task repository, counting `task_counts_for_milestones`
/// calls and delegating everything else to Postgres: proves a list page
/// resolves every milestone's counts with one grouped lookup (roadmap
/// 3.13), the twin of [`crate::task_assignee_tests::CountingUserRepository`]'s
/// D17 rule. `pub` so other handler test modules can reuse it.
pub struct CountingTaskRepository {
    pub inner: Arc<PostgresTaskRepository>,
    pub count_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TaskRepository for CountingTaskRepository {
    async fn create(&self, task: Task) -> Result<Task, RepositoryError> {
        self.inner.create(task).await
    }
    async fn find_by_id(&self, id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.inner.find_by_id(id).await
    }
    async fn find_by_ids(&self, ids: &[TaskId]) -> Result<Vec<Task>, RepositoryError> {
        self.inner.find_by_ids(ids).await
    }
    async fn task_counts_for_milestones(
        &self,
        milestone_ids: &[MilestoneId],
    ) -> Result<HashMap<MilestoneId, TaskCounts>, RepositoryError> {
        self.count_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.task_counts_for_milestones(milestone_ids).await
    }
    async fn update(&self, task: Task) -> Result<Task, RepositoryError> {
        self.inner.update(task).await
    }
    async fn set_status(&self, id: TaskId, status: TaskStatus) -> Result<Task, RepositoryError> {
        self.inner.set_status(id, status).await
    }
    async fn set_assignee(
        &self,
        id: TaskId,
        assignee_id: Option<UserId>,
    ) -> Result<Task, RepositoryError> {
        self.inner.set_assignee(id, assignee_id).await
    }
    async fn delete(&self, id: TaskId) -> Result<(), RepositoryError> {
        self.inner.delete(id).await
    }
    async fn list_page(
        &self,
        filter: &TaskListFilter,
        page: &PageRequest,
    ) -> Result<Page<Task>, RepositoryError> {
        self.inner.list_page(filter, page).await
    }
}

/// Build the app with the real route table and only the `web::Data` the list
/// handler (and the milestone routes these tests clean up through) can need;
/// the other routes 500 if hit, but these tests never do. A macro because
/// `init_service`'s service type is opaque. The task repository is always
/// the counting wrapper: two tests assert the one-grouped-lookup-per-page
/// rule on its counter, the rest pass a throwaway.
macro_rules! test_app {
    ($url:expr, $count_calls:expr) => {{
        let pool = test_pool($url);
        let sessions: Arc<dyn SessionRepository> =
            Arc::new(PostgresSessionRepository::new(pool.clone()));
        let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
        let users_data: web::Data<dyn UserRepository> = users.clone().into();
        let session_service = SessionService::new(
            sessions,
            Arc::new(Sha256SessionTokens),
            SessionService::DEFAULT_SESSION_TTL,
        );
        // The list goes through the service (roadmap 3.10), like in `main.rs`.
        let list =
            MilestoneListService::new(Arc::new(PostgresMilestoneRepository::new(pool.clone())));
        // The milestone handlers read task counts through the port (roadmap
        // 3.13), so a trait-object registration sits beside the concrete one,
        // like in `main.rs`.
        let task_counts: Arc<dyn TaskRepository> = Arc::new(CountingTaskRepository {
            inner: Arc::new(PostgresTaskRepository::new(pool.clone())),
            count_calls: $count_calls,
        });
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(web::Data::from(task_counts))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(list))
                .app_data(web::Data::new(CookieSettings { secure: false }))
                .configure(routes::configure),
        )
        .await
    }};
}

/// One request against the test app: `token` is the session cookie value (or
/// none), `body` a JSON body for POST/PUT/PATCH.
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

/// The parsed JSON body of a response.
async fn json_of(res: ServiceResponse) -> serde_json::Value {
    serde_json::from_slice(&read_body(res).await).expect("a JSON body")
}

fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4())
}

/// A user created directly in the database (no sign-in needed: sessions are
/// issued straight from the [`SessionService`]).
async fn create_user(pool: &PgPool, email: String, role: Role) -> User {
    let users = PostgresUserRepository::new(pool.clone());
    let now = Utc::now();
    let user = User {
        id: UserId::new(),
        email,
        password_hash: None,
        display_name: "Milestone list test user".into(),
        role,
        deactivated_at: None,
        role_managed_by_sso: false,
        sso_role_exempt: false,
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

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("milestone-list"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

/// A fixed date in 2026.
fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).expect("valid date")
}

/// A fixed timestamp in 2026, exact to the second.
fn at(month: u32, day: u32) -> DateTime<Utc> {
    date(month, day)
        .and_hms_opt(9, 0, 0)
        .expect("valid time")
        .and_utc()
}

/// A list row under this test's unique title prefix, so every assertion can
/// filter to the test's own rows (the test database is shared across parallel
/// runs).
fn list_milestone(
    prefix: &str,
    n: u32,
    status: Status,
    target_date: Option<NaiveDate>,
    created_at: DateTime<Utc>,
) -> Milestone {
    Milestone {
        id: MilestoneId::new(),
        title: format!("{prefix}-{n}"),
        description: None,
        status,
        status_override: None,
        target_date,
        owner_id: None,
        created_at,
        updated_at: created_at,
    }
}

/// Set a milestone's manual override directly in the database, the way
/// `StatusOverrideService` would (the list tests register no such service).
fn set_milestone_override(pool: &PgPool, id: MilestoneId, status: &str) {
    let mut conn = pool.get().expect("pool connection");
    diesel::update(infrastructure::schema::milestones::table.find(id.0))
        .set((
            infrastructure::schema::milestones::status_override.eq(status),
            infrastructure::schema::milestones::status_source.eq("manual_override"),
        ))
        .execute(&mut conn)
        .expect("set milestone override");
}

/// The milestone ids of a page's items, in the order the API returned them.
fn item_ids(json: &serde_json::Value) -> Vec<Uuid> {
    json["items"]
        .as_array()
        .expect("an items array")
        .iter()
        .map(|item| Uuid::parse_str(item["id"].as_str().expect("an id string")).expect("a uuid"))
        .collect()
}

/// The keys of a JSON object, for exact-shape assertions.
fn keys(json: &serde_json::Value) -> BTreeSet<&str> {
    json.as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[actix_web::test]
async fn the_list_answers_with_the_page_envelope_and_standard_milestone_items() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-{}", Uuid::new_v4().simple());
    let created = milestones
        .create(list_milestone(
            &prefix,
            1,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;

    // The envelope: exactly items/total/limit/offset, the defaults applied.
    assert_eq!(
        keys(&json),
        ["items", "limit", "offset", "total"].into_iter().collect()
    );
    assert_eq!(json["total"], 1);
    assert_eq!(json["limit"], PageRequest::DEFAULT_PAGE_LIMIT);
    assert_eq!(json["offset"], 0);

    // ...and each item is the standard milestone shape — no new fields. The
    // status is the effective one, with its source and the automatic value
    // underneath alongside, plus the task counts and completion share.
    let item = &json["items"][0];
    assert_eq!(item["id"], created.id.0.to_string());
    assert_eq!(item["status"], "on_track");
    assert_eq!(item["status_source"], "automatic");
    assert_eq!(item["automatic_status"], "on_track");
    assert_eq!(
        item["task_counts"],
        serde_json::json!({ "total": 0, "done": 0 })
    );
    assert_eq!(item["percent_complete"], 0);
    assert_eq!(
        keys(item),
        [
            "automatic_status",
            "created_at",
            "description",
            "id",
            "owner",
            "percent_complete",
            "status",
            "status_source",
            "target_date",
            "task_counts",
            "title",
            "updated_at"
        ]
        .into_iter()
        .collect()
    );

    milestones
        .delete(created.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_default_order_is_target_date_nulls_last_then_created_at_then_id() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-{}", Uuid::new_v4().simple());
    // Two rows share the earliest date and created_at: their order is the id.
    let a = milestones
        .create(list_milestone(
            &prefix,
            1,
            Status::OnTrack,
            Some(date(1, 5)),
            at(1, 3),
        ))
        .await
        .expect("create milestone");
    let b = milestones
        .create(list_milestone(
            &prefix,
            2,
            Status::OnTrack,
            Some(date(1, 5)),
            at(1, 3),
        ))
        .await
        .expect("create milestone");
    let c = milestones
        .create(list_milestone(
            &prefix,
            3,
            Status::OnTrack,
            Some(date(1, 10)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let d = milestones
        .create(list_milestone(&prefix, 4, Status::OnTrack, None, at(1, 1)))
        .await
        .expect("create milestone");
    let e = milestones
        .create(list_milestone(&prefix, 5, Status::OnTrack, None, at(1, 9)))
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);

    // The tied pair comes back in id order; the rest by date, then created_at.
    let (lo, hi) = if a.id.0 < b.id.0 {
        (a.id.0, b.id.0)
    } else {
        (b.id.0, a.id.0)
    };
    assert_eq!(
        item_ids(&json_of(res).await),
        vec![lo, hi, c.id.0, d.id.0, e.id.0]
    );

    for milestone in [a, b, c, d, e] {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn paging_walks_every_match_exactly_once_and_the_total_ignores_paging() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-{}", Uuid::new_v4().simple());
    // Seven rows that all tie on date and created_at: the only thing keeping
    // the pages stable is the id, so a drifting order would surface as a
    // duplicate or a gap.
    let mut created = Vec::new();
    for n in 1..=7u32 {
        created.push(
            milestones
                .create(list_milestone(
                    &prefix,
                    n,
                    Status::OnTrack,
                    Some(date(6, 1)),
                    at(1, 1),
                ))
                .await
                .expect("create milestone"),
        );
    }

    let mut seen = Vec::new();
    for offset in [0i64, 3, 6] {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/milestones?q={prefix}&limit=3&offset={offset}"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK);
        let json = json_of(res).await;
        // The total counts every match, ignoring limit and offset...
        assert_eq!(json["total"], 7);
        // ...and the pages do not overlap.
        seen.extend(item_ids(&json));
    }

    let mut expected: Vec<Uuid> = created.iter().map(|milestone| milestone.id.0).collect();
    expected.sort();
    let mut walked = seen.clone();
    walked.sort();
    assert_eq!(walked, expected, "every match exactly once");

    // An offset past the end is an empty page with the right total.
    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}&limit=3&offset=9"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert!(json["items"].as_array().expect("an items array").is_empty());
    assert_eq!(json["total"], 7);

    for milestone in created {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_default_page_limit_is_50() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-{}", Uuid::new_v4().simple());
    let mut created = Vec::new();
    for n in 1..=52u32 {
        created.push(
            milestones
                .create(list_milestone(
                    &prefix,
                    n,
                    Status::OnTrack,
                    Some(date(6, 1)),
                    at(1, 1),
                ))
                .await
                .expect("create milestone"),
        );
    }

    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["items"].as_array().expect("an items array").len(), 50);
    assert_eq!(json["total"], 52);
    assert_eq!(json["limit"], PageRequest::DEFAULT_PAGE_LIMIT);

    // The tail of the result set is reachable by offset.
    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}&offset=50"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["items"].as_array().expect("an items array").len(), 2);
    assert_eq!(json["total"], 52);

    for milestone in created {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_status_filter_matches_the_effective_status_and_search_is_literal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // One milestone per status under a shared prefix...
    let s_prefix = format!("ml-{}", Uuid::new_v4().simple());
    let on_track = milestones
        .create(list_milestone(
            &s_prefix,
            1,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let at_risk = milestones
        .create(list_milestone(
            &s_prefix,
            2,
            Status::AtRisk,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let off_track = milestones
        .create(list_milestone(
            &s_prefix,
            3,
            Status::OffTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let complete = milestones
        .create(list_milestone(
            &s_prefix,
            4,
            Status::Complete,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    // ...and one whose automatic status is OnTrack but whose manual override
    // says AtRisk: the filter must follow the override.
    let overridden = milestones
        .create(list_milestone(
            &s_prefix,
            5,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    set_milestone_override(&pool, overridden.id, "at_risk");

    let app_ref = &app;
    let token_ref: &str = &token;
    let get = |uri: String| async move {
        let res = request!(
            app_ref,
            Method::GET,
            uri,
            Some(token_ref),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK);
        item_ids(&json_of(res).await)
    };

    // One status: the automatic AtRisk milestone and the overridden one...
    let mut at_risk_page = get(format!("/api/milestones?q={s_prefix}&status=at_risk")).await;
    at_risk_page.sort();
    let mut expected = vec![at_risk.id.0, overridden.id.0];
    expected.sort();
    assert_eq!(at_risk_page, expected);
    // ...and the OnTrack page must NOT contain the overridden milestone: it
    // is AtRisk for now, whatever its automatic value says.
    assert_eq!(
        get(format!("/api/milestones?q={s_prefix}&status=on_track")).await,
        vec![on_track.id.0]
    );
    // Several statuses, comma-separated...
    let mut several = get(format!(
        "/api/milestones?q={s_prefix}&status=on_track,off_track"
    ))
    .await;
    several.sort();
    let mut expected = vec![on_track.id.0, off_track.id.0];
    expected.sort();
    assert_eq!(several, expected);
    // ...and the same set given as repeated parameters.
    let mut repeated = get(format!(
        "/api/milestones?q={s_prefix}&status=on_track&status=complete"
    ))
    .await;
    repeated.sort();
    let mut expected = vec![on_track.id.0, complete.id.0];
    expected.sort();
    assert_eq!(repeated, expected);

    // Search: a case-insensitive substring on the title (the needle is
    // uppercase, the stored titles are lowercase hex).
    let q_prefix = format!("ml-{}", Uuid::new_v4().simple());
    let alpha = milestones
        .create(list_milestone(
            &q_prefix,
            1,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let beta = milestones
        .create(list_milestone(
            &q_prefix,
            2,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let needle = format!("{}-1", q_prefix.to_uppercase());
    assert_eq!(
        get(format!("/api/milestones?q={needle}")).await,
        vec![alpha.id.0]
    );

    for milestone in [
        on_track, at_risk, off_track, complete, overridden, alpha, beta,
    ] {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_date_bounds_are_inclusive_and_exclude_undated_milestones() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-{}", Uuid::new_v4().simple());
    let early = milestones
        .create(list_milestone(
            &prefix,
            1,
            Status::OnTrack,
            Some(date(6, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let on_boundary = milestones
        .create(list_milestone(
            &prefix,
            2,
            Status::AtRisk,
            Some(date(6, 10)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let late = milestones
        .create(list_milestone(
            &prefix,
            3,
            Status::OnTrack,
            Some(date(7, 1)),
            at(1, 1),
        ))
        .await
        .expect("create milestone");
    let undated = milestones
        .create(list_milestone(&prefix, 4, Status::OnTrack, None, at(1, 1)))
        .await
        .expect("create milestone");

    let app_ref = &app;
    let token_ref: &str = &token;
    let get = |uri: String| async move {
        let res = request!(
            app_ref,
            Method::GET,
            uri,
            Some(token_ref),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::OK);
        item_ids(&json_of(res).await)
    };

    // Inclusive lower bound...
    let mut after = get(format!(
        "/api/milestones?q={prefix}&target_after=2026-06-10"
    ))
    .await;
    after.sort();
    let mut expected = vec![on_boundary.id.0, late.id.0];
    expected.sort();
    assert_eq!(after, expected);
    // ...inclusive upper bound...
    let mut before = get(format!(
        "/api/milestones?q={prefix}&target_before=2026-06-10"
    ))
    .await;
    before.sort();
    let mut expected = vec![early.id.0, on_boundary.id.0];
    expected.sort();
    assert_eq!(before, expected);
    // ...both bounds together...
    let mut range = get(format!(
        "/api/milestones?q={prefix}&target_after=2026-06-10&target_before=2026-06-30"
    ))
    .await;
    range.sort();
    assert_eq!(range, vec![on_boundary.id.0]);
    // ...and a date bound never matches an undated milestone.
    assert!(!after.contains(&undated.id.0));
    assert!(!before.contains(&undated.id.0));

    // The filters AND: only the AtRisk row sits on the boundary date.
    let mut combined = get(format!(
        "/api/milestones?q={prefix}&status=at_risk&target_after=2026-06-10&target_before=2026-07-31"
    ))
    .await;
    combined.sort();
    assert_eq!(combined, vec![on_boundary.id.0]);

    for milestone in [early, on_boundary, late, undated] {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn invalid_query_values_are_400s_naming_the_parameter() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let cases = [
        ("?limit=0", "limit"),
        (
            &format!("?limit={}", PageRequest::MAX_PAGE_LIMIT + 1),
            "limit",
        ),
        ("?limit=abc", "limit"),
        ("?offset=-1", "offset"),
        ("?offset=abc", "offset"),
        ("?status=bogus", "status"),
        ("?target_after=banana", "target_after"),
        ("?target_before=2026-13-40", "target_before"),
        (&format!("?q={}", "x".repeat(101)), "q"),
        // A duplicate parameter is unparseable by the extractor itself; the
        // query error handler must still answer with the standard envelope.
        ("?limit=1&limit=2", "limit"),
    ];
    for (query, param) in cases {
        let res = request!(
            &app,
            Method::GET,
            format!("/api/milestones{query}"),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{query}");
        let json = json_of(res).await;
        assert_eq!(json["error"]["code"], "invalid_query", "{query}");
        assert!(
            json["error"]["message"]
                .as_str()
                .expect("a message")
                .contains(param),
            "message names {param}: {}",
            json["error"]["message"]
        );
    }

    // Unknown parameters are ignored, not errors.
    let res = request!(
        &app,
        Method::GET,
        "/api/milestones?bogus=1",
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);

    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_list_reports_task_counts_and_percent_complete_per_milestone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url, Arc::new(AtomicUsize::new(0)));

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let tasks = PostgresTaskRepository::new(pool.clone());
    let prefix = format!("ml-counts-{}", Uuid::new_v4().simple());

    // Three milestones: no tasks, one of two done, both of two done.
    let none = milestones
        .create(list_milestone(&prefix, 1, Status::OnTrack, None, at(1, 1)))
        .await
        .expect("create milestone");
    let partial = milestones
        .create(list_milestone(&prefix, 2, Status::OnTrack, None, at(1, 2)))
        .await
        .expect("create milestone");
    let full = milestones
        .create(list_milestone(&prefix, 3, Status::OnTrack, None, at(1, 3)))
        .await
        .expect("create milestone");
    let mut created_tasks = Vec::new();
    for status in [TaskStatus::Backlog, TaskStatus::Done] {
        created_tasks.push(
            tasks
                .create(test_task(partial.id, status))
                .await
                .expect("create task"),
        );
    }
    for _ in 0..2 {
        created_tasks.push(
            tasks
                .create(test_task(full.id, TaskStatus::Done))
                .await
                .expect("create task"),
        );
    }

    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["total"], 3);

    // The ids outlive the lookups below, which shadow the milestone names.
    let none_id = none.id;
    let partial_id = partial.id;
    let full_id = full.id;

    fn item(json: &serde_json::Value, id: MilestoneId) -> &serde_json::Value {
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == id.0.to_string())
            .expect("the milestone in the list")
    }

    // Each item reports its own counts and completion share (roadmap 3.13):
    // zero, partial and all-done.
    let none = item(&json, none.id);
    assert_eq!(
        none["task_counts"],
        serde_json::json!({ "total": 0, "done": 0 })
    );
    assert_eq!(none["percent_complete"], 0);
    let partial = item(&json, partial.id);
    assert_eq!(
        partial["task_counts"],
        serde_json::json!({ "total": 2, "done": 1 })
    );
    assert_eq!(partial["percent_complete"], 50);
    let full = item(&json, full.id);
    assert_eq!(
        full["task_counts"],
        serde_json::json!({ "total": 2, "done": 2 })
    );
    assert_eq!(full["percent_complete"], 100);

    for task in &created_tasks {
        tasks.delete(task.id).await.expect("cleanup task");
    }
    milestones.delete(none_id).await.expect("cleanup milestone");
    milestones
        .delete(partial_id)
        .await
        .expect("cleanup milestone");
    milestones.delete(full_id).await.expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_page_of_milestones_costs_one_grouped_count_lookup() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let count_calls = Arc::new(AtomicUsize::new(0));
    let app = test_app!(&url, count_calls.clone());

    let (user, token) = staff(&pool).await;
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let prefix = format!("ml-counting-{}", Uuid::new_v4().simple());
    let a = milestones
        .create(list_milestone(&prefix, 1, Status::OnTrack, None, at(1, 1)))
        .await
        .expect("create milestone");
    let b = milestones
        .create(list_milestone(&prefix, 2, Status::OnTrack, None, at(1, 2)))
        .await
        .expect("create milestone");

    // A page with two items costs exactly one grouped lookup (roadmap 3.13),
    // not one per row...
    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_of(res).await;
    assert_eq!(json["total"], 2);
    assert_eq!(count_calls.load(Ordering::SeqCst), 1);

    // ...and a second page costs one more.
    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones?q={prefix}&limit=1"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_of(res).await["total"], 2);
    assert_eq!(count_calls.load(Ordering::SeqCst), 2);

    milestones.delete(a.id).await.expect("cleanup milestone");
    milestones.delete(b.id).await.expect("cleanup milestone");
    delete_user(&pool, user.id);
}
