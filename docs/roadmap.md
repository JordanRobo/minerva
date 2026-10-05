# Minerva Roadmap

Living checklist for the Minerva build. This is the source of truth for what is done, what is next, what is deferred, and what is out of scope. Update it whenever work lands or scope changes, and log every change in the [Change log](#change-log).

_Last updated: 2026-10-05_

---

## How to use this doc

**Status markers**

- `[x]` done and merged
- `[ ]` not started
- A `Partial:` note after an unchecked item means some of it exists; the note says what is left.

**Tags**

- `(orig #N)`: one of the 32 tasks in the original breakdown, numbered globally across milestones (so "Auth provider abstraction" is orig #7).
- `(new)`: added after the repo review on 2026-10-01.
- `(moved)`: moved from a different milestone; the note says where from.
- `(D#)`: shaped by an entry in the [Decisions log](#decisions-log).

**Rules for scope changes**

1. Anything not in this doc is not planned. If it comes up, add it here (or to Future releases / Out of scope) before any worker prompt is written for it.
2. Anything deferred must either name the milestone that will pick it up or live in [Future releases](#future-releases).
3. Each worker prompt should reference the roadmap item it completes; tick the box when it merges.

---

## Current position

| Milestone | State |
|---|---|
| M0 Foundations (new) | Done: 0.1 to 0.7 merged |
| M1 Core Domain & Persistence | Done |
| M2 Authentication & Authorization | In progress: basic auth, OIDC, the provider abstraction, #8 (roles & permissions), 2.5 (first-admin bootstrap), 2.6 (invite & reset links) and 2.7 (SSO group-to-role mapping) done; session hardening next |
| M3 API Layer | In progress: CRUD, errors and OpenAPI done; most non-CRUD endpoints outstanding |
| M4 Frontend: Goal & Task Management | Not started |
| M5 Frontend: Reporting Dashboard | Not started |
| M6 Frontend: Application Shell & UX | Not started |
| M7 Infrastructure & Deployment | Partial: Redis-backed sessions done |
| M8 v1 Release Polish | Not started |

**Next up:** 2.7 (SSO group-to-role mapping) is done; next is 2.8 (session & login hardening).

All planning decisions (D1 to D15) are resolved; see the [Decisions log](#decisions-log).

---

## Recommended execution order

Milestone numbers are kept stable for reference, but the work is not executed strictly in numeric order.

1. **M0** Foundations: migrations on startup, CI, auth characterization tests, config/docs cleanup, then the `minerva.toml` configuration system (0.7) so the auth work that follows lands on it.
2. **M2** finish: #7 provider abstraction, roles & permissions with route protection, first-admin bootstrap and closing open signup, invite/reset links, SSO group mapping, session hardening.
3. **M3** finish: non-CRUD endpoints, assignees/owners, comments, pagination, then status computation, snapshots and the dashboard API.
4. **M6** app shell, design system and auth UI. The board needs somewhere to live and login is required to use anything.
5. **M4** goal and task management UI.
6. **M5** reporting dashboard UI.
7. **M7** remaining infrastructure (storage, email delivery, production deployment).
8. **M8** release polish.

---

## Milestone 0: Foundations & Hygiene (new)

Goal: a fresh clone runs end to end, is tested in CI, and the docs match reality. Small items that would otherwise block or undermine later work.

- [x] **0.1 Run migrations on startup** (new): embed Diesel migrations in the server and apply them at boot (opt-out via config flag). A fresh `docker compose up` currently yields an empty schema.
- [x] **0.2 CI pipeline** (moved from M7, orig #28): build, test and lint on PRs for both `server` and `apps/web`; Postgres and Redis service containers so the skipped-without-env tests actually run.
- [x] **0.3 Auth characterization tests** (new): tests for signup/login/logout/me and the `AuthenticatedUser` extractor; a `PostgresSessionRepository` round-trip test (only Redis has one today). Written before #7 refactors this code.
- [x] **0.4 Config completeness** (new): OIDC and related variables documented in `.env.example` and passed through compose; bundled Authentik service intentionally not added (see 8.2). Superseded in part by 0.7, which moves server configuration into `minerva.toml`.
- [x] **0.5 Docs drift** (new): update `CLAUDE.md` (migrations exist, routes beyond `/health`, `domain` dependency note), `docs/architecture.md` (migrations, sessions on Postgres or Redis, OIDC, same-origin deployment) and the stale "Milestone 3" comment in `main.rs`.
- [x] **0.6 Contributor-friendly tooling config** (new): `.claude/settings.json` hardcodes `/home/jordan/.local/bin/graphify`; move to `settings.local.json` or make it optional.
- [x] **0.7 Configuration system** (new, D13): typed, validated server configuration read from an optional `minerva.toml`, layered with environment overrides.
  - [x] Precedence: built-in defaults, then `minerva.toml`, then environment variables. The file is optional; with no file the server runs from defaults plus required settings.
  - [x] File discovery: `--config <path>` or `MINERVA_CONFIG`, then `./minerva.toml`, then `/etc/minerva/minerva.toml`. An explicitly named file that is missing is an error.
  - [x] Typed sections with real arrays (for example `oidc.scopes`); unknown keys are rejected; all validation errors are reported together at startup
  - [x] Environment overrides use the `MINERVA_` prefix with `__` for nesting (for example `MINERVA_OIDC__ISSUER_URL`); `DATABASE_URL`, `REDIS_URL` and `PORT` stay as aliases
  - [x] Secrets (database URL credentials, OIDC client and state secrets, and later S3 keys and the bootstrap admin password) can come from an env var or a `*_file` path, are documented as not belonging in the TOML, and never appear in logs or `Debug` output
  - [x] Only bootstrap and infrastructure settings live in config; runtime-managed settings (for example the SSO group mapping, D8) stay in the database
  - [x] All existing environment reads move onto it (`main.rs`, OIDC config, `COOKIE_SECURE`, `RUN_MIGRATIONS`); `minerva.example.toml` committed, `minerva.toml` git-ignored, `.env.example` reduced to compose-level variables, compose and docs updated

---

## Milestone 1: Core Domain & Persistence (done)

Goal: the data model exists and can be read/written from Postgres.

- [x] **1.1 Task domain model** (orig #1): `Task`, extensible `TaskRelationType` (`#[non_exhaustive]`), link to Milestone
- [x] **1.2 Diesel schema & migrations** (orig #2): goals, milestones, goal_milestones, progress_snapshots, tasks, task_relations (plus users, sessions, user_identities under M2)
- [x] **1.3 Repository traits (ports)** (orig #3): in `application`
- [x] **1.4 Postgres repository implementations** (orig #4): Diesel-backed, with round-trip tests

Carry-over notes (tracked in later milestones, not gaps in M1):

- `Goal::compute_status_from_milestones` exists but is never called (see 3.13).
- The `TODO(application)` in `task_relation.rs` about `Blocks`/`BlockedBy` pairs is resolved by D4 (see 3.6).

---

## Milestone 2: Authentication & Authorization

Goal: users can sign in via either method; single-tenant, so no org/school switching.

- [x] **2.1 Basic auth** (orig #5): signup/login/logout/me, Argon2id, cookie sessions stored in Postgres or Redis. (Open signup was removed in 2.5.)
- [x] **2.2 OIDC/OAuth support** (orig #6): discovery, PKCE, nonce, link/create user policy, encrypted state cookie, handler tests with a fake provider. Partial: live validation against a real Authentik instance is pending 0.4.
- [x] **2.3 Auth provider abstraction** (orig #7): a common provider contract in `application` so password and OIDC (and future providers) sit behind it without touching session handling.
  - [x] Provider trait/contract lives in `application`; password and OIDC implement it
  - [x] `AuthenticatedUser` extractor depends on the `UserRepository` port, not `PostgresUserRepository`
  - [x] Orchestration (email normalisation, token hashing, session issuing) moves out of `interface` handlers into application services
  - [x] `/api/auth/providers` response driven by the registered providers
  - [x] Existing behaviour preserved (covered by 0.3)
  - Note: the pre-hijacking fix is no longer a blocker (D1, D2). Password accounts can only come from an invite, the configured bootstrap admin or SSO, so nobody can pre-register an address they do not control. Keep the existing rule that SSO linking requires `email_verified`.
- [x] **2.4 Roles & permissions** (orig #8): Admin / Staff / Read-only, app-wide.
  - [x] Role on the user (domain, migration, mapping)
  - [x] Authorization checks in the application layer; route protection on every `/api/*` endpoint
  - [x] Remove or lock down `/debug/*` (locked down to Admin; removal stays in 8.5)
  - [x] Users API (new): list users, change role, deactivate/reactivate; admin only, with the race-safe last-admin guard
  - [x] OpenAPI documents the auth requirements
- [x] **2.5 First-admin bootstrap & invite-only signup** (new, D1)
  - [x] Owner/admin account created from configuration at startup (a `minerva.toml` section, with the password supplied via env var or `*_file`), only when no users exist yet
  - [x] Remove the open `POST /api/auth/signup` endpoint (and update its tests and OpenAPI entry)
  - [x] SSO logins keep auto-creating accounts (behind `oidc.auto_create_users`); new SSO accounts get the fallback role from 2.7 (Read-only)
- [x] **2.6 Invite & password reset links** (orig #9, D2, D3): basic-auth accounts only, using one token-link mechanism.
  - [x] Invite: admin creates an invite (email, role); token is single-use, expiring and stored hashed; invitee follows the link and sets a password
  - [x] Invite management: list, revoke, re-issue
  - [x] Password reset uses the same mechanism
  - [x] If SMTP is configured the link is emailed (delivery arrives via 7.3); otherwise the admin gets a link to copy and share. The flow works fully without SMTP.
  - [x] Edge case to cover in design: an invited email that signs in via SSO before accepting the invite
  - Note: password reset is admin-initiated in v1 (an admin issues the link for a named user); the self-service "forgot password" endpoint lands with 7.3. The sub-item above about emailing is wired through the `AccountEmailSender` port, which today has only the no-op implementation — links are returned to the caller until 7.3 provides a real sender.
- [x] **2.7 SSO group-to-role mapping** (new, D8): mapping is configured by admins in the Admin settings UI, not env vars.
  - [x] Settings storage and admin-only API for group-to-role rules (UI is 6.5)
  - [x] Fallback role is Read-only: a user with no matching rule (or a misconfigured mapping) can still sign in and must ask an admin to correct it
  - [x] If several groups match, the **least permissive** role wins; permissions are the admin's decision and fixes happen in the IdP
  - [x] When rules exist, the role is recomputed at every SSO login (IdP is source of truth); those users are flagged "managed by SSO" so their role cannot be edited by hand in Minerva
  - [x] When no rules exist, roles are never touched
  - [x] To confirm in the prompt: the owner/admin account created in 2.5 is exempt from SSO role recomputation so nobody is locked out of administration
  - Note: both open questions above — the bootstrap admin's exemption from recomputation and whether group rules override an invite's role — are settled by D15; see the Decisions log before building this item.
- [ ] **2.8 Session & login hardening** (new)
  - [ ] Sliding expiry: call `touch_last_seen` on authenticated requests
  - [ ] Purge expired sessions in Postgres when Redis is absent
  - [ ] Make Redis session `create` atomic (MULTI/pipeline)
  - [ ] Equalise login timing for unknown and passwordless accounts (dummy Argon2 verify)
  - [ ] Rate limiting on login and reset/invite endpoints (Redis when present; Postgres counters or documented per-node limits when absent, since in-process counters break horizontal scaling)

---

## Milestone 3: API Layer

Goal: a documented, consistent HTTP API in front of the domain, complete enough that the frontend milestones never need backend changes mid-flight.

### Finish the resource APIs

- [x] **3.1 Goals CRUD** (orig #10, part)
- [ ] **3.2 Goal and milestone status override endpoints** (orig #10, part; D12): set a manual override and clear it ("return to automatic").
  - Overrides are **sticky**: they hold until explicitly cleared; recomputation never overwrites them.
  - Setting or clearing an override triggers an immediate progress snapshot (3.14).
  - No audit history in v1 (see Future releases); the override only records that the status source is manual.
- [x] **3.3 Milestones CRUD** (orig #11, part)
- [ ] **3.4 Goal↔Milestone linkage endpoints** (orig #11, part): link, unlink, list milestones for a goal and goals for a milestone. Partial: `goal_milestones` repository exists; no routes.
- [x] **3.5 Tasks CRUD** (orig #12, part)
- [ ] **3.6 Task relation endpoints** (orig #12, part; D4): create, delete, list for a task.
  - One canonical row per relationship. The API still accepts `blocks`, `blocked_by` and `relates_to`; `blocked_by` is normalised on write to a `blocks` row with the endpoints swapped, and the other view is derived when reading.
  - Reject self-relations, duplicate pairs (unique constraint on the normalised pair; `relates_to` ordered consistently) and direct reverse loops. Longer cycle detection is not in v1.
  - Fix `Task::is_blocked` so a blocker that is `Done` no longer counts as blocking.
  - Partial: repository exists; no routes.
- [ ] **3.7 Board-state transitions** (orig #12, part; D6): `PATCH /api/tasks/{id}/status` changing only the column. Any column to any column (no workflow restrictions in v1); a blocked task is a UI warning, not a hard stop.
- [x] **3.8 API error handling & response conventions** (orig #13): `{"error": {"code", "message"}}` envelope
- [x] **3.9 API documentation** (orig #14): OpenAPI + Swagger UI. **Standing rule:** every new endpoint ships with its `#[utoipa::path]` annotation.

### Gaps found in review (new)

- [ ] **3.10 Pagination & filtering** on list endpoints, agreed before the UI depends on the current shape
- [ ] **3.11 Assignees and owners:** task assignee; goal and milestone owner (domain, migration, API)
- [ ] **3.12 Task comments:** domain type, table, endpoints (needed by the task detail view)
- [ ] **3.13 Status computation** (D12): Milestone status from task completion, percent complete and target dates; Goal status from its milestones (wire up `compute_status_from_milestones`).
  - Always compute the automatic status, even when an override is active, and expose both the effective status and the computed status so the UI can show "manually set to At Risk (automatic: On Track)".
  - A goal's rollup uses its milestones' **effective** status (what people see, including overrides).
- [ ] **3.14 Progress snapshots** (D5): daily scheduled sweep plus an immediate snapshot whenever staff manually change a status.
  - The sweep runs inside the API process and is guarded by a Postgres advisory lock so only one node runs it (no Redis or extra worker container needed).
  - Snapshots record the effective status; no per-task-edit snapshots.
- [ ] **3.15 Dashboard API:** rollup counts by status, progress trend series from snapshots, timeline data (goals and milestones against target dates); flags items whose status is manually set
- [ ] **3.16 Pagination-safe list ordering:** deterministic default ordering for tasks (target date, then created date), since v1 has no manual card ordering

---

## Milestone 4: Frontend: Goal & Task Management

Goal: teaching staff can manage day-to-day work. Built on the M6 shell.

- [ ] **4.1 Task board UI** (orig #15): Backlog / To Do / In Progress / Done, drag and drop using the status `PATCH` (3.7); cards ordered by target date then created date; blocked tasks show a warning
- [ ] **4.2 Task detail view** (orig #16): edit fields, assignee, relations, comments
- [ ] **4.3 Milestone view** (orig #17): list and detail with linked tasks and goals
- [ ] **4.4 Goals list/detail and status override UI** (new): create and edit goals, link milestones, set or clear a status override, show automatic vs effective status
- [ ] **4.5 "My tasks" view** (new): tasks assigned to the signed-in user, answering "what do I need to do today"

---

## Milestone 5: Frontend: Reporting Dashboard

Goal: leadership and board audiences get the goal-progress view this project is built around.

- [ ] **5.1 Status rollup view** (orig #18): On Track / At Risk / Off Track / Complete counts at a glance, with manually set statuses flagged
- [ ] **5.2 Progress trend charts** (orig #19): from `ProgressSnapshot` history
- [ ] **5.3 Timeline view** (orig #20): goals and milestones against target dates

---

## Milestone 6: Frontend: Application Shell & UX

Goal: replace the SvelteKit scaffold with a real app a non-technical user can navigate. Executed before M4.

- [ ] **6.1 Same-origin browser-to-API setup** (new, D7): one public origin; the reverse proxy routes `/api/*` to the server and everything else to the web container.
  - Vite dev proxy for `bun run dev`
  - SvelteKit server-side loads call the API over the internal URL and forward the session cookie
  - `server.web_base_url` is the public origin; the OIDC redirect URL is that origin plus `/api/auth/oidc/callback`
- [ ] **6.2 Design system baseline** (orig #23): typography, colour, component conventions; includes an accessibility and mobile baseline (new)
- [ ] **6.3 Navigation & app shell** (orig #22): real layout replacing the scaffold
- [ ] **6.4 Auth UI** (orig #21): login, accept-invite and password-reset screens, plus the OIDC option alongside password (driven by `/api/auth/providers`). No public signup screen (D1).
- [ ] **6.5 Admin screens** (new): user list and role management, invite creation with copyable link (2.6), SSO group-to-role mapping (2.7), showing "managed by SSO" roles as read-only

---

## Milestone 7: Infrastructure & Deployment

Goal: easy to self-host and scales horizontally.

- [ ] **7.1 Object storage integration** (orig #24): S3 trait abstraction, RustFS default, works unmodified against AWS S3, MinIO, R2
  - [ ] Attachment domain type, table and endpoints (new; nothing models attachments yet)
- [x] **7.2 Redis-backed caching** (orig #25): closed. Sessions are done (Redis or Postgres, see 2.1). Query caching is not needed for v1 and moved to Future releases (D11). Redis also backs rate limiting (2.8).
- [ ] **7.3 Email delivery** (orig #26): swappable SMTP/provider abstraction used to send invite and reset links. Optional: the app works fully without SMTP (D3). Status-change alerts are a later addition on top of the same abstraction.
  - [ ] Implement the `AccountEmailSender` port over the SMTP/provider abstraction — the 2.6 flows already call it; today only the no-op implementation exists, so links are returned to the caller instead of emailed
  - [ ] Public, rate-limited "forgot password" request endpoint: issues the same reset token for a password account and always answers 204 without revealing whether the email exists (depends on 2.8 for rate limiting)
- [ ] **7.4 Production Docker Compose / Helm chart** (orig #27): beyond the dev compose file; includes a reference Caddy service doing same-origin routing (6.1), `server.cookie_secure = true` guidance, and an example of mounting `minerva.toml` (Swarm config / Kubernetes ConfigMap) with secrets via env or `*_file`
- [ ] ~~CI pipeline~~ (orig #28): moved to **0.2**
- [ ] **7.5 Backup & restore documentation** (new): Postgres and object storage, for self-hosters

---

## Milestone 8: v1 Release Polish

Goal: ship it.

- [ ] **8.1 Elastic License 2.0 text finalised** (orig #29): replace the `LICENSE` placeholder
- [ ] **8.2 Onboarding / first-run setup flow** (orig #30): minimise config friction (`minerva.toml`, bootstrap admin, SMTP, S3, Redis, OIDC), including guided Authentik and generic OIDC setup docs
- [ ] **8.3 Seed / demo data script** (orig #31): for evaluators and school IT staff
- [ ] **8.4 Test coverage targets** (orig #32): decide what "enough" looks like per layer
- [ ] **8.5 Security review pass** (new): cookie flags in production, CSRF posture (SameSite=Lax plus JSON-only endpoints, same-origin), rate limits, dependency audit, removal of all `/debug` code

---

## Future releases

Good ideas that are deliberately **not** in v1. Nothing here is planned work until it is moved into a milestone. Add new ideas as they come up.

- **Activity / audit history** (deferred from D9): who changed a status, override, assignment or role, and when
- **Export / printable reports** (deferred from D10): PDF or CSV board packs from the dashboard
- **Query caching** (deferred from D11): revisit only if profiling shows a real need
- **Manual card ordering within board columns** (from D6): needs a rank column; v1 sorts by target date then created date
- **Web UI as a standalone executable** (from D14): at the UI stage, investigate whether Bun (for example `bun build --compile`) can package the SvelteKit UI as a single executable. The UI stays a separate service from the API either way.
- **Configurable CORS allowlist** (`CORS_ALLOWED_ORIGINS`) for alternate frontends on other origins (from D7)

---

## Decisions log

All planning decisions so far. Record new decisions here with their outcome.

| ID | Decision | Outcome |
|---|---|---|
| D1 | Signup policy | **Invite-only.** The first account (admin/owner) is created from configuration (originally specified as environment variables; see D13). SSO users simply sign in via the IdP and their account is created automatically. |
| D2 | Email verification / invites | Email is only used to send an invite link carrying an embedded token tied to that user's invite. No separate signup verification. |
| D3 | Email delivery without SMTP | If no SMTP credentials are set, an Admin can generate an invite link to share any way they like. |
| D4 | Blocks / BlockedBy consistency | **Store one canonical row** and derive the other view when reading. |
| D5 | Snapshot cadence | **Daily scheduled sweep plus an immediate snapshot** on any manual status change. |
| D6 | Board transitions | **Dedicated `PATCH /api/tasks/{id}/status`** changing only the column. |
| D7 | Browser-to-API strategy | **Same-origin** behind the reverse proxy (recommendation accepted). |
| D8 | OIDC groups to roles | **In v1.** Mapping configured by admins in the Admin settings UI. Fallback role is Read-only. If multiple groups match, the least permissive role wins. |
| D9 | Audit / activity history | **Deferred** to a future release. |
| D10 | Export / printable reports | **Deferred** to a future release. |
| D11 | Query caching | **Not in v1** (recommendation accepted); moved to Future releases. |
| D12 | Manual override semantics | **Sticky** until explicitly cleared. The automatic status is always computed and shown alongside; rollups use effective status. |
| D13 | Configuration system | **`minerva.toml`** replaces the flat `.env` approach for server settings: typed config with real arrays, and it enables running the binary directly without Docker. Layered as defaults, then file, then env overrides, so Docker users can still use env vars only. `MINERVA_` prefix with `__` nesting plus `DATABASE_URL`, `REDIS_URL` and `PORT` aliases (assumed; change if you prefer another scheme). Secrets via env or `*_file`, not the TOML. Runtime-managed settings stay in the database. |
| D14 | Frontend deployment model | **The web UI is never served from the Rust binary.** Deliberate: the server stays modular so a Swarm or Kubernetes deployment can run multiple web services separately from the rest of the infrastructure, and anyone can omit the Web UI and build their own. Packaging the UI as a single executable (for example via Bun) is investigated later at the UI stage (see Future releases). |
| D15 | SSO role recomputation | **Rules always win.** While at least one group rule exists, the role of every user who signs in via SSO is recomputed at each login from the IdP groups claim; rules always win over an invite's role (the invite is still consumed; its role is used only when no rules exist). Matching is exact, case-sensitive after trimming, with one rule per group name; the least permissive matched role wins (Read-only < Staff < Admin), and a missing or malformed groups claim — or no match — gives Read-only. The 2.5 bootstrap admin is exempt via an explicit `users.sso_role_exempt` flag set at bootstrap; as a backstop for all users, recomputation never demotes the last active administrator (skip and log a warning). Users whose role was recomputed carry `users.role_managed_by_sso = true`, and hand edits to their role are rejected (409) only while rules exist — when no rules exist, roles are never touched. `oidc.groups_claim` (default "groups") is bootstrap config in `minerva.toml`; the rules themselves are stored in Postgres. |

**Open decisions:** none.

---

## Out of scope for v1

Explicitly not planned. Move an item out of this list (and into a milestone) before any work starts on it.

- Multi-tenancy or per-school scoping (single deployment = single school or organisation)
- Per-project or per-goal permissions (roles are app-wide)
- SAML or other non-OIDC identity protocols (the provider abstraction in 2.3 keeps the door open)
- Real-time collaboration and live cursors
- Native mobile apps (the web UI should be responsive, see 6.2)
- Serving the web UI from the Rust server (D14): the API and the UI are always separate, independently deployable services
- Time tracking, billing and resource planning
- Sprints, epics and other developer-centric constructs (by design; see project principles)

---

## Change log

| Date | Change |
|---|---|
| 2026-10-01 | Revised roadmap created from the original 8-milestone breakdown after a full repo review. Added M0; added missing M3 endpoints and supporting items (pagination, assignees/owners, comments, status computation, snapshots, dashboard API, audit); added goal management and "my tasks" to M4; added export to M5; added browser-to-API decision and admin screens to M6; moved CI from M7 to M0; reduced the Redis item to optional query caching; added attachments, backup docs and scheduler lock to M7; added a security pass to M8; recorded M1 done, M2 #5 and #6 done, and the M3 items already complete. |
| 2026-10-01 | All open decisions resolved (D1 to D12). Added 2.5 (first-admin bootstrap, invite-only signup), 2.6 (invite and reset links, admin-generated link fallback), 2.7 (SSO group-to-role mapping via admin UI). Dropped the pre-hijacking fix and signup email verification from M2. Removed audit history (3.16) and export (5.4) from v1 and created the Future releases section. Closed 7.2 (sessions done, query caching deferred). Merged the scheduler lock (old 7.6) into 3.14. Specified task-relation normalisation (3.6), status `PATCH` (3.7), sticky overrides (3.2, 3.13) and daily snapshot sweep (3.14). Added same-origin setup (6.1). |
| 2026-10-02 | M0 items 0.1 to 0.6 recorded as merged (0.4 reworded to match what was delivered). Added 0.7 (configuration system, `minerva.toml`) to M0 and made it the next item before 2.3. Added D13 (configuration system) and D14 (web UI is always a separate service). Updated 2.5, 7.4, 8.2 wording to refer to configuration instead of env vars. Added the web UI single-executable investigation to Future releases and the no-UI-in-Rust-binary rule to Out of scope. |
| 2026-10-02 | 0.7 (configuration system) delivered and ticked: typed, validated config in the `interface` crate (`src/config.rs`) layered as defaults < `minerva.toml` < `DATABASE_URL`/`REDIS_URL`/`PORT` aliases < `MINERVA_*` env vars; `--config`/`MINERVA_CONFIG` discovery; `*_file` secrets redacted from logs and `Debug`; legacy variables warn once at startup. All environment reads moved onto it (`main.rs`, OIDC config, cookie flags); `infrastructure::oidc::OidcConfig` is now a plain struct. Added `minerva.example.toml`, git-ignored `minerva.toml`, reduced `.env.example` to compose-level variables, updated compose and docs. M0 is done; next up is 2.3. |
| 2026-10-02 | 2.3 part 1 of 3 delivered (auth provider abstraction): session handling extracted into `application::auth::SessionService` (`issue`/`resolve`/`revoke`, `DEFAULT_SESSION_TTL`) over the new `SessionTokens` port (`Sha256SessionTokens` adapter in `infrastructure`); `PasswordHasher` is now async with Argon2 running on `spawn_blocking`; all auth handlers and the `AuthenticatedUser` extractor depend only on ports (`dyn UserRepository`, `dyn UserIdentityRepository`, `dyn PasswordHasher`, `SessionService`) — `hash_token`, `run_hasher`, `SESSION_TTL` and the `sha2` dependency are gone from `interface`. No behaviour change: cookies, status codes, bodies and DB writes are unchanged and all 0.3 characterization and OIDC handler tests pass; new unit tests cover the service with in-memory fakes. Ticked the "`AuthenticatedUser` extractor depends on the `UserRepository` port" sub-item; 2.3 stays open for parts 2 and 3. |
| 2026-10-02 | 2.3 part 2 of 3 delivered (auth provider contract): new `application::auth::provider` — the `AuthProvider`/`CredentialProvider`/`RedirectProvider` traits, `Credentials` (Debug redacts the secret), `PendingLogin`/`CallbackParams`/`RedirectStart`, and `AuthError`; the `AuthProviders` registry validates ids (`[a-z0-9_-]+`, unique across both lists) and can list providers for clients. Email/password login now runs through `PasswordAuthProvider` (id `password`) built in `main.rs`; the handler only translates the request to `Credentials`, calls the provider, issues a session and sets the cookie. No behaviour change: cookies, status codes, bodies and DB writes are unchanged and all 0.3 characterization tests pass; new unit tests cover the provider and the registry, including a second fake provider found by id with no other change. Ticked the orchestration sub-item for password (OIDC moves in part 3). |
| 2026-10-02 | 2.3 part 3 of 3 delivered (auth provider abstraction complete): OIDC sign-in now runs through `OidcAuthProvider` (`application::auth::oidc`), which wraps the `OidcProvider` protocol port in the `RedirectProvider` contract — user lookup, link/create decisions and identity creation (including the conflict re-lookup) moved out of the interface handlers into it. The OIDC-specific handler module was replaced by a generic redirect module (`interface/src/redirect.rs`): `GET /api/auth/{provider}/login?next=` and `/callback` dispatch to any registered redirect provider by id, unknown or non-redirect ids 404; the state cookie is now `minerva_auth_state`, encrypted, path-scoped per provider, payload `{provider, pending, next, expires_at}`; interface-level failures (missing/tampered/expired cookie, provider mismatch) redirect with `login_failed` while provider-carried codes are preserved. `/api/auth/providers` moved to the auth module and is driven by the registry (`{id, display_name, kind, login_url}`). Deliberate breaks only: state-cookie name/path/payload shape, `login_failed` for interface-level failures, providers response shape — callback URL, session cookie and the `oidc_*` codes are unchanged. A fake second redirect provider in the handler tests proves generic dispatch and the provider-mismatch rule; 2.3 is done. |
| 2026-10-02 | 2.4 step 1 of 5 delivered (role on the user): new `domain::Role` (`admin`/`staff`/`read_only` — identical in JSON and the database) and `domain::Permission` (`ViewContent`/`EditContent`/`ManageUsers`) with an exhaustive, wildcard-free `Role::allows` matrix; `User` gains a `role` field. Migration 20261002000001 adds `users.role TEXT NOT NULL CHECK (IN ('admin','staff','read_only'))`, promotes the single oldest user to `admin` so an existing dev database keeps an administrator until 2.5, then drops the column default so every insert must state its role. New accounts get `DEFAULT_NEW_USER_ROLE` (`read_only`) — open signup today, SSO fallback in 2.7 reuses it. `role` is exposed on `UserResponse` (login/signup/me) with a `RoleDoc` OpenAPI schema. No enforcement yet: every endpoint behaves exactly as before. Ticked the "Role on the user" sub-item; steps 2-5 of 2.4 remain. |
| 2026-10-02 | 2.4 step 2 of 5 delivered (authorization checks and route protection): new `application::authz` — `AuthzError::Forbidden` and `authorize(user, permission)`, the single place an access decision is made (future deactivation/ownership checks land there); `ApiError::forbidden()` (403, standard envelope); three interface extractors (`ViewAccess`/`EditAccess`/`AdminAccess`) wrapping `AuthenticatedUser` with ViewContent/EditContent/ManageUsers. Every goals/milestones/tasks route takes View (GET) or Edit (POST/PUT/DELETE); the temporary `/debug/*` routes are locked down to Admin; health, signup/login/logout, providers, the redirect flow, `/api/auth/me` (any role) and the API docs stay public as before. The route table moved out of `main.rs` into `routes::configure`, used by both the server and the new Postgres-backed access tests: a table-driven 401/403 matrix over every protected route with one user per role, an explicit public-route allowlist that fails when a route is added without an extractor or entry, and a same-cookie role promote/demote check. No response bodies or status codes changed for authorised requests; the OpenAPI auth documentation stays open (step 3). Ticked the authorization-checks and `/debug` lockdown sub-items. |
| 2026-10-03 | 2.4 step 3 of 5 delivered (OpenAPI documents the auth requirements): the document now carries a `session_cookie` security scheme — an API key in the session cookie, registered by a `utoipa::Modify` addon in `interface/src/openapi.rs` with the name taken from the same `COOKIE_NAME` constant the auth code uses — so Swagger UI's Authorize button fills it in. Every protected operation (`/api/auth/me` and all goals/milestones/tasks routes) declares `security(("session_cookie" = []))` plus a 401 (missing or invalid session); writes also carry a 403 (requires the Staff or Admin role), each operation's description states its required role in plain language, and the API-level description summarises the role model and the 401/403 meanings. Public operations (signup/login/logout/providers, the redirect flow) are unchanged. A new document test walks every path/method: an operation must be either in the shared `PUBLIC_OPERATIONS` allowlist or carry the session requirement and a 401; protected non-GETs outside `/api/auth/` must document a 403; allowlisted operations must not declare security; and every allowlist entry must exist in the document. The allowlist moved to a `#[cfg(test)] public_routes` module now used by both this test and the access tests, so a new `#[utoipa::path]` without security (or an allowlist entry) fails the build. No behaviour changed: no status codes or bodies touched, `/debug/*` stays undocumented. Ticked the OpenAPI sub-item; 2.4 step 4 (Users API) is next. |
| 2026-10-03 | 2.4 final step delivered (Users API), completing the item: new `application::user_admin::UserAdminService` — `list`, `change_role(actor, target, role)`, `deactivate`, `reactivate` — with a typed `UserAdminError` (`NotFound`, `CannotModifySelf`, `LastAdmin`, `Repository`). The rules live in the service, not the handlers: an administrator cannot change their own role or deactivate themselves; the last **active** administrator can never be demoted or deactivated (a deactivated admin does not count); re-applying the current state is a no-op. The last-admin guard is race-safe by construction: the new `UserRepository::apply_access_change` port method applies the change inside one Postgres transaction guarded by a transaction-level advisory lock, re-checking the active-admin count only after acquiring it, so concurrent changes serialize instead of interleaving; the in-memory test fakes enforce the same rule. Deactivation sets `users.deactivated_at` (migration 20261003000001), revokes all of the user's sessions (`SessionService::revoke_all_for_user`) and refuses new sign-ins — password login fails as invalid credentials, OIDC completion rejects the account — while the per-request user re-resolution in `AuthenticatedUser` is the backstop that 401s any surviving session on its next request. New admin-only routes in `interface/src/users.rs`: `GET /api/users` (stable order: created_at then id), `PUT /api/users/{id}/role`, `POST /api/users/{id}/deactivate`, `POST /api/users/{id}/reactivate`; mutations return the updated user; `UserAdminResponse` never carries a password hash; unknown id is 404, self-modification and last-admin are 409 in plain language, an unknown role string is 400. All four operations are documented under a new `users` tag with the `session_cookie` security scheme and 401/403 responses, covered by the document test; the access-test matrix gained the four routes (401 without a cookie, 403 for read-only/staff, open for admin). Postgres-backed behaviour tests cover: listing, a role change applying on the user's next request with the same session, deactivation killing an already-issued session and login (reactivation restoring both), self-modification 409s, demoting one of two admins succeeding while the remaining one gets the last-admin 409, a deactivated admin not counting as active, and repeated concurrent mutual demotions that never leave zero active admins; plus unknown-id 404 and unknown-role 400. Docs updated: `docs/architecture.md` gained a "User administration" subsection (rules, lock, deactivation semantics) and the `AdminAccess` row now lists `/api/users`; the README keeps the temporary SQL-promote note for the first admin but points to the API once one exists; `CLAUDE.md` mentions the user-administration endpoints. Ticked the Users API sub-item and 2.4 itself; next is 2.5 (first-admin bootstrap and invite-only signup). |
| 2026-10-03 | 2.5 delivered (first-admin bootstrap, open signup removed), completing the item: new `[bootstrap]` config section — `admin_email` (blank disables the bootstrap), `admin_display_name` (default "Administrator") and `admin_password` (required when the email is set, at least 8 characters; setting the password without an email is also rejected) handled as a secret like the others: env var or `*_file`, redacted from logs and `Debug`, validated at startup together with the other config errors. New `application::bootstrap::bootstrap_admin` normalizes the email, hashes the password (the plaintext is never stored or logged) and creates an Admin account; whether anyone may be created is decided by the new `UserRepository::create_if_no_users` port method — one Postgres transaction that takes a transaction-level advisory lock before counting rows and inserting, so racing first starts cannot both win, and any existing user (deactivated or passwordless) blocks it; the empty-table and blocked branches are covered by fake-based unit tests. The server calls it at startup only when configured, logging `created bootstrap admin account <email>` or the skip line. Open signup is gone: the `POST /api/auth/signup` route, handler, request schema and its characterization tests are removed (the access tests now pin the path to 404), and the README, architecture doc, CLAUDE.md and OpenAPI no longer describe it — the SQL `UPDATE users SET role = 'admin'` remains only as a manual recovery path for databases that already have users. SSO behaviour is locked in by test: a callback-created account gets `Role::ReadOnly` (`DEFAULT_NEW_USER_ROLE`, never Admin), and with `oidc.auto_create_users = false` unknown users are still rejected with `oidc_signup_disabled`. Ticked 2.5; next is 2.6 (invite & password reset links). |
| 2026-10-04 | 2.6 delivered (invite & password reset links), completing the item: one token-link mechanism for both flows — new `account_tokens` table (migration 20261004000001) where an invite names an email and a role and a reset names the user whose password changes, with only the SHA-256 hash of the token stored (the raw value appears once, in the link). Partial unique indexes keep at most one live (unconsumed, unrevoked) token per subject, so re-issuing replaces the old token even under concurrent issuers; consumption is an atomic single-use claim. TTLs are 7 days for invites and 24 hours for resets. New `application::account_links::AccountLinkService` owns the rules: create/list/revoke/re-issue invites (re-issue revokes the live token and inserts a fresh one in one transaction), issue password resets, inspect a token without consuming it, accept an invite and reset a password. Email delivery goes through the new `AccountEmailSender` port — today only the no-op `NoEmailSender` exists, so every send fails gracefully and the link is returned to the caller instead (a real sender arrives with 7.3); the flow works fully without SMTP (D3). New admin-only routes: `POST /api/invites` (201 with `{invite, link, emailed}`), `GET /api/invites` (newest first), `POST /api/invites/{id}/revoke` (idempotent; 404 unknown, 409 already accepted), `POST /api/invites/{id}/reissue` (201 with a fresh token) and `POST /api/users/{id}/password-reset` (201 with `{link, expires_at, emailed}`; 404 unknown user, 409 for SSO-only or deactivated accounts). New public routes: `POST /api/auth/tokens/inspect` (200 with purpose/email/role/expiry; one opaque 400 `invalid_token` for unknown, expired, used or revoked tokens), `POST /api/auth/accept-invite` (201 with the created user and a session cookie — accepting signs the invitee in; 409 `account_exists` when the email already has an account) and `POST /api/auth/reset-password` (204, revokes all of the account's sessions so older devices are signed out). Links are built from `server.web_base_url` (`/accept-invite?token=…`, `/reset-password?token=…`; site-relative paths when it is unset). The SSO edge case is covered: an invited email that signs in via SSO before accepting gets its account created with the invite's role and the pending invite consumed, so the link then fails; expired or revoked invites are ignored, a pending invite never enables SSO signup while `oidc.auto_create_users` is off, and linking to an existing user never touches invites. Docs updated: `docs/architecture.md` gained an "Invites and password reset" subsection plus the new public routes and `AdminAccess` entries, the README gained an "Inviting people" note, and CLAUDE.md the twelfth migration and the new endpoints. Ticked 2.6; next is 2.7 (SSO group-to-role mapping). |
| 2026-10-05 | Recorded D15 (SSO role recomputation), settling the invite-role interplay that 2.7's notes flagged as needing a decision before the item is built: while at least one group rule exists, the role of every user signing in via SSO is recomputed at each login from the IdP groups claim and rules always win over an invite's role (the invite is still consumed; its role is used only when no rules exist); matching is exact, case-sensitive after trimming, with one rule per group name, the least permissive matched role wins (Read-only < Staff < Admin), and a missing or malformed groups claim — or no match — gives Read-only; the 2.5 bootstrap admin is exempt via an explicit `users.sso_role_exempt` flag set at bootstrap, and as a backstop for all users recomputation never demotes the last active administrator (skip and log a warning); users whose role was recomputed carry `users.role_managed_by_sso = true` and hand edits to their role are rejected (409) only while rules exist; `oidc.groups_claim` (default "groups") is bootstrap config in `minerva.toml` while the rules themselves stay in Postgres. Replaced the two open 2.7 notes with a single pointer to D15 and widened the resolved-decisions range to D1 to D15. No boxes ticked; no code written. |
| 2026-10-05 | 2.7 step 2 of 6 delivered (domain type, storage and repository — no routes, no behaviour change): new `domain::SsoGroupRule` (`id`, `group_name`, `role`, timestamps) with `SsoGroupRuleId`; an exhaustive, wildcard-free `Role::rank()` (Read-only < Staff < Admin); a separate `SSO_FALLBACK_ROLE` constant (Read-only, deliberately distinct from `DEFAULT_NEW_USER_ROLE`); and the pure `resolve_role(rules, groups)` — exact, case-sensitive match on trimmed names, least permissive matched role wins, no match gives the fallback (D15) — with unit tests covering no rules, no match, a single match, several matches choosing the least permissive, whitespace trimming and case sensitivity. Migration 20261005000001 creates `sso_group_role_rules` (uuid PK; `group_name` TEXT NOT NULL with a non-empty-after-trim CHECK; role CHECK on the three role strings; timestamps) plus a unique index on `group_name`. New application port `SsoGroupRuleRepository` — `list` (stable order: group name then id), `find_by_id`, `create`, `update`, `delete`, `any_exist` — where a duplicate group name surfaces as a typed `Conflict`, never a raw Diesel error; the Postgres implementation follows the existing Diesel pattern and an in-memory fake enforces the same one-rule-per-name rule for application tests. New Postgres round-trip tests cover CRUD, ordering, `any_exist` and the duplicate error (skipped without `DATABASE_URL`, run in CI). No boxes ticked; next is step 3 of 6. |
| 2026-10-05 | 2.7 step 3 of 6 delivered (the admin-only group-rule API — no sign-in behaviour change yet): new `application::sso_rules::SsoGroupRuleService` (`list`, `create`, `update`, `delete`) with a typed `SsoGroupRuleError` (`InvalidGroupName`, `GroupRuleExists`, `NotFound`, `Repository`); validation lives in the service, not the handlers — the name is trimmed and must be 1 to 255 characters, and the repository's duplicate-name `Conflict` becomes the typed `GroupRuleExists` — with unit tests over the in-memory fake covering trimming, both length bounds, duplicates across whitespace, rename-onto-taken-name, unknown ids and delete. New admin-only routes in `interface/src/sso_rules.rs`: `GET /api/sso/group-rules`, `POST /api/sso/group-rules` (201), `PUT /api/sso/group-rules/{id}`, `DELETE /api/sso/group-rules/{id}` (204); the body is `{group_name, role}` and the response `{id, group_name, role, created_at, updated_at}`; a duplicate name is 409 with the new `group_rule_exists` code, an unknown id is 404, and an empty or too-long name or an unknown role string is 400 (the latter through the standard JSON handler, as with the Users API). All four operations are documented under a new `sso` tag with the `session_cookie` security scheme, 401/403 and their per-route 400/404/409 responses, each stating the required Admin role; the OpenAPI document test passes unmodified. The access-test matrix covers all four routes (401 without a cookie, 403 for read-only and staff, success for admin), and Postgres-backed behaviour tests cover the create/list/update/delete round trip, the duplicate 409 (create and rename-onto-taken-name), name trimming, the 400 cases and unknown-id 404s. No boxes ticked; next is step 4 of 6. |
| 2026-10-05 | 2.7 step 4 of 6 delivered (the two SSO flags on users plus the hand-edit lock — no sign-in behaviour change yet): migration 20261005000002 adds `users.role_managed_by_sso` and `users.sso_role_exempt` (both BOOLEAN NOT NULL DEFAULT false), carried by the domain `User` and the Diesel mapping. The bootstrap admin is created with `sso_role_exempt = true` through `create_if_no_users`; every other creation path leaves both flags false, pinned by tests over the bootstrap, invite acceptance and normal creations. `UserAdminService::change_role` now rejects a target whose role SSO recomputed (D15) while any group rule exists: new `UserAdminError::RoleManagedBySso`, checked in the contract order NotFound, CannotModifySelf, RoleManagedBySso, LastAdmin — the lock reads two committed facts (the flag, the rule count) so a stale read can only delay it; deactivation and reactivation are unaffected, and the last-admin guard stays untouched inside the repository's locked transaction. HTTP: 409 with the new `role_managed_by_sso` code and a plain-language message pointing at the identity provider, documented on PUT /api/users/{id}/role; every user response now reports `role_managed_by_sso` as its effective value (the flag AND any rule exists), computed once per request. Unit tests over the in-memory fakes cover the lock while a rule exists, unlocking when the rules are deleted, unflagged users changing fine, deactivation unaffected, and the check order (NotFound and self-edit precede the lock; the lock precedes the last-admin guard); Postgres-backed behaviour tests cover the 409 while a rule exists and the change succeeding after it is gone, the effective value in list and single-user responses, and deactivating a flagged user. The README's First admin section notes the exempt flag and the one-off `UPDATE` for databases bootstrapped before this change. No boxes ticked; next is step 5 of 6. |
| 2026-10-05 | 2.7 step 5 of 6 delivered (carrying the IdP's groups through SSO login — no role behaviour change yet): most of the plumbing already existed from the OIDC work — `oidc.groups_claim` in the typed configuration (default "groups", a blank value falls back to it, passed to the adapter as a plain value), `OidcClaims.groups`, and the adapter reading the configured claim from the re-parsed, fully verified ID token with no userinfo call — so this step adds what was missing: a warning log when the claim is absent or not a string/array of strings (a lone string is still accepted, as before; group contents and tokens are never logged), a configuration test for the override and the blank fallback, a `claims_with_groups` test helper so the fake OIDC provider can supply groups to the upcoming role tests, and the README's OIDC section documenting `oidc.groups_claim` with guidance that the IdP must release the claim (and `oidc.scopes` may need adjusting for it — live validation against Authentik is still pending 0.4, so it is worded as guidance). The groups already reach `decide_login` inside `OidcAuthProvider::complete` via the claims and are unused for now. No boxes ticked; next is step 6 of 6. |
| 2026-10-05 | 2.7 step 6 of 6 delivered (the D15 behaviour itself), completing the item: new `application::sso_roles::SsoRoleService` (`for_new_user`, `recompute`) used by `OidcAuthProvider` after the user is resolved or created and only after the deactivated-account rejection, computing roles with the domain's `resolve_role`. While no rule exists nothing is touched — new SSO accounts keep today's role (the pending invite's if one existed, else Read-only) unflagged; while rules exist a new account is created with the computed role and `role_managed_by_sso = true` (a pending invite is still consumed but its role ignored), an existing user's role moves to the computed one at every login (linking follows the same rule), the exempt bootstrap admin is skipped entirely, and the last active administrator is never demoted — the change is skipped, role and flag both untouched, with a warning naming the account's email and the computed role. The write goes through a new `AccessChange::RoleManagedBySso(Role)` on `UserRepository::apply_access_change`, setting role and flag in the same advisory-locked transaction under the existing last-admin guard (a no-op only when the role matches AND the flag is set); roles are re-resolved per request, so no session revocation is needed. `domain::Role` gains a `Display` matching the JSON/database strings for that warning. Two latent bugs from earlier steps were fixed along the way: the Postgres `create_if_no_users` insert omitted both SSO flag columns (the bootstrap admin would have come up non-exempt on a real database), and the in-memory fake cleared both flags on deactivate/reactivate where Postgres leaves them untouched. Tests: nine fake-provider tests over the full D15 matrix (no rules leaves new, existing and invited-role accounts untouched; matching groups for new and existing users; several matches giving the least permissive; no match and empty groups falling back to Read-only; an existing Admin demoted when its groups no longer match; the invite role overridden with rules and honoured without; the exempt admin never changed; the last-admin backstop skipping while a second active admin allows the demotion; the SSO-computed role locked against hand edits until the rules are deleted), plus a Postgres-backed test of the locked role+flag write, its no-op, the flag-only write and the refused last-admin demotion. Docs: `docs/architecture.md` gained an "SSO group mapping" subsection, the README a short admin-facing section, and CLAUDE.md the feature summary (and the migration count). Ticked 2.7 and its sub-items; next is 2.8 (session & login hardening). |