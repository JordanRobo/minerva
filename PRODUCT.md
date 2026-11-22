# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Primary (confirmed 2026-11-22): school staff** — coordinators and teachers who plan the term's work and track how it is going. They define what the team is trying to achieve, break it into milestones, assign tasks, and check progress without maintaining status by hand.
- **Secondary: non-profit staff, volunteers and boards** — the same product serves them; copy and surfaces must not assume school-only context in shared places, but school vocabulary (e.g. "term") stays primary.
- Three roles, gated server-side on every route: **Administrator** (people, invitations, SSO group rules), **Staff** (create and edit goals, milestones, tasks, comments, assignments, status overrides), **Read-only** (view everything). Plain-language labels are shown as "Administrator", "Staff", "Read-only".

## Product Purpose

Minerva is an open source project-management platform built for schools (and non-profits). It is **goal/milestone-first**: teams start from what they are trying to achieve, break it into milestones, and track progress against those — rather than managing a flat list of tasks. It exists to keep staff, volunteers and boards on the same page about the work that matters, self-hosted without enterprise overhead. Success looks like: at any moment, every goal and milestone shows a trustworthy status derived from real task progress and schedule, with history recorded automatically.

## Positioning

The organizing unit is the goal, not the task. The whole system — data model, status engine (computed On Track / At Risk / Off Track / Complete from task completion and schedule gap, rolled up milestones → goals), sticky-but-visible manual overrides, daily progress snapshots, and the dashboard — is built around that hierarchy. A flat to-do or task-board tool cannot truthfully copy it: in Minerva, tasks serve milestones, milestones serve goals, and status is computed and persisted, not typed in. Second axis: open source and self-hosted for schools and non-profits, without enterprise overhead.

## Operating Context

- **Self-hosted, single origin:** one Docker Compose stack (Postgres, optional Redis, RustFS S3-compatible storage, Caddy reverse proxy); the browser sees one origin where `/api/*` is routed to the API and everything else to the web app.
- **Invite-only access:** no public signup. Administrators issue one-time invite links and password-reset links; until email delivery ships (roadmap 7.3) links are copied and shared manually.
- **SSO:** OIDC sign-in alongside email/password, for schools that already run an identity provider (tested with Authentik). IdP groups map to Minerva roles (least permissive match wins; no match → Read-only); the bootstrap admin is exempt and a last-active-admin backstop applies.
- **Term rhythm:** school terms frame planning vocabulary and cadence.
- Current build state: backend API layer complete (roadmap M3), application shell and auth UI done (M6); goal/task management UI (M4) and reporting dashboard UI (M5) not started; v1 release polish (M8) not started.

## Capabilities and Constraints

Confirmed today: goals, milestones and tasks with goal↔milestone linkage; task relations (blocks / blocked-by / relates-to, one canonical row per pair); board-state transitions on tasks (any column to any, no workflow restrictions in v1); one assignee per task, one owner per goal/milestone (person summaries never expose email); flat plain-text task comments (author edits, author-or-admin deletes); persisted automatic status with sticky manual overrides reporting their source; append-only daily progress snapshots; dashboard API (summary counts by effective status, trends from snapshot history, timeline of goals and milestones); paginated/filterable lists; rate limiting on login, token-link and invite-issuance routes; OpenAPI + Swagger UI at `/api-docs`.

Constraints and boundaries:

- **No student personal information (confirmed 2026-11-22):** Minerva holds only the work data staff manage — goals, milestones, tasks, comments. No student names or records are stored. This is a durable guardrail for every future surface and piece of copy.
- **Pre-launch (confirmed 2026-11-22):** no production deployments; no customers, testimonials, case studies or benchmarks exist. Future copy must not invent adoption claims.
- License will be Elastic License 2.0; `LICENSE` is a placeholder until the real text is added (README says TBD).
- No email delivery yet (roadmap 7.3); S3 object-storage integration not built (roadmap 7.1).
- Deliberately out of v1 (roadmap): audit history, comment threading/mentions/reactions/attachments, longer cycle detection in task relations, snapshot retention, dashboard caching/export, workflow restrictions on task columns, public signup.

## Brand Commitments

- **Name and tagline:** Minerva — "Open source, goal/milestone-first project management for schools and non-profits."
- **Voice (binding, from `docs/design/brand.json`):** warm, capable, unfussy — "a well-run staff room". Plain, direct sentences about getting the term's work done; no enterprise overhead or jargon, no corporate or bureaucratic phrasing, never flat task-list framing. Use: goal, milestone, term, progress, staff, volunteers, boards.
- **Messaging pillars:** (1) open source for schools and non-profits — self-hosted, without the enterprise overhead; (2) goal/milestone-first rather than a flat task list; (3) keeps staff, volunteers and boards on the same page.
- **Wordmark:** Fraunces wordmark with a terracotta dot over the i — the only colour in the wordmark. Strict usage rules (clear space, minimum sizes, one-colour black/white/ink versions, never retype/stretch/squash/rotate/outline/effects). Assets: `apps/web/static/wordmark*.svg`, `favicon.svg`; banners in `docs/assets/`.
- The full incumbent visual world (palette, typography, layout rules) is recorded in `docs/design/` (`DESIGN.md`, `brand.json`, `system/`).

## Evidence on Hand

- The running product itself: full dev stack via Docker Compose at one origin; backend covered by a large Postgres-backed test suite.
- Brand assets: wordmark set and favicon in `apps/web/static/`, banner SVGs in `docs/assets/`, brand seed and voice in `docs/design/brand.json`.
- Scope record: `docs/roadmap.md` (checklist + decisions log D1–D22) is the source of truth for what is planned.
- **Absent:** no customers, testimonials, case studies, press, benchmarks or usage statistics — pre-launch. Do not fabricate any of these.

## Product Principles

1. **Goals before tasks.** The unit of planning is what the team is trying to achieve; tasks serve milestones, milestones serve goals. Never frame Minerva as a to-do app.
2. **Status that maintains itself.** Progress status is computed from real work and schedule and recorded automatically; manual overrides exist but are always visible as manual.
3. **Self-hosted without enterprise overhead.** One stack, invite-only access, SSO that works with the school's existing identity provider — no per-seat cloud.
4. **Staff data only.** Minerva never holds student personal information; design and copy must keep it that way.
5. **A well-run staff room.** Warm, capable, unfussy language about getting the work done; no corporate jargon.
