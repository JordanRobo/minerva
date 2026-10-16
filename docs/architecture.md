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

## Same-origin deployment

The browser sees exactly one public origin (roadmap 6.1, D7). A reverse
proxy in front of the two services routes `/api/*` and `/api-docs/*` to the
API and everything else to the web app, so the session cookie is
same-origin and no CORS is needed. The dev compose stack does this with a
minimal Caddy (`deploy/Caddyfile` — a small slice of 7.4 pulled forward);
the production reference config starts from that file, and `bun run dev`
replaces the proxy with a Vite proxy of the same two paths.

Server-side web code (hooks and load functions) still talks to the API, but
over the internal service URL, not the public origin: `apiFetch`
(`apps/web/src/lib/server/api.ts`) calls that URL and forwards only the
incoming request's `Cookie` header — SvelteKit's automatic cookie forwarding
does not apply to a foreign URL — and its `Set-Cookie` is never copied back
to the browser; sign-in and sign-out are the browser's job, calling
`/api/*` through the proxy. `ORIGIN` tells the adapter-node server its
public origin (without it SvelteKit rejects form-action POSTs as cross-site),
and `server.web_base_url` is that same public origin, so absolute URLs
(OIDC redirect, invite links) point at the proxy, not the API.
`rate_limit.client_ip_header` is set to the proxy's `X-Forwarded-For`
header, which the proxy sets itself, so rate limiting keys on the client's
address rather than the proxy's.

Because the proxy owns `/api/*` and `/api-docs/*` in both the dev server
and the deployment, the web app must never define routes under those paths.

## Web app structure

The web app (roadmap 6.3a/6.3b/6.4) is a SvelteKit 2 / Svelte 5 (runes)
application built with adapter-node. Signed-in routes live in the `(app)`
route group; signed-out screens live in the `(auth)` route group
(invisible in URLs): `/login`, `/accept-invite` and `/reset-password`
(6.4, "Auth screens" below).

- **Auth guard** — `(app)/+layout.server.ts` reads `url` (so the load
  re-runs on every client navigation), resolves `locals.currentUser()`
  (6.1) and, when there is no user, 303s to `/login?next=<path+query>`.
  A 502 from an unreachable API propagates instead of looking signed out.
  `(app)/+page.server.ts` 307s `/` to `/goals`.
- **Shell** — `(app)/+layout.svelte` composes small components from
  `apps/web/src/lib/components/shell/` (Sidebar, TopBar, TabBar, UserSheet,
  Wordmark, NavLink) and a no-dependency `Icon` component keyed by name
  (`$lib/components/icons/`). One breakpoint at 900px: at and above it a
  two-column grid with a fixed sidebar (264px, collapsible to 72px in
  memory only — a reload starts expanded) and only the content column
  scrolling; below it a 56px top bar and a 64px (+ safe-area) bottom tab
  bar. The desktop user menu is a Bits UI Popover — a disclosure, not a
  menu (`trapFocus={false}`, focus returns to the trigger on close) — and
  the mobile one is a Bits UI Dialog bottom sheet (6.6, D18). Collapsed
  sidebar nav links and the collapse toggle carry Bits UI Tooltips; one
  `Tooltip.Provider` wraps this layout for all of them, and the shared
  floating chrome (dialog overlay/card, tooltip bubble) is styled from
  tokens in `src/lib/styles/dialogs.css`. Sign out posts `/api/auth/logout`, then
  `goto('/login', { invalidateAll: true })`; a failed sign-out stays put
  with an inline error.
- **Navigation** — `apps/web/src/lib/nav.ts` owns the primary and admin
  item lists (href, label, icon), `roleLabel()` and `isActive()` (exact
  match or `href + '/'` prefix, so `/my-tasks` never activates Tasks). The
  Admin group is rendered via `{#if}` only for admins — it is absent from
  the DOM for other roles.
- **Components** — shared components in `apps/web/src/lib/components/`:
  `ui/` (Button, PageHeader, Banner, Skeleton, PageSkeleton), `states/`
  (StateCard, EmptyState, ErrorState, AccessDenied, RouteError) and
  `shell/` (the 6.3a shell components plus BareLayout — a centred card and
  the 28px wordmark, no navigation; the `(auth)` layout's frame). `Button`
  wraps Bits UI Button (6.6, D18) — it renders an `<a>` when given an
  `href`, else a `<button type="button">`; a disabled link is
  `aria-disabled`, out of the tab order, and does not navigate. `PageHeader`
  owns a page's one `<h1>` and its document title; the state components
  render beneath it as `h2`, while route errors render their card at `h1`.
  Components never check roles themselves: pages pass an action only when
  `can(role, action)` from `src/lib/permissions.ts` allows it. That helper
  mirrors the server's `Role::allows` matrix and is **cosmetic only** —
  the server enforces every permission, so the UI only hides controls a
  role could not use.
- **Pages** — every nav target has a `PageHeader` + `EmptyState`
  placeholder (no actions). Unknown paths in the signed-in area 404 via
  the `(app)/[...unmatched]` catchall, so the error renders inside the
  shell rather than falling out to the bare frame. A dev-only component
  gallery at `/dev/components` (a 404 outside dev, absent from production
  builds) shows every shared component for visual checking.
  `(app)/admin/+layout.server.ts` 403s non-admins before any admin page
  renders.
- **Error pages** — both error boundaries render the shared
  `states/RouteError`, which reads `page.status` and `page.error` from
  `$app/state`: 403 as AccessDenied (the page's own message), 404 with a
  Back home link and no retry, 502 as "We couldn't reach the server", and
  anything else a generic message — the latter two with a retry. In the
  shell a retry `invalidateAll()`s; at the root it is a full reload,
  because the page never loaded. `(app)/+error.svelte` renders inside the
  shell and catches child-route and child-layout errors (the admin 403,
  the catchall 404). The root `+error.svelte` renders inside `BareLayout`:
  the `(app)` layout's own failure (API down → guard 502) falls through to
  the root boundary, which is also why the root frame must not depend on
  the user the failed load was supposed to fetch.

### Auth screens

Roadmap 6.4 adds the three signed-out screens. They live in the `(auth)`
group under the shell-less `BareLayout` and share `src/lib/styles/auth.css`
(a centred bare card, tokens only). None of them is a SvelteKit form
action: every auth request is a same-origin `fetch` from the browser to
`/api/auth/*` through `postJson` in `src/lib/api.ts`. That is deliberate —
the 2.8 rate limiter keys login and token-link routes on the **client IP**,
and a request relayed through the web container would pool every user
behind one shared address; it also means the web server never reads or
forwards the session `Set-Cookie` the API sets. `postJson` returns one
result type (`{ ok, status, code, message, retryAfter? }`), parsing the
`{"error":{"code","message"}}` envelope and `Retry-After`; a network
failure or a non-JSON body becomes `code: "network"` and it never throws.

- **Sign-in** — `(auth)/login/+page.server.ts` 303s a signed-in visitor to
  a validated `next` and otherwise reads `GET /api/auth/providers`, the
  validated `next` and the `error`/`notice` query values (notice accepts
  only `password_reset`). The email/password form renders **only** when a
  `credentials`-kind provider exists; each `redirect`-kind provider becomes
  one full-width "Sign in with …" button carrying `data-sveltekit-reload`
  (a `/api/*` URL is not a SvelteKit route) with the encoded `next` appended
  to its `login_url`; the "or" divider shows only when both kinds exist, and
  no providers at all renders a "Sign-in isn't set up yet" line. On
  submit the page runs cheap client checks (email present + `@`, password
  present), then POSTs `/api/auth/login`. Exactly one message is shown per
  failed attempt — 401 "That email and password don't match.", 429 a
  "Too many attempts. Please wait N minutes…" from `Retry-After`, network
  "We couldn't reach the server…", everything else generic — never which
  part of the server check failed; after a failure focus moves to the
  `FormAlert`.
- **Redirect safety** — `src/lib/safe-redirect.ts` `safeNext(raw, fallback)`
  is the one validator for any `next`/redirect value: a single leading `/`,
  never `//`, no backslash, no scheme, no control/whitespace character, at
  most 512 chars, and never one of the other auth screens — anything else
  returns the fallback. Both the `/login` load and the post-login `goto` go
  through it.
- **Accept-invite / reset-password** — `?token` pages. Their server loads
  set `Referrer-Policy: no-referrer` and `Cache-Control: no-store` so the
  one-time token never leaks into a referrer or is cached. On mount each
  POSTs `/api/auth/tokens/inspect` behind a pulsing skeleton (motion-aware);
  a missing, invalid, expired or wrong-purpose token renders "This link
  isn't valid any more" directly in the card (not a nested `StateCard`) with
  a "Go to sign in" ghost, while a network/429 failure gets a retryable
  state. A valid invite shows the role, the read-only email, a visually
  hidden `autocomplete="username"` anchor (so password managers fill the
  right field without a visible username box), an optional "Your name"
  (blank → the email's local part, matching the server's `Option<String>`
  display name), and New/Confirm password against the real 8-character
  minimum. Accepting returns 201 and signs the user in straight to `/`; a
  reset returns 204 and lands on `/login?notice=password_reset`, because the
  API revokes **every** session for that user (the previous one is dead).
- **Error wording** — `src/lib/auth-errors.ts` maps SSO failure codes to
  plain-language messages (the raw code is shown as small muted
  "Reference: …" text, never HTML) and shapes the generic post failure
  messages. There is no public signup screen (D1); "Forgot your password?"
  on `/login` is a plain-text pointer to an administrator, since
  self-service reset emails only arrive with 7.3 email delivery.
- **Form components** — `ui/Input.svelte`, `ui/PasswordInput.svelte`
  (Input plus a Bits UI Toggle show/hide with `aria-pressed` and a "Show
  password"/"Hide password" label), `ui/Field.svelte` (Bits UI Label +
  hint/error, wiring `aria-describedby`/`aria-invalid`), and `ui/FormAlert.svelte`
  (`role="alert"`, `tabindex="-1"` so a failed submit can move focus
  there). There is no error colour token, so an error is signalled by the
  alert icon plus a 3 px accent left border and the field's accent outline —
  never colour alone.

Styling uses only the `--minerva-*` design tokens (6.2); the shell layout
tokens are the `--minerva-shell-*` set.

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
| `ViewAccess` | `ViewContent` | the GET goal/milestone/task routes, plus `GET /api/goals/{id}/milestones`, `GET /api/milestones/{id}/goals`, `GET /api/tasks/{id}/relations` and `GET /api/people` |
| `EditAccess` | `EditContent` | the POST/PUT/DELETE goal/milestone/task routes, plus `PUT`/`DELETE /api/goals/{id}/status-override` and the milestone equivalent, plus `PUT`/`DELETE /api/goals/{goal_id}/milestones/{milestone_id}`, plus `POST /api/tasks/{id}/relations` and `DELETE /api/tasks/{id}/relations/{relation_id}`, plus `PATCH /api/tasks/{id}/status`, plus `PUT`/`DELETE /api/tasks/{id}/assignee`, `PUT`/`DELETE /api/goals/{id}/owner` and `PUT`/`DELETE /api/milestones/{id}/owner` |
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
- The listings are paginated (D16) and return full goals/milestones in the
  shared default order: target date (nulls last), then creation time, then id.

The routes are `PUT`/`DELETE /api/goals/{goal_id}/milestones/{milestone_id}` —
Staff or Admin (`EditAccess`), both answering 204 with no body — and
`GET /api/goals/{id}/milestones` plus `GET /api/milestones/{id}/goals`, open to
any signed-in role (`ViewAccess`), taking only `limit` and `offset` and
answering 200 with the D16 envelope whose items are the standard goal/milestone
responses (effective `status` plus `status_source` included) — see "List
pagination, filtering and ordering". Unknown ids are 404s with distinct `goal_not_found`/
`milestone_not_found` codes, so a client can tell which of the two ids it sent
was missing. Linking does not fire the snapshot hook: status computation is
3.13's and snapshots 3.14's (D5).

### Task relations

Tasks are related through the `task_relations` table (roadmap 3.6, D4) with
**one canonical row per relationship**. The API accepts three relation types
from the path task's perspective — `blocks`, `blocked_by` and `relates_to` —
but storage keeps a single form: a submitted `blocked_by` is normalised on
write to a `blocks` row with the endpoints swapped, and a `relates_to` stores
the lower task id as the source. Both views are then derived per task when
reading (`TaskRelation::as_seen_by`), so there is exactly one row no matter
which side created the relation or which type it used.

The guarantees live in the database, not in application checks that could
race: migration 20261007000001 adds a `CHECK (source_task_id <>
target_task_id)` self-relation constraint and unique indexes on the unordered
pair — `(LEAST(source, target), GREATEST(source, target))` — for `blocks` and
for `relates_to`, so duplicate pairs and direct reverse loops (two tasks
blocking each other) are impossible at the database level with no race
window. The same migration first deletes offending rows on existing
databases: self-relations are removed, and duplicate or reversed pairs keep
only the oldest row (earliest `created_at`, then lowest id). Pre-existing
`blocked_by` rows are left as-is — they violate none of the new constraints,
and application writes no longer produce them.

The rules live in `application::task_relations::TaskRelationService`, not in
the handlers:

- Creating a relation checks that both tasks exist (a missing path task and a
  missing related task are distinct 404s), rejects a task relating to itself,
  and lets the unique indexes reject duplicates and reverse loops — surfaced
  as typed `RelationExists`/`ReverseRelationExists` errors.
- Deleting requires the relation to involve the task named in the path; a
  relation that belongs to other tasks answers the same 404, so its
  existence is not leaked.
- Listing returns every relation the task participates in, from its
  perspective, each with a summary of the other task (id, title, current
  status), ordered by creation time then id.

The routes are `POST /api/tasks/{id}/relations` and
`DELETE /api/tasks/{id}/relations/{relation_id}` — Staff or Admin
(`EditAccess`), answering 201 with the new relation and 204 — plus
`GET /api/tasks/{id}/relations`, open to any signed-in role (`ViewAccess`),
answering 200 with an array of `{id, relation_type, related_task: {id, title,
status}, created_at}`. A self-relation is a 400 (`self_relation`); a
duplicate or the opposite blocking relationship is a 409
(`relation_exists`/`reverse_relation_exists`); unknown tasks are 404s with
the generic `not_found` code, the message naming which of the two ids was
missing; a missing or unrelated relation on delete is a 404
`relation_not_found`. Longer cycle detection (A blocks B, B blocks C, C
blocks A) is deliberately out of v1.

### Task board transitions

The drag-and-drop task board moves cards between columns through
`PATCH /api/tasks/{id}/status` (roadmap 3.7, D6). The move is a single-column
write: `TaskRepository::set_status` issues one `UPDATE … RETURNING` that sets
only the status column and `updated_at`, so a concurrent edit of any other
field through `PUT /api/tasks/{id}` cannot be clobbered — there is no
load-modify-save round trip. An unknown id matches no row and surfaces as the
typed not-found error.

The rules live in `application::task_status::TaskStatusService`:

- Any column may move to any other, including Done back to Backlog — v1 has
  no workflow restrictions (D6).
- A blocked task is never refused; flagging a move past unfinished blockers
  is the UI's call (D6), not the API's.
- Setting the column a task already has is an idempotent no-op: the service
  answers with the stored task without writing, so `updated_at` is untouched.

A board move does not fire the snapshot trigger and does not touch goal or
milestone status computation (D5): snapshots are 3.14's and computed status
3.13's. The route is Staff or Admin (`EditAccess`) and answers 200 with the
updated task in the standard task response shape; an unknown task is a 404,
and an unknown status string a 400 from JSON parsing before the handler runs.

### List pagination, filtering and ordering

The five list endpoints — `GET /api/tasks`, `GET /api/goals`,
`GET /api/milestones` and the two goal↔milestone link lists (roadmap 3.10,
D16) — answer a shared envelope instead of a bare array:

```json
{ "items": [ … ], "total": 42, "limit": 50, "offset": 0 }
```

`total` counts every row matching the filters, ignoring paging. Paging is
limit/offset: `limit` accepts 1–200 and defaults to 50, `offset` must be ≥ 0
and defaults to 0. A malformed or out-of-range value is a 400 `invalid_query`
naming the parameter — values are never clamped — and an offset past the end
of the result is simply an empty page with the correct total. Every list goes
through a small application service (`application::task_list`, `goal_list`,
`milestone_list`, or the goal–milestone link service) so the handlers never
touch a repository.

Each endpoint's filters:

- Tasks: `status` (comma-separated and/or repeated), `milestone_id`,
  `assignee_id` (a UUID, roadmap 3.11), `q` (case-insensitive literal
  substring on the title, trimmed, at most 100 characters; `%`, `_` and `\`
  match literally) and inclusive `target_after`/`target_before`
  (`YYYY-MM-DD`) that never match undated tasks.
- Goals and milestones: the same minus `milestone_id`, with one difference —
  `status` matches the **effective** status (the manual override when set, else
  the automatic value; roadmap 3.2), which in Postgres is
  `COALESCE(status_override, status)`.
- The link lists take only `limit` and `offset`: the path id is the filter.

No filter can 404: an unmatched filter — including an unknown `milestone_id`
or `assignee_id` — is a 200 empty page. (An unknown *path* id on the link
lists still 404s.)

Ordering is one shared default for every list, not client-selectable (roadmap
3.16): target date ascending with undated rows last, then `created_at`, then
id. With a total count and a stable order, pages are deterministic — walking
the pages reassembles the full result exactly once. There is no `sort`
parameter in v1; manual card ordering would be the reason to add one.

Migration 20261008000001 adds `idx_tasks_status` and
`idx_tasks_list_order (target_date NULLS LAST, created_at, id)` for the task
list; migration 20261013000001 adds `idx_tasks_assignee_id` for the assignee
filter (roadmap 3.11); goals and milestones have no indexes yet — school-scale
row counts do not justify one, and the effective-status filter is an expression
over two columns that a plain b-tree cannot serve.

Deliberately **not** paginated: `GET /api/users`, `GET /api/invites`,
`GET /api/sso/group-rules`, `GET /api/people` and
`GET /api/tasks/{id}/relations` — each is small and bounded, so a bare array
stays the simpler contract.

### Assignees and owners

Tasks have one optional **assignee**; goals and milestones each have one
optional **owner** (roadmap 3.11, D17). Storage is a nullable user reference
on each table (`tasks.assignee_id`, `goals.owner_id`, `milestones.owner_id`,
migration 20261013000001) with `ON DELETE SET NULL`: deleting a user leaves
their assignments in place as unassigned rather than failing or cascading.

A person is rendered everywhere as a **person summary** — `{id, display_name,
active}`, never the email (D17). Task responses embed it under `assignee`,
goal and milestone responses under `owner` (`null` while unset); a deactivated
person still renders, flagged by `active: false`. Summaries are filled in
batch: one `UserRepository::summaries_by_ids` call per response or page
(`resolve_summaries` in `interface/src/people.rs`), not one query per row.

The dedicated endpoints are Staff or Admin (`EditAccess`) and answer 200 with
the updated entity in its standard shape:

- `PUT /api/tasks/{id}/assignee` (body `{assignee_id}`) and
  `DELETE /api/tasks/{id}/assignee`,
- `PUT /api/goals/{id}/owner` and `PUT /api/milestones/{id}/owner` (body
  `{owner_id}`), each with a `DELETE` equivalent.

Creating a task, goal or milestone accepts the same optional id and validates
it with the same check. The rules live in
`application::assignment::AssignmentService`, not in the handlers:

- The target entity is checked before the user: an unknown task, goal or
  milestone is a 404 even when the user id is also unknown; an unknown user is
  a 404 `user_not_found`, a deactivated one a 409 `user_deactivated`.
- Setting the value already set, or clearing one that is not set, is an
  idempotent no-op that returns the entity without writing, so `updated_at` is
  untouched.
- Clearing never looks the user up: it works even when the current assignee
  or owner has since been deactivated.

The writes are single-column — `TaskRepository::set_assignee` and the
goal/milestone `set_owner` ports issue one `UPDATE … RETURNING` of only the
reference and `updated_at`, so a concurrent edit of any other field cannot be
clobbered; an ordinary update never changes or clears the reference (the
repository's `update` omits the column). Like board moves, no snapshot trigger
fires and nothing is audited (D5, D9).

`GET /api/people` (any signed-in role, `ViewAccess`) lists every **active**
user as a person summary, ordered by display name then id — the data for the
UI's pickers. Deactivated accounts are excluded there but still render on the
entities they were assigned to. It sits beside the admin-only
`GET /api/users` deliberately: the picker needs names for every signed-in
role, while `/api/users` is the management surface (full user rows, role and
active-state changes) and exposes no person summary at all.

The task list filters on `assignee_id` (above); there is deliberately no owner
filter and no "my tasks"/"unassigned" shortcut in v1 — roadmap 4.5 builds
"My tasks" on `assignee_id` plus the caller's own id.

## API documentation

The `interface` crate generates an OpenAPI 3 document from code annotations
(utoipa) and serves it alongside a Swagger UI, both outside the `/api` scope:

- Swagger UI: http://localhost:8080/api-docs/swagger-ui/
- Raw OpenAPI JSON: http://localhost:8080/api-docs/openapi.json

Under Docker compose the API is mapped to host port 3010, so use
http://localhost:3010/api-docs/swagger-ui/ there. All `/api/*` endpoints
(auth, goals, goal–milestone links, milestones, tasks, task relations, people,
users, invites and SSO group rules) are documented; the temporary
`/debug/*` routes are not. Protected endpoints declare the `session_cookie` security
scheme (the session cookie as an API key, registered by a `utoipa::Modify`
addon in `interface/src/openapi.rs`) so Swagger UI's Authorize button can
fill it in; a document test keeps every operation's security requirement and
401/403 responses in sync with the access rules (see "Authorization"). The
paginated list endpoints additionally document their paging and filter
parameters and their page schemas (`TaskPage`, `GoalPage`, `MilestonePage`);
see "List pagination, filtering and ordering".
