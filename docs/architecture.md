# Architecture

Minerva's backend is a Rust Cargo workspace (`server/`) organized around
domain-driven design in four crates. The frontend (`apps/web`, SvelteKit)
communicates with the backend over HTTP only.

## Layers

| Crate | Kind | Responsibility | May depend on |
|---|---|---|---|
| `domain` | lib | Core business concepts and invariants (goals, milestones, ...). Pure logic, no I/O. | nothing but the `uuid`, `chrono`, `serde` value-type exceptions |
| `application` | lib | Use cases: orchestrate domain objects to fulfill a request. Transaction boundaries live here. | `domain` |
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
| `ViewAccess` | `ViewContent` | the GET goal/milestone/task routes |
| `EditAccess` | `EditContent` | the POST/PUT/DELETE goal/milestone/task routes |
| `AdminAccess` | `ManageUsers` | the `/api/users` routes, the invite and password-reset routes (`/api/invites`, `/api/users/{id}/password-reset`) and the temporary `/debug/*` routes (until 3.4/3.6 replace them) |

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

## API documentation

The `interface` crate generates an OpenAPI 3 document from code annotations
(utoipa) and serves it alongside a Swagger UI, both outside the `/api` scope:

- Swagger UI: http://localhost:8080/api-docs/swagger-ui/
- Raw OpenAPI JSON: http://localhost:8080/api-docs/openapi.json

Under Docker compose the API is mapped to host port 3010, so use
http://localhost:3010/api-docs/swagger-ui/ there. All `/api/*` endpoints
(auth, goals, milestones, tasks, users) are documented; the temporary
`/debug/*` routes are not. Protected endpoints declare the `session_cookie` security
scheme (the session cookie as an API key, registered by a `utoipa::Modify`
addon in `interface/src/openapi.rs`) so Swagger UI's Authorize button can
fill it in; a document test keeps every operation's security requirement and
401/403 responses in sync with the access rules (see "Authorization").
