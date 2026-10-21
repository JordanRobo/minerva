mod access;
mod account_links;
mod auth;
mod client_ip;
mod config;
mod debug;
mod error;
mod goal_milestones;
mod goals;
mod invites;
mod maintenance;
mod milestones;
mod openapi;
mod people;
mod redirect;
mod routes;
mod sso_rules;
mod task_relations;
mod tasks;
mod users;

#[cfg(test)]
mod access_tests;

#[cfg(test)]
mod goal_list_tests;

#[cfg(test)]
mod goal_milestone_tests;

#[cfg(test)]
mod milestone_list_tests;

#[cfg(test)]
mod owner_tests;

#[cfg(test)]
mod public_routes;

#[cfg(test)]
mod rate_limit_tests;

#[cfg(test)]
mod status_override_tests;

#[cfg(test)]
mod status_recompute_tests;

#[cfg(test)]
mod task_assignee_tests;

#[cfg(test)]
mod task_list_tests;

#[cfg(test)]
mod task_relation_tests;

#[cfg(test)]
mod task_status_tests;

use actix_web::cookie::Key;
use actix_web::{App, HttpServer, web};
use application::account_links::AccountLinkService;
use application::assignment::AssignmentService;
use application::auth::SessionService;
use application::auth::oidc::OidcAuthProvider;
use application::auth::password::PasswordAuthProvider;
use application::auth::provider::{AuthProviders, RedirectProvider};
use application::bootstrap::{BootstrapAdmin, BootstrapOutcome, bootstrap_admin};
use application::goal_list::GoalListService;
use application::goal_milestone_links::GoalMilestoneLinkService;
use application::goal_service::GoalService;
use application::milestone_list::MilestoneListService;
use application::milestone_service::MilestoneService;
use application::oidc_login::LoginPolicy;
use application::ports::{
    AccountEmailSender, AccountTokenRepository, NoopStatusSnapshotTrigger, OidcProvider,
    PasswordHasher, SessionRepository, SessionTokens, StatusSnapshotTrigger, SystemClock,
    TaskRepository, UserIdentityRepository, UserRepository,
};
use application::rate_limit::{RateLimitService, RateLimiter};
use application::sso_roles::SsoRoleService;
use application::sso_rules::SsoGroupRuleService;
use application::status_computation::{StatusComputationService, StatusRecomputation};
use application::status_override::StatusOverrideService;
use application::task_list::TaskListService;
use application::task_relations::TaskRelationService;
use application::task_service::TaskService;
use application::task_status::TaskStatusService;
use application::user_admin::UserAdminService;
use chrono::Utc;
use infrastructure::Argon2PasswordHasher;
use infrastructure::NoEmailSender;
use infrastructure::Sha256SessionTokens;
use infrastructure::db::build_pool;
use infrastructure::migrations::run_migrations;
use infrastructure::oidc::{OidcConfig, OpenIdConnectProvider};
use infrastructure::repositories::{
    PostgresAccountTokenRepository, PostgresGoalMilestoneRepository, PostgresGoalRepository,
    PostgresMilestoneRepository, PostgresProgressSnapshotRepository, PostgresRateLimiter,
    PostgresSessionRepository, PostgresSsoGroupRuleRepository, PostgresTaskRelationRepository,
    PostgresTaskRepository, PostgresUserIdentityRepository, PostgresUserRepository,
    RedisRateLimiter, RedisSessionRepository,
};
use std::sync::Arc;

use crate::redirect::RedirectFlow;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // All file and environment access happens in the config module; it layers
    // defaults, minerva.toml and environment overrides, and reports every
    // problem together instead of failing one fix at a time.
    let config = match config::load() {
        Ok(config) => config,
        Err(errors) => {
            eprintln!("configuration error(s):");
            for error in &errors {
                eprintln!("  - {error}");
            }
            std::process::exit(1);
        }
    };

    let port = config.server.port;

    // Fail fast if the primary datastore is missing or unreachable: a server
    // without its database has no meaningful degraded mode. `build_pool` also
    // checks one connection, so an unreachable Postgres panics here at startup
    // rather than on the first request.
    let database_url = config.database.url.expose().to_owned();
    let pool = build_pool(&database_url);

    // Apply pending migrations so a fresh database is usable without a
    // separate migration step. Several nodes can start at once (the API
    // scales horizontally), and `run_migrations` serializes them with a
    // Postgres advisory lock. server.run_migrations = false opts out, e.g.
    // when a dedicated migration job owns the schema.
    if config.server.run_migrations {
        match run_migrations(&pool) {
            Ok(applied) if applied.is_empty() => println!("no pending migrations"),
            Ok(applied) => println!("applied {} migration(s)", applied.len()),
            Err(err) => panic!("{err}"),
        }
    } else {
        println!("run_migrations is false; skipping migrations");
    }

    // One shared pool, ten repositories. Each is registered as its own
    // `web::Data` rather than wrapped in a single AppState struct: every
    // handler uses exactly one repository, so per-repo Data keeps each
    // handler's signature naming only the repo it actually calls. (The pool
    // clones are cheap — r2d2 pools share their connections behind an Arc.)
    let goals = web::Data::new(PostgresGoalRepository::new(pool.clone()));
    let milestones = web::Data::new(PostgresMilestoneRepository::new(pool.clone()));
    let goal_milestones = web::Data::new(PostgresGoalMilestoneRepository::new(pool.clone()));
    let tasks = web::Data::new(PostgresTaskRepository::new(pool.clone()));
    // The milestone handlers read task counts through the port (roadmap
    // 3.13), so a trait-object registration sits beside the concrete one
    // above; tests can swap in a counting fake for it.
    let task_counts: Arc<dyn TaskRepository> = Arc::new(PostgresTaskRepository::new(pool.clone()));
    let task_relations = web::Data::new(PostgresTaskRelationRepository::new(pool.clone()));
    let progress_snapshots = web::Data::new(PostgresProgressSnapshotRepository::new(pool.clone()));
    // Manual status overrides (roadmap 3.2): set/clear go through the service
    // so the no-op and snapshot rules live in one place. Snapshots are not
    // built yet (roadmap 3.14), so the trigger is the no-op; a real one swaps
    // in here without touching the handlers. The service takes ports, so it
    // gets its own repository instances over the shared pool (r2d2 handles).
    // Built before the maintenance runner below, which moves the pool.
    let status_snapshots: Arc<dyn StatusSnapshotTrigger> = Arc::new(NoopStatusSnapshotTrigger);
    let status_snapshots_data: web::Data<dyn StatusSnapshotTrigger> =
        status_snapshots.clone().into();
    // Automatic status recomputation (roadmap 3.13, D12/D19): milestones from
    // their task counts and dates, goals from their linked milestones'
    // effective statuses. Built before the mutation services below because
    // every one of them shares it as the `StatusRecomputation` port and
    // recomputes best-effort after each write (roadmap 3.13, step 4). Like
    // the status-override service it takes ports, over its own repository
    // instances on the shared pool.
    let status_computation = Arc::new(StatusComputationService::new(
        Arc::new(PostgresGoalRepository::new(pool.clone())),
        Arc::new(PostgresMilestoneRepository::new(pool.clone())),
        Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        Arc::new(SystemClock),
    ));
    let recomputation: Arc<dyn StatusRecomputation> = status_computation.clone();
    // One full recompute at startup (roadmap 3.13, step 5): existing rows
    // carry whatever statuses their last mutation left them — for a fresh
    // database, the defaults — so refresh everything before serving.
    // Best-effort like the per-mutation recomputes: a failure only warns and
    // the hourly sweep below heals any miss. No advisory lock: the recompute
    // writes nothing when nothing changed, so concurrent multi-node backfills
    // are harmless.
    match status_computation.recompute_all().await {
        Ok(counts) if counts.milestones + counts.goals > 0 => println!(
            "status backfill: refreshed {} milestone(s), {} goal(s)",
            counts.milestones, counts.goals
        ),
        Ok(_) => {}
        Err(err) => eprintln!("warning: startup status backfill failed: {err}"),
    }
    let status_override_service = web::Data::new(StatusOverrideService::new(
        Arc::new(PostgresGoalRepository::new(pool.clone())),
        Arc::new(PostgresMilestoneRepository::new(pool.clone())),
        status_snapshots,
        recomputation.clone(),
    ));
    // Goal–milestone linkage (roadmap 3.4): link/unlink/list go through the
    // service so the existence and idempotency rules live in one place. Like
    // the status-override service it takes ports, so it gets its own
    // repository instances over the shared pool.
    let goal_milestone_link_service = web::Data::new(GoalMilestoneLinkService::new(
        Arc::new(PostgresGoalRepository::new(pool.clone())),
        Arc::new(PostgresMilestoneRepository::new(pool.clone())),
        Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
        recomputation.clone(),
    ));
    // Task relations (roadmap 3.6, D4): create/delete/list go through the
    // service so the existence checks and canonicalisation live in one place.
    // Like the linkage service it takes ports, so it gets its own repository
    // instances over the shared pool.
    let task_relation_service = web::Data::new(TaskRelationService::new(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        Arc::new(PostgresTaskRelationRepository::new(pool.clone())),
    ));
    // Board-state transitions (roadmap 3.7, D6): the column move goes through
    // the service so the no-op rule lives in one place; like the relation
    // service it takes ports, over its own repository instance on the shared
    // pool. No snapshot trigger: a board move is not a status change (D5).
    let task_status_service = web::Data::new(TaskStatusService::new(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        recomputation.clone(),
    ));
    // Task create/update/delete (roadmap 3.13, step 4): writes go through the
    // service so the best-effort status recompute lives with the mutation;
    // like the board-move service it takes ports over its own repository
    // instance on the shared pool.
    let task_service = web::Data::new(TaskService::new(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        recomputation.clone(),
    ));
    // Milestone create/update/delete: the same pattern; delete also
    // recomputes the goals linked to the milestone, captured before the
    // links cascade away with it.
    let milestone_service = web::Data::new(MilestoneService::new(
        Arc::new(PostgresMilestoneRepository::new(pool.clone())),
        Arc::new(PostgresGoalMilestoneRepository::new(pool.clone())),
        recomputation.clone(),
    ));
    // Goal create: computes the goal's initial status. Goal update and delete
    // are not triggers (a goal's status depends only on its linked
    // milestones), so those handlers keep talking to the repository directly.
    let goal_service = web::Data::new(GoalService::new(
        Arc::new(PostgresGoalRepository::new(pool.clone())),
        recomputation,
    ));
    // Task list pagination & filtering (roadmap 3.10, 3.16, D16): the list
    // goes through the service so the handler never touches a repository.
    let task_list_service = web::Data::new(TaskListService::new(Arc::new(
        PostgresTaskRepository::new(pool.clone()),
    )));
    // Goal and milestone list pagination & filtering (roadmap 3.10, 3.16):
    // the same pattern as the task list — each service takes its own
    // repository instance over the shared pool so the handler stays thin.
    let goal_list_service = web::Data::new(GoalListService::new(Arc::new(
        PostgresGoalRepository::new(pool.clone()),
    )));
    let milestone_list_service = web::Data::new(MilestoneListService::new(Arc::new(
        PostgresMilestoneRepository::new(pool.clone()),
    )));
    // SSO group rules (roadmap 2.7, D15): the admin-only API goes through the
    // service so name validation and the duplicate-name conflict live in one
    // place (see application::sso_rules).
    // The user-admin service below shares the same repository: it asks
    // whether any rule exists to lock SSO-managed roles against hand edits.
    let sso_group_rules_repo = Arc::new(PostgresSsoGroupRuleRepository::new(pool.clone()));
    let sso_group_rules = web::Data::new(SsoGroupRuleService::new(sso_group_rules_repo.clone()));
    // The auth handlers take ports, not concrete repositories, so these are
    // registered as trait objects (the same way sessions below are).
    let users: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool.clone()));
    let users_data: web::Data<dyn UserRepository> = users.clone().into();
    // Assignees and owners (roadmap 3.11, D17): set/clear go through the
    // service so the user-existence and no-op rules live in one place; like
    // the status-override service it takes ports, over its own repository
    // instances on the shared pool. No handlers yet: the endpoints land with
    // the people picker (roadmap 3.11).
    let assignment_service = web::Data::new(AssignmentService::new(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        Arc::new(PostgresGoalRepository::new(pool.clone())),
        Arc::new(PostgresMilestoneRepository::new(pool.clone())),
        users.clone(),
    ));
    // The token store behind invites and password resets (roadmap 2.6); the
    // account-link service below and the OIDC provider (when enabled) both
    // take it — the latter to consume a pending invite when an invited email
    // signs in via SSO before accepting it.
    let account_tokens: Arc<dyn AccountTokenRepository> =
        Arc::new(PostgresAccountTokenRepository::new(pool.clone()));
    // The OIDC provider below also takes these ports, so the Arcs stay
    // around instead of being converted straight to `web::Data`.
    let user_identities: Arc<dyn UserIdentityRepository> =
        Arc::new(PostgresUserIdentityRepository::new(pool.clone()));
    let user_identities_data: web::Data<dyn UserIdentityRepository> =
        user_identities.clone().into();
    // SSO group-to-role mapping (roadmap 2.7, D15): while any rule exists,
    // the OIDC provider below recomputes roles from the IdP groups at every
    // login. It shares the rules repository with the user-admin service's
    // hand-edit lock, so both read the same committed rules.
    let sso_roles = SsoRoleService::new(sso_group_rules_repo.clone(), users.clone());
    // Redis when redis.url is configured, Postgres otherwise — one shared
    // selection for rate-limit counters and sessions below. In-process
    // counters are deliberately not an option: they break horizontal scaling.
    let postgres_stores = config.redis.url.expose().trim().is_empty();
    // Rate limiting (roadmap 2.8): fixed-window counters behind the shared
    // storage selection above. Built before the maintenance runner below,
    // which moves the pool.
    let rate_limiter: Arc<dyn RateLimiter> = if postgres_stores {
        println!("redis not configured; using Postgres for rate-limit counters");
        Arc::new(PostgresRateLimiter::new(pool.clone()))
    } else {
        println!("using Redis for rate-limit counters");
        Arc::new(
            RedisRateLimiter::connect(config.redis.url.expose())
                .expect("redis.url is set but could not connect to Redis"),
        )
    };
    // Sessions are the swappable storage behind the shared selection above.
    // A missing Redis URL is not an error — it just means "use Postgres for
    // sessions" (see docs/architecture.md). A present but unreachable one
    // fails fast, like database.url does. The URL may carry credentials, so
    // it stays out of the log line.
    let sessions: Arc<dyn SessionRepository> = if postgres_stores {
        println!("redis not configured; using Postgres for session storage");
        Arc::new(PostgresSessionRepository::new(pool.clone()))
    } else {
        println!("using Redis for session storage");
        Arc::new(
            RedisSessionRepository::connect(config.redis.url.expose())
                .expect("redis.url is set but could not connect to Redis"),
        )
    };
    // The maintenance runner starts on every node (roadmap 3.13, step 5): the
    // status recompute needs only Postgres and runs wherever the sessions
    // live; with Redis, native TTLs evict expired sessions and stale
    // rate-limit counters instead of the hourly purges, so those two jobs
    // register only when Postgres holds the stores. Every node ticks every
    // job it registers; the advisory locks mean exactly one node does each
    // job's work per tick (roadmap 2.8).
    maintenance::start(
        pool,
        maintenance::jobs_for(
            status_computation.clone(),
            postgres_stores.then(|| (sessions.clone(), rate_limiter.clone())),
        ),
    );
    // The raw repository stays registered alongside the service: handlers go
    // through the service (the TTL is the fixed 30 days), but code that needs
    // session storage directly can still extract the port.
    let sessions_data: web::Data<dyn SessionRepository> = sessions.clone().into();
    // The raw-token generator is shared between the session service and the
    // account-link service, so both kinds of token come from one port.
    let session_tokens: Arc<dyn SessionTokens> = Arc::new(Sha256SessionTokens);
    let session_service = web::Data::new(SessionService::new(
        sessions,
        session_tokens.clone(),
        SessionService::DEFAULT_SESSION_TTL,
    ));
    // Rate limiting (roadmap 2.8): the service hashes subjects and fails open
    // on store errors (see application::rate_limit). It guards the login,
    // token-link and admin issue endpoints; the config registration carries
    // the client-IP header setting the handlers resolve it with.
    let rate_limit_service = web::Data::new(RateLimitService::new(
        rate_limiter,
        config.rate_limit.enabled,
    ));
    let rate_limit_config = web::Data::new(config.rate_limit.clone());
    // User administration goes through the service so the self-modification
    // and last-admin rules live in one place (see application::user_admin).
    let user_admin = web::Data::new(UserAdminService::new(
        users.clone(),
        sso_group_rules_repo,
        session_service.get_ref().clone(),
    ));
    // The password hasher holds no state; the password provider and the
    // bootstrap admin below both need it, so it is built once here.
    let password_hasher: Arc<dyn PasswordHasher> = Arc::new(Argon2PasswordHasher);
    // Account links (roadmap 2.6): invites and password resets go through the
    // service so the link rules live in one place. Email delivery is not
    // configured yet (roadmap 7.3), so NoEmailSender: every send fails and
    // the endpoints return the links to the caller instead of emailing them.
    let email: Arc<dyn AccountEmailSender> = Arc::new(NoEmailSender);
    let account_link_service = web::Data::new(AccountLinkService::new(
        account_tokens.clone(),
        users.clone(),
        password_hasher.clone(),
        session_tokens,
        session_service.get_ref().clone(),
        email,
        // Links are built from server.web_base_url — already trailing-slash-
        // stripped when set, empty for site-relative links.
        config.server.web_base_url.clone(),
    ));
    // First-admin bootstrap (roadmap 2.5): while the database has no users,
    // create the configured admin so there is someone who can log in and
    // start inviting people. The repository decides atomically whether the
    // table is empty, so racing nodes cannot both create an admin; once any
    // user exists this prints only the skip line. It runs regardless of
    // run_migrations — the schema exists either way (migrated above or owned
    // by a dedicated job). The password never reaches these log lines.
    if config.bootstrap_enabled() {
        match bootstrap_admin(
            users.as_ref(),
            password_hasher.as_ref(),
            BootstrapAdmin {
                email: config.bootstrap.admin_email.clone(),
                display_name: config.bootstrap.admin_display_name.clone(),
                password: config.bootstrap.admin_password.expose().to_owned(),
            },
            Utc::now(),
        )
        .await
        {
            Ok(BootstrapOutcome::Created(user)) => {
                println!("created bootstrap admin account {}", user.email)
            }
            Ok(BootstrapOutcome::UsersAlreadyExist) => {
                println!("bootstrap admin skipped: users already exist")
            }
            Err(err) => panic!("{err}"),
        }
    }
    // OIDC sign-in is optional: without oidc.issuer_url the server behaves
    // exactly as before and nothing redirect-related is registered. With it,
    // an unreachable/misconfigured IdP that answers 4xx fails startup like
    // database.url does; a merely unreachable IdP only warns (discovery
    // retries lazily on first use). The configuration has already validated
    // every URL and secret, so the parses below cannot fail.
    let (redirect_providers, redirect_flow) = if config.oidc_enabled() {
        let provider_config = OidcConfig {
            issuer_url: url::Url::parse(&config.oidc.issuer_url)
                .expect("validated by the configuration"),
            client_id: config.oidc.client_id.clone(),
            client_secret: config.oidc.client_secret.expose().to_owned(),
            redirect_url: url::Url::parse(&config.oidc.redirect_url)
                .expect("validated by the configuration"),
            display_name: config.oidc.display_name.clone(),
            scopes: config.oidc.scopes.clone(),
            groups_claim: config.oidc.groups_claim.clone(),
        };
        let protocol: Arc<dyn OidcProvider> = Arc::new(
            OpenIdConnectProvider::connect(provider_config)
                .await
                .unwrap_or_else(|err| panic!("{err}")),
        );
        println!("OIDC enabled (issuer {})", config.oidc.issuer_url);
        // The protocol provider is wrapped in the login-policy provider and
        // registered like any other redirect provider.
        let oidc: Arc<dyn RedirectProvider> = Arc::new(OidcAuthProvider::new(
            protocol,
            users.clone(),
            user_identities.clone(),
            account_tokens,
            sso_roles,
            LoginPolicy {
                auto_create_users: config.oidc.auto_create_users,
            },
        ));
        (
            vec![oidc],
            // The state cookie is shared by all redirect providers, so it is
            // registered exactly when at least one exists; its key still comes
            // from oidc.state_secret (the only secret of the right size today).
            Some(web::Data::new(RedirectFlow {
                key: Key::derive_from(config.oidc.state_secret.expose().as_bytes()),
                base_url: config.server.web_base_url.clone(),
            })),
        )
    } else {
        println!("OIDC not configured");
        (Vec::new(), None)
    };

    // Every sign-in method is a registered provider, looked up by id: adding
    // one later means implementing a trait and extending this list, not
    // touching session handling or the existing handlers.
    let auth_providers = web::Data::new(
        AuthProviders::new(
            vec![Arc::new(PasswordAuthProvider::new(
                users.clone(),
                password_hasher.clone(),
            ))],
            redirect_providers,
        )
        .expect("static provider ids are valid and unique"),
    );
    // Shared cookie attributes (the `Secure` flag) for the session and
    // redirect-state cookies, from server.cookie_secure.
    let cookies = web::Data::new(auth::CookieSettings {
        secure: config.server.cookie_secure,
    });

    println!("minerva-server listening on 0.0.0.0:{port}");

    HttpServer::new(move || {
        let mut app = App::new()
            .app_data(goals.clone())
            .app_data(milestones.clone())
            .app_data(goal_milestones.clone())
            .app_data(tasks.clone())
            .app_data(web::Data::from(task_counts.clone()))
            .app_data(task_relations.clone())
            .app_data(progress_snapshots.clone())
            .app_data(sso_group_rules.clone())
            .app_data(users_data.clone())
            .app_data(user_identities_data.clone())
            .app_data(sessions_data.clone())
            .app_data(session_service.clone())
            .app_data(rate_limit_service.clone())
            .app_data(rate_limit_config.clone())
            .app_data(user_admin.clone())
            .app_data(status_override_service.clone())
            .app_data(goal_milestone_link_service.clone())
            .app_data(task_relation_service.clone())
            .app_data(task_status_service.clone())
            .app_data(task_service.clone())
            .app_data(milestone_service.clone())
            .app_data(goal_service.clone())
            .app_data(assignment_service.clone())
            .app_data(task_list_service.clone())
            .app_data(goal_list_service.clone())
            .app_data(milestone_list_service.clone())
            .app_data(status_snapshots_data.clone())
            .app_data(account_link_service.clone())
            .app_data(auth_providers.clone())
            .app_data(cookies.clone());
        // Registered only when at least one redirect provider exists; the
        // flow handlers take it as an `Option` extractor and treat its
        // absence as "no redirect login" (404 on the flow routes).
        if let Some(flow) = redirect_flow.clone() {
            app = app.app_data(flow);
        }
        app.configure(routes::configure)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
