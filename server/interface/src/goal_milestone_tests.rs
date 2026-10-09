//! Handler tests for the goal–milestone linkage endpoints (roadmap 3.4),
//! against a real Postgres: linking shows up in both lists, repeat link and
//! unlink are no-ops that still answer 204, deleting either end drops the
//! link from the other side's list, unknown ids 404 with the code that names
//! which one is missing, listed entities carry their effective status, and
//! the lists are deterministically ordered — now as pages (roadmap 3.10):
//! the D16 envelope with a `total` that counts only linked rows, a paging
//! walk that yields each link exactly once, and 400s naming malformed
//! paging parameters.

use actix_web::App;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode, header};
use actix_web::test::{TestRequest, init_service, read_body};
use actix_web::web;
use application::auth::SessionService;
use application::goal_milestone_links::GoalMilestoneLinkService;
use application::pagination::PageRequest;
use application::ports::{GoalRepository, MilestoneRepository, SessionRepository, UserRepository};
use chrono::{NaiveDate, Utc};
use diesel::prelude::*;
use domain::{Goal, GoalId, Milestone, MilestoneId, Role, Status, User, UserId};
use infrastructure::Sha256SessionTokens;
use infrastructure::db::PgPool;
use infrastructure::repositories::{
    PostgresGoalMilestoneRepository, PostgresGoalRepository, PostgresMilestoneRepository,
    PostgresSessionRepository, PostgresUserRepository,
};
use std::sync::Arc;
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
        panic!("DATABASE_URL is not set; refusing to skip goal-milestone tests in CI");
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

/// Build the app with the real route table and only the `web::Data` the
/// linkage handlers (and the existing goal/milestone DELETE routes these
/// tests use to drop an end of a link) can need; the other routes 500 if hit,
/// but these tests never do. A macro because `init_service`'s service type is
/// opaque.
macro_rules! test_app {
    ($url:expr) => {{
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
        // The link service takes ports, so it gets its own repository
        // instances over the shared pool (mirrors `main.rs`).
        let links = GoalMilestoneLinkService::new(
            Arc::new(PostgresGoalRepository::new(pool.clone())),
            Arc::new(PostgresMilestoneRepository::new(pool.clone())),
            Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
        );
        init_service(
            App::new()
                .app_data(web::Data::new(PostgresGoalRepository::new(pool.clone())))
                .app_data(web::Data::new(PostgresMilestoneRepository::new(
                    pool.clone(),
                )))
                .app_data(users_data)
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(links))
                .app_data(web::Data::new(CookieSettings { secure: false }))
                .configure(routes::configure),
        )
        .await
    }};
}

/// One request against the test app: `token` is the session cookie value (or
/// none), `body` a JSON body for POST/PUT.
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

/// The `id` fields of the items in a page response, in list order.
fn listed_ids(json: &serde_json::Value) -> Vec<Uuid> {
    json["items"]
        .as_array()
        .expect("an items array")
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .expect("an id string")
                .parse()
                .expect("a uuid")
        })
        .collect()
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
        display_name: "Linkage test user".into(),
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

/// A goal with the given target date and no override.
fn test_goal(target_date: Option<NaiveDate>) -> Goal {
    let now = Utc::now();
    Goal {
        id: GoalId::new(),
        title: "Linkage test goal".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date,
        created_at: now,
        updated_at: now,
    }
}

/// A milestone with the given target date and no override.
fn test_milestone(target_date: Option<NaiveDate>) -> Milestone {
    let now = Utc::now();
    Milestone {
        id: MilestoneId::new(),
        title: "Linkage test milestone".into(),
        description: None,
        status: Status::OnTrack,
        status_override: None,
        target_date,
        created_at: now,
        updated_at: now,
    }
}

/// A Staff user with a session token, the way every test needs one.
async fn staff(pool: &PgPool) -> (User, String) {
    let user = create_user(pool, unique_email("linkage"), Role::Staff).await;
    let token = issue_token(pool, user.id).await;
    (user, token)
}

#[actix_web::test]
async fn linking_a_goal_to_a_milestone_shows_in_both_lists() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(listed_ids(&json_of(list).await), vec![milestone.id.0]);

    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(listed_ids(&json_of(list).await), vec![goal.id.0]);

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn linking_the_same_pair_twice_is_idempotent() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    for _ in 0..2 {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    // The pair is linked exactly once despite two link requests.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(listed_ids(&json_of(list).await), vec![milestone.id.0]);

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn unlinking_removes_the_link_and_repeating_is_a_no_op() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    let link = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(link.status(), StatusCode::NO_CONTENT);

    // The first unlink removes the link from both sides...
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(listed_ids(&json_of(list).await), Vec::<Uuid>::new());
    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(listed_ids(&json_of(list).await), Vec::<Uuid>::new());

    // ...and a second unlink of an unlinked pair still answers 204.
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_goal_and_a_milestone_can_each_link_to_many_of_the_other() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal_a = goals.create(test_goal(None)).await.expect("create goal");
    let goal_b = goals.create(test_goal(None)).await.expect("create goal");
    let milestone_a = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let milestone_b = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    for goal in [&goal_a, &goal_b] {
        for milestone in [&milestone_a, &milestone_b] {
            let res = request!(
                &app,
                Method::PUT,
                format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
                Some(&token),
                None::<&serde_json::Value>
            );
            assert_eq!(res.status(), StatusCode::NO_CONTENT);
        }
    }

    // Every side lists both of its partners.
    let mut expected_goals = vec![goal_a.id.0, goal_b.id.0];
    expected_goals.sort();
    for milestone in [&milestone_a, &milestone_b] {
        let list = request!(
            &app,
            Method::GET,
            format!("/api/milestones/{}/goals", milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let mut ids = listed_ids(&json_of(list).await);
        ids.sort();
        assert_eq!(ids, expected_goals);
    }
    let mut expected_milestones = vec![milestone_a.id.0, milestone_b.id.0];
    expected_milestones.sort();
    for goal in [&goal_a, &goal_b] {
        let list = request!(
            &app,
            Method::GET,
            format!("/api/goals/{}/milestones", goal.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let mut ids = listed_ids(&json_of(list).await);
        ids.sort();
        assert_eq!(ids, expected_milestones);
    }

    for goal in [goal_a, goal_b] {
        goals.delete(goal.id).await.expect("cleanup goal");
    }
    for milestone in [milestone_a, milestone_b] {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_goal_is_a_404_that_names_the_goal() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    // A real milestone on the other side of each request: the 404 must come
    // from the goal id, not the milestone one.
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let missing_goal = Uuid::new_v4();

    for method in [Method::PUT, Method::DELETE] {
        let res = request!(
            &app,
            method.clone(),
            format!("/api/goals/{missing_goal}/milestones/{}", milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(
            json_of(res).await["error"]["code"],
            "goal_not_found",
            "{method}"
        );
    }
    let res = request!(
        &app,
        Method::GET,
        format!("/api/goals/{missing_goal}/milestones"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(res).await["error"]["code"], "goal_not_found");

    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn an_unknown_milestone_is_a_404_that_names_the_milestone() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    // A real goal on the other side of each request: the 404 must come from
    // the milestone id, not the goal one.
    let goals = PostgresGoalRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let missing_milestone = Uuid::new_v4();

    for method in [Method::PUT, Method::DELETE] {
        let res = request!(
            &app,
            method.clone(),
            format!("/api/goals/{}/milestones/{missing_milestone}", goal.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(
            json_of(res).await["error"]["code"],
            "milestone_not_found",
            "{method}"
        );
    }
    let res = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{missing_milestone}/goals"),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_of(res).await["error"]["code"], "milestone_not_found");

    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn a_linked_milestone_reports_its_manual_override_in_the_list() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    // Stored automatic status differs from the override, so the list entry
    // has something visible to prove it reports the effective one.
    let mut milestone = test_milestone(None);
    milestone.status = Status::OnTrack;
    milestone.status_override = Some(Status::OffTrack);
    let milestone = milestones
        .create(milestone)
        .await
        .expect("create milestone");

    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    let entry = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == milestone.id.0.to_string())
        .expect("the milestone in the list");
    assert_eq!(entry["status"], "off_track");
    assert_eq!(entry["status_source"], "manual");

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn deleting_either_end_drops_the_link_from_the_other_side() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());

    // Deleting the goal drops it from the milestone's list...
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/goals/{}", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    let json = json_of(list).await;
    assert_eq!(listed_ids(&json), Vec::<Uuid>::new());
    // The total drops with the link, not just the items.
    assert_eq!(json["total"], 0);
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");

    // ...and deleting the milestone drops it from the goal's list.
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = request!(
        &app,
        Method::DELETE,
        format!("/api/milestones/{}", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    let json = json_of(list).await;
    assert_eq!(listed_ids(&json), Vec::<Uuid>::new());
    // The total drops with the link, not just the items.
    assert_eq!(json["total"], 0);

    // The milestone itself was deleted through the API above; only the goal
    // still needs removing.
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn lists_order_by_target_date_with_nulls_last() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    // Distinct target dates make the expected order independent of creation
    // time; one goal without a date must sort last.
    let january = NaiveDate::from_ymd_opt(2026, 1, 15).expect("a valid date");
    let may = NaiveDate::from_ymd_opt(2026, 5, 1).expect("a valid date");
    let goal_january = goals
        .create(test_goal(Some(january)))
        .await
        .expect("create goal");
    let goal_may = goals
        .create(test_goal(Some(may)))
        .await
        .expect("create goal");
    let goal_undated = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    for goal in [&goal_january, &goal_may, &goal_undated] {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(
        listed_ids(&json_of(list).await),
        vec![goal_january.id.0, goal_may.id.0, goal_undated.id.0]
    );

    for goal in [goal_january, goal_may, goal_undated] {
        goals.delete(goal.id).await.expect("cleanup goal");
    }
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_link_lists_answer_the_page_envelope() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // No query params: the defaults apply and the envelope is complete on
    // both sides of the relation.
    for uri in [
        format!("/api/goals/{}/milestones", goal.id.0),
        format!("/api/milestones/{}/goals", milestone.id.0),
    ] {
        let list = request!(
            &app,
            Method::GET,
            uri,
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json = json_of(list).await;
        assert!(json["items"].is_array());
        assert_eq!(json["total"], 1);
        assert_eq!(json["limit"], 50);
        assert_eq!(json["offset"], 0);
    }

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn the_total_counts_only_linked_rows() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    // Two of three milestones are linked; the third must not count.
    let linked_a = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let linked_b = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    let unlinked = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");
    for milestone in [&linked_a, &linked_b] {
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    assert_eq!(json["total"], 2);
    let mut ids = listed_ids(&json);
    ids.sort();
    let mut expected = vec![linked_a.id.0, linked_b.id.0];
    expected.sort();
    assert_eq!(ids, expected);

    // The mirror side counts only the goals linked to the milestone.
    let goal_two = goals.create(test_goal(None)).await.expect("create goal");
    let stray_goal = goals.create(test_goal(None)).await.expect("create goal");
    let res = request!(
        &app,
        Method::PUT,
        format!("/api/goals/{}/milestones/{}", goal_two.id.0, linked_a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let list = request!(
        &app,
        Method::GET,
        format!("/api/milestones/{}/goals", linked_a.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    // goal and goal_two are linked, stray_goal is not: total is 2, not 3.
    assert_eq!(json["total"], 2);

    for goal in [goal, goal_two, stray_goal] {
        goals.delete(goal.id).await.expect("cleanup goal");
    }
    for milestone in [linked_a, linked_b, unlinked] {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn paging_walks_the_linked_milestones_exactly_once_in_order() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    // Seven linked milestones: two share a target date, three have none. The
    // dated ones come first in date order, the undated last; creation order
    // breaks the ties inside each group.
    let january_fifth = NaiveDate::from_ymd_opt(2026, 1, 5).expect("a valid date");
    let february_tenth = NaiveDate::from_ymd_opt(2026, 2, 10).expect("a valid date");
    let march_third = NaiveDate::from_ymd_opt(2026, 3, 3).expect("a valid date");
    let target_dates = [
        Some(january_fifth),
        Some(january_fifth),
        Some(february_tenth),
        None,
        None,
        None,
        Some(march_third),
    ];
    let mut linked = Vec::new();
    for target_date in target_dates {
        let milestone = milestones
            .create(test_milestone(target_date))
            .await
            .expect("create milestone");
        let res = request!(
            &app,
            Method::PUT,
            format!("/api/goals/{}/milestones/{}", goal.id.0, milestone.id.0),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        linked.push(milestone);
    }

    // The full list fixes the deterministic order; the pages must reassemble
    // it. Dated first in date order, undated last.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let full = json_of(list).await;
    let full_order = listed_ids(&full);
    assert_eq!(full_order.len(), 7);
    let dates: Vec<Option<&str>> = full["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["target_date"].as_str())
        .collect();
    assert_eq!(
        dates,
        vec![
            Some("2026-01-05"),
            Some("2026-01-05"),
            Some("2026-02-10"),
            Some("2026-03-03"),
            None,
            None,
            None
        ]
    );

    let mut walked = Vec::new();
    for offset in [0u64, 3, 6] {
        let list = request!(
            &app,
            Method::GET,
            format!(
                "/api/goals/{}/milestones?limit=3&offset={offset}",
                goal.id.0
            ),
            Some(&token),
            None::<&serde_json::Value>
        );
        assert_eq!(list.status(), StatusCode::OK);
        let json = json_of(list).await;
        assert_eq!(json["total"], 7);
        assert_eq!(json["limit"], 3);
        assert_eq!(json["offset"], offset);
        walked.extend(listed_ids(&json));
    }
    let mut unique = walked.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 7, "each milestone exactly once");
    assert_eq!(walked, full_order, "pages reassemble the full order");

    // An offset past the end is an empty page with the total intact.
    let list = request!(
        &app,
        Method::GET,
        format!("/api/goals/{}/milestones?limit=3&offset=9", goal.id.0),
        Some(&token),
        None::<&serde_json::Value>
    );
    assert_eq!(list.status(), StatusCode::OK);
    let json = json_of(list).await;
    assert!(json["items"].as_array().unwrap().is_empty());
    assert_eq!(json["total"], 7);

    for milestone in linked {
        milestones
            .delete(milestone.id)
            .await
            .expect("cleanup milestone");
    }
    goals.delete(goal.id).await.expect("cleanup goal");
    delete_user(&pool, user.id);
}

#[actix_web::test]
async fn invalid_query_values_are_400s_naming_the_parameter() {
    let Some(url) = database_url() else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let pool = test_pool(&url);
    let app = test_app!(&url);

    let (user, token) = staff(&pool).await;
    let goals = PostgresGoalRepository::new(pool.clone());
    let milestones = PostgresMilestoneRepository::new(pool.clone());
    let goal = goals.create(test_goal(None)).await.expect("create goal");
    let milestone = milestones
        .create(test_milestone(None))
        .await
        .expect("create milestone");

    let cases = [
        ("?limit=0", "limit"),
        (
            &format!("?limit={}", PageRequest::MAX_PAGE_LIMIT + 1),
            "limit",
        ),
        ("?limit=abc", "limit"),
        ("?offset=-1", "offset"),
        ("?offset=abc", "offset"),
        // A duplicate parameter is unparseable by the extractor itself; the
        // query error handler must still answer with the standard envelope.
        ("?limit=1&limit=2", "limit"),
    ];
    for (query, param) in cases {
        for path in [
            format!("/api/goals/{}/milestones{query}", goal.id.0),
            format!("/api/milestones/{}/goals{query}", milestone.id.0),
        ] {
            let res = request!(
                &app,
                Method::GET,
                path,
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
    }

    goals.delete(goal.id).await.expect("cleanup goal");
    milestones
        .delete(milestone.id)
        .await
        .expect("cleanup milestone");
    delete_user(&pool, user.id);
}
