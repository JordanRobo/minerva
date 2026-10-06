# Architecture

Minerva's backend is a Rust Cargo workspace (`server/`) organized around
domain-driven design in four crates. The frontend (`apps/web`, SvelteKit)
communicates with the backend over HTTP only.

## Layers

| Crate | Kind | Responsibility | May depend on |
|---|---|---|---|
| `domain` | lib | Core business concepts and invariants (goals, milestones, ...). Pure logic, no I/O. | nothing but the `uuid`, `chrono`, `serde` value-type exceptions |
| `application` | lib | Use cases: orchestrate domain objects to fulfill a request. Transaction boundaries live here. | `domain`, plus `sha2` for one-way subject hashing in rate limiting |
| `infrastructure` | lib | Adapters for external systems: Postgres (Diesel), Redis; S3 (`object_store`) declared but not yet implemented (roadmap 7.1). Receives ready-made configuration values; reads no files or environment itself. | `domain`, `application` (ports) |
| `interface` | bin | HTTP API server (Actix-web). Routing and request/response mapping; the composition root that wires the other crates together at startup. Owns all configuration: typed, validated settings from an optional `minerva.toml` layered with environment variables (`src/config.rs`). | all of the above |

## Dependency direction

```
            interface ──────────────> application ──────> domain
                 │                          ^                ^
                 └────────> infrastructure ─┴────────────────┘
                    (implements ports/traits declared by inner layers)
```

Rules:

- Dependencies point inward, toward `domain`. `domain` itself depends on
  nothing but the three value-type crates (`uuid`, `chrono`, `serde`) — no
  I/O, no frameworks.
- Only `interface` may depend on Actix-web; HTTP types never leak into the
  other crates.
- `infrastructure` implements the storage/cache traits (ports) declared by
  the inner layers and is swapped in at startup from the composition root in
  `interface`.
- Redis is optional at runtime; S3 support lands with roadmap 7.1. All
  configuration is loaded by the `interface` crate's config module (defaults,
  then an optional `minerva.toml`, then the `DATABASE_URL`/`REDIS_URL`/`PORT`
  aliases, then `MINERVA_`-prefixed environment variables), with local-dev
  defaults provided in `deploy/docker-compose.yml` — never hardcoded in Rust
  source. The future `S3_*` settings will arrive through the same layering.

## Database migrations

Diesel migrations live in `server/migrations/` and are embedded in the
binary; the server applies any pending ones at startup (skip with
`server.run_migrations = false` in `minerva.toml`, or
`MINERVA_SERVER__RUN_MIGRATIONS=false`, e.g. when a dedicated migration job
owns the schema).
Concurrent startups are safe — application is serialized on a Postgres
advisory lock. Generate a new migration using the diesel CLI:

```sh
cd server
diesel migration generate add_first_table
```

## Sessions and authentication

Every sign-in method is an *auth provider* registered at startup in the
`AuthProviders` registry (`application/src/auth/provider.rs`). There are two
kinds:

- **Credential providers** verify credentials submitted to a login form.
  Today: `password` (Argon2id email/password).
- **Redirect providers** send the browser to an external identity provider
  and back through a callback route. Today: `oidc`, registered only when
  `oidc.issuer_url` is set — see `minerva.example.toml` for all the settings.

Any successful sign-in, from any provider, ends in the same cookie session
(`minerva_session`) issued by the `SessionService`; the server stores only
the SHA-256 hash of the token, never the raw value. Sessions sit behind the
`SessionRepository` port: Redis when a Redis URL is configured (`redis.url`,
via `REDIS_URL` or `minerva.toml`), Postgres otherwise.

The password provider **equalises login timing**: every attempt runs exactly
one Argon2id verification before any rejection is evaluated — against the
account's stored hash when one exists, otherwise against a process-wide dummy
hash that the adapter generates once with the same parameters as real hashes
(`PasswordHasher::verify_dummy`) — and only afterwards are the deactivated and
passwordless conditions checked. Unknown email, deactivated account,
passwordless account and wrong password therefore all cost one verification
and return the identical 401 body, so response time does not reveal which
accounts exist. This is a side-channel mitigation only: it throttles nothing
(login rate limiting is a separate 2.8 item) and covers the credential path —
the SSO redirect flow has no submitted secret to verify.

Sessions use **sliding expiry**: `SessionService::resolve` — which every
authenticated request runs through — slides a session's `expires_at` forward
by a full TTL (`DEFAULT_SESSION_TTL`, 30 days) whenever at least
`SESSION_TOUCH_INTERVAL` (5 minutes) has passed since the stored
`last_seen_at`. The stored timestamp is the only throttle state, so the rule
needs no in-memory bookkeeping and behaves identically across any number of
API nodes. The touch is conditional on the session still being live — a
revoked or expired one is a `NotFound`, never resurrected — and a failed
touch is logged at warn without failing the request: the session simply
keeps its current expiry. The `minerva_session` cookie carries a fixed, long
`Max-Age` (365 days) rather than the session's issue-time expiry: with
sliding expiry the server-side `expires_at` is the authority on validity,
and a cookie that outlives its session is harmless — the token simply stops
resolving.

Every Redis operation that writes more than one key issues its writes as a
single MULTI/EXEC pipeline: `create` writes the session hash, its id mapping
and the per-user index entry (with both TTLs) in one shot, and `delete`,
`delete_all_for_user` and the write phase of `touch_last_seen` do the same
for their removals and TTL refreshes — a crash mid-write cannot leave a
session without its id mapping or index entry. In Redis, expiry itself needs
no maintenance: the keys' native TTLs evict expired sessions, so
`purge_expired` there is a no-op.

When Postgres holds the sessions, expired rows are removed by an hourly
maintenance job in `interface` (`src/maintenance.rs`), started only on that
branch of startup. Each tick first takes a session-level Postgres advisory
lock with a try-lock — when another node holds it the tick skips silently, so
however many API nodes run, exactly one purges per tick; the first tick lands
about 30 seconds after startup. While holding the lock it calls
`SessionRepository::purge_expired` and logs the removed count only when it is
non-zero; a failing tick warns and retries on the next hour. The rate-limit
counter purge is registered as a second `MaintenanceJob` (name + advisory
lock key + async fn) on the same Postgres branch, with its own lock key so
the two jobs tick independently.

The redirect flow (`interface/src/redirect.rs`) is generic over providers:

- `GET /api/auth/{provider}/login?next=` asks the provider for its
  authorization URL, stores the provider's pending state (plus the validated
  `next` destination) in an encrypted, HttpOnly state cookie scoped to that
  provider's path, and sends the browser off.
- `GET /api/auth/{provider}/callback` verifies the state cookie (signature,
  expiry, and that it was issued for *this* provider), hands every callback
  query parameter to the provider's `complete`, and on success issues the
  session cookie and redirects to `next`. Every failure redirects to the
  login page with a stable error code; the detail goes to the log only.

The OIDC protocol itself sits behind the `OidcProvider` port
(`OpenIdConnectProvider` implements it); `OidcAuthProvider` wraps that port
in the redirect-provider contract, including user lookup/linking decisions
and identity creation.

**Adding a provider:** implement `CredentialProvider` or
`RedirectProvider` in `application`, add an instance to the registry
construction in `interface/src/main.rs`. Nothing else changes — routing,
cookies, session issuance, and the `/api/auth/providers` listing all follow
from the registry.

**Rate limiting** (roadmap 2.8) caps the endpoints that accept secrets or
issue links, with fixed windows aligned to the epoch, so every node computes
the same boundaries from its own clock:

| Policy | Limit | Window | Bucket | Guards |
|---|---|---|---|---|
| `login_ip` | 30 | 15 min | client IP | `POST /api/auth/login` |
| `login_ip_email` | 10 | 15 min | client IP + normalised email | `POST /api/auth/login` |
| `token_link_ip` | 30 | 15 min | client IP | the three public token-link routes |
| `admin_issue_actor` | 60 | 1 hour | acting user's id | the invite and password-reset issue endpoints |

The handlers only build the subject string and call
`RateLimitService::hit`; a limited request answers 429 with the standard
error envelope plus a `Retry-After` header in whole seconds. The login checks
run before any credential work, so a limited attempt never reaches Argon2;
the admin checks run after the Admin extractor, so 401/403 still come first.
Every other route — including the SSO redirect flow — is unlimited.

Counters sit behind the `RateLimiter` port (`application::rate_limit`) with
the same storage selection as sessions: Redis when `redis.url` is configured
(a hit is one atomic INCR+EXPIRE pipeline; native TTLs evict expired
windows), Postgres otherwise (one row per key and window, incremented by a
single atomic upsert). The service hashes subjects with SHA-256 before they
reach the store — raw IPs and emails never touch it — and fails open: a
store error is logged at warn and the request allowed, because rate limiting
must never take login down. `rate_limit.client_ip_header` names the proxy
header carrying the real client address (the first comma-separated value
wins), falling back to the TCP peer address; with Postgres counters, the
hourly maintenance runner purges windows older than the longest policy
window.

What this does **not** cover: a distributed brute force against one account
from many IPs stays under every per-IP bucket (the per-pair bucket slows it,
but an attacker with enough IPs still gets 10 tries per 15 minutes per IP).
Account lockout after repeated failures is deliberately out of scope — the
per-IP caps plus Argon2's cost are the v1 answer.

### Invites and password reset

Both flows are one mechanism (roadmap 2.6): a single `account_tokens` table
holds either an invite (an email address plus the role the new account gets)
or a password reset (the user whose password changes). Only the SHA-256 hash
of the token is stored — the raw value appears once, in the link. Tokens are
single-use and expiring (7 days for invites, 24 hours for resets), and at most
one live token may exist per subject: partial unique indexes on `(email)` and
`(user_id)` make re-issuing replace the old token even under concurrent
issuers, and consumption is an atomic claim, so a link opened by two people at
once works for exactly one of them.

The rules live in `application::account_links::AccountLinkService`, not in
the handlers. The HTTP side is admin-only — `POST /api/invites`,
`GET /api/invites`, `POST /api/invites/{id}/revoke`,
`POST /api/invites/{id}/reissue`, `POST /api/users/{id}/password-reset` — plus
three public routes the link itself calls: `POST /api/auth/tokens/inspect`,
`POST /api/auth/accept-invite` and `POST /api/auth/reset-password`. Accepting
an invite creates the user with the invited role and signs them in; resetting
a password revokes all of the account's sessions. Links are built from
`server.web_base_url` (`/accept-invite?token=…`, `/reset-password?token=…`)
and are site-relative paths when that setting is unset.

Email delivery goes through the `AccountEmailSender` port; today its only
implementation is the no-op one, so a "send" fails gracefully and the link is
returned to the caller instead — the flow works fully without SMTP (D3), and
a real sender arrives with roadmap 7.3. Password reset is admin-initiated in
v1: an admin issues the link for a named user. The self-service "forgot
password" endpoint lands with 7.3, because it needs 2.8's rate limiting.

One edge case is handled deliberately: if an invited email signs in via SSO
before accepting, the account is created with the invite's role and the
pending invite is consumed, so the link then fails (the token is already used).
Expired or revoked invites are ignored, a pending invite never enables SSO
signup while `oidc.auto_create_users` is off, and linking to an existing user
never touches invites.

## Authorization

Authentication answers "who is this?"; authorization answers "may they do
this?". The policy lives in two layers:

- `domain::Role` (`admin`, `staff`, `read_only`) and `domain::Permission`
  (`ViewContent`, `EditContent`, `ManageUsers`), with an exhaustive,
  wildcard-free `Role::allows` matrix that lists every (role, permission)
  pair.
- `application::authz::authorize(user, permission)` — the single place a
  role decision is made. Future checks (ownership rules) land here, so no
  handler or extractor compares roles itself. Deactivated accounts are not
  a role question and are handled where the user row is resolved (below).

HTTP-level enforcement is three request extractors in
`interface/src/access.rs`, each wrapping the `AuthenticatedUser` session
extractor and asking `authz` for its permission:

| Extractor | Permission | Used by |
|---|---|---|
| `ViewAccess` | `ViewContent` | the GET goal/milestone/task routes, plus `GET /api/goals/{id}/milestones` and `GET /api/milestones/{id}/goals` |
| `EditAccess` | `EditContent` | the POST/PUT/DELETE goal/milestone/task routes, plus `PUT`/`DELETE /api/goals/{id}/status-override` and the milestone equivalent, plus `PUT`/`DELETE /api/goals/{goal_id}/milestones/{milestone_id}` |
| `AdminAccess` | `ManageUsers` | the `/api/users` routes, the invite and password-reset routes (`/api/invites`, `/api/users/{id}/password-reset`) and the temporary `/debug/*` routes (until their roadmap items replace them) |

A missing or invalid session is a 401; a valid session whose role lacks the
permission is a 403 with the standard error envelope (`forbidden`). Handlers
declare their required level in their signature — no inline role checks.
`GET /api/auth/me` requires only a session (any role); login, logout,
providers, the redirect flow, the invite/reset token routes
(`/api/auth/tokens/inspect`, `/api/auth/accept-invite`,
`/api/auth/reset-password`), `/health` and the API docs stay public. The public list is an explicit allowlist in
`interface/src/access_tests.rs`: adding a route without an extractor or an
allowlist entry fails the tests.

Roles are stored on `users.role`; a change takes effect on the next request,
because every authenticated request re-resolves the user row.

### User administration

The admin-only Users API (`/api/users`, roadmap 2.4) lists users and changes
a user's role or active state. The rules live in
`application::user_admin::UserAdminService`, not in the handlers:

- An administrator cannot change their own role or deactivate themselves.
- The last **active** administrator can never be demoted or deactivated, so
  there is always someone who can manage users again. A deactivated
  administrator does not count as active.

The last-admin rule must hold under concurrent requests, so the write goes
through `UserRepository::apply_access_change`: a single Postgres transaction
guarded by a transaction-level advisory lock that re-checks the active-admin
count after acquiring it. Concurrent changes serialize on the lock instead of
interleaving between the count and the write.

Deactivation sets `users.deactivated_at`, revokes all of the user's sessions
(`SessionService::revoke_all_for_user`), and refuses new sign-ins — password
login fails as invalid credentials and OIDC completion rejects the account.
The per-request re-resolution above is the backstop: an already-issued session
stops working on its next request even if it races the revocation.

The very first admin comes from the first-admin bootstrap (roadmap 2.5): at
startup, only while the users table is empty, `application::bootstrap`
creates the configured admin — a password account, with the password
supplied via environment variable or `*_file`, never in a committed config.
The check-and-insert runs as one Postgres transaction under an advisory lock
(`UserRepository::create_if_no_users`), so racing starts cannot both create
an admin; once any user exists the bootstrap never runs again and can never
reset a password or modify an existing account. All other accounts are
created by SSO (while `oidc.auto_create_users` is set) and, later, by
invites (roadmap 2.6); afterwards, admins manage roles and active state
through this API.

### SSO group mapping

While at least one group-to-role rule exists, the role of every user signing
in via SSO is recomputed from the IdP's groups claim (roadmap 2.7, D15);
while none exists, roles and flags are never touched. The rules live in
`sso_group_role_rules` — one per group name, admin-managed through the
admin-only `/api/sso/group-rules` API — and the mapping itself is pure domain
logic: `domain::resolve_role` matches exact, case-sensitive trimmed group
names, keeps the **least permissive** of several matched roles, and falls
back to Read-only (`SSO_FALLBACK_ROLE`) when nothing matches — including a
missing or malformed groups claim.

The application service is `application::sso_roles::SsoRoleService`, called
by the OIDC provider after the user is resolved or created and after the
deactivated-account rejection:

- A new SSO account is created with the computed role and
  `role_managed_by_sso = true`; a pending invite for its email is still
  consumed, but its role is ignored while rules exist (honoured when they do
  not).
- An existing user's role moves to the computed one at every login — the IdP
  is the source of truth. Linking an existing account to an SSO identity
  follows the same rule.
- The first-admin bootstrap account is exempt via `sso_role_exempt` and is
  never recomputed.
- As a backstop for everyone, recomputation never demotes the last active
  administrator: the change is skipped (role and flag both untouched) and a
  warning names the account's email and the computed role.

The write goes through `UserRepository::apply_access_change` with
`AccessChange::RoleManagedBySso`, so the role and the flag change in the same
advisory-locked transaction as every other access change, under the same
last-admin guard. Roles are re-resolved per request (above), so a changed
role takes effect on the user's next request without any session revocation.

While any rule exists, a role SSO recomputed (`role_managed_by_sso = true`)
is locked against hand edits: `UserAdminService::change_role` answers 409
(`role_managed_by_sso`) and every user response reports the flag's effective
value (the flag AND any rule exists). Deleting the last rule frees such roles
for hand edits again; the stored flag then simply stops being enforced.

### Status overrides

Goals and milestones each carry a **manual status override** on top of their
automatic status (roadmap 3.2). Storage is a nullable `status_override` column
on both tables (migration 20261006000002, same four-value CHECK as the
`status` columns); the domain types expose it through `effective_status()` —
the override when set, otherwise the automatic status — and
`status_source()`, which is `ManualOverride` only while an override is active.

Overrides are **sticky**: a normal update never writes the column (the
repository's `update` omits it), and `apply_computed_status` — what 3.13's
recomputation will call — updates only the automatic status, so a
recomputation can neither change nor clear an override. The only write path is
`set_status_override(id, Option<Status>)`, where `None` clears it.

The endpoints are `PUT`/`DELETE /api/goals/{id}/status-override` and the
milestone equivalent — Staff or Admin (`EditAccess`); both verbs answer 200
with the updated entity, whose response reports the effective `status` plus a
`status_source` of `"automatic"` or `"manual"`. The rules (an unknown id is a
404; setting the value already set, or clearing when none is set, is an
idempotent no-op) live in
`application::status_override::StatusOverrideService`, not in the handlers.

After every successful set or clear the service calls the
`StatusSnapshotTrigger` port with a `StatusChangeTarget::{Goal, Milestone}`,
so 3.14 can attach real progress snapshots without touching the endpoints. The
hook is best-effort like email delivery: a failure logs a warning and the
override still succeeds. `NoopStatusSnapshotTrigger` is the current
implementation.

### Goal↔milestone links

Goals and milestones are related through the `goal_milestones` table (roadmap
3.4) — a plain many-to-many: a goal may link to many milestones and a
milestone to many goals, and deleting either end removes its links (the
foreign keys cascade). The endpoints needed no new migration; the table has
existed since M1 with its pair primary key.

The rules live in `application::goal_milestone_links::GoalMilestoneLinkService`,
not in the handlers:

- Linking and unlinking are **idempotent** — linking an already-linked pair or
  unlinking an unlinked one is a no-op (the insert is `ON CONFLICT DO NOTHING`,
  so racing duplicates cannot error) — and both still answer 204.
- Both ids must exist before either verb acts: the goal is checked first, then
  the milestone; an unknown id is a typed `GoalNotFound`/`MilestoneNotFound`.
- The listings return full goals/milestones in a deterministic order: target
  date (nulls last), then creation time, then id.

The routes are `PUT`/`DELETE /api/goals/{goal_id}/milestones/{milestone_id}` —
Staff or Admin (`EditAccess`), both answering 204 with no body — and
`GET /api/goals/{id}/milestones` plus `GET /api/milestones/{id}/goals`, open to
any signed-in role (`ViewAccess`), answering 200 with plain arrays of the
standard goal/milestone responses (effective `status` plus `status_source`
included). Unknown ids are 404s with distinct `goal_not_found`/
`milestone_not_found` codes, so a client can tell which of the two ids it sent
was missing. Linking does not fire the snapshot hook: status computation is
3.13's and snapshots 3.14's (D5).

## API documentation

The `interface` crate generates an OpenAPI 3 document from code annotations
(utoipa) and serves it alongside a Swagger UI, both outside the `/api` scope:

- Swagger UI: http://localhost:8080/api-docs/swagger-ui/
- Raw OpenAPI JSON: http://localhost:8080/api-docs/openapi.json

Under Docker compose the API is mapped to host port 3010, so use
http://localhost:3010/api-docs/swagger-ui/ there. All `/api/*` endpoints
(auth, goals, goal–milestone links, milestones, tasks, users, invites and SSO
group rules) are documented; the temporary
`/debug/*` routes are not. Protected endpoints declare the `session_cookie` security
scheme (the session cookie as an API key, registered by a `utoipa::Modify`
addon in `interface/src/openapi.rs`) so Swagger UI's Authorize button can
fill it in; a document test keeps every operation's security requirement and
401/403 responses in sync with the access rules (see "Authorization").
