# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Minerva is an open source project-management platform for schools, organized goal/milestone-first. The backend has: the `domain` layer (Goal, Milestone, GoalMilestone, Task, TaskRelation, ProgressSnapshot, Status, User — with a Role —, Session, UserIdentity, Permission), eleven Diesel migrations (the six core tables plus users/sessions, user_identities and the role/deactivation adjustments), application-layer repository ports with Postgres implementations, a Redis-backed session repository as an alternative to the Postgres one, an auth provider registry (`AuthProviders`) with two kinds of providers — credential providers (today: email/password via Argon2id) and redirect providers (today: OIDC, behind the `OidcProvider` port) — all issuing cookie sessions, `/api/*` endpoints for auth, goals, milestones, tasks and user administration (admin-only role and active-state changes), and OpenAPI 3 with Swagger UI at `/api-docs`. Every `/api/*` route requires a session with the matching role — the view/edit/admin access extractors call `application::authz::authorize` (see docs/architecture.md, "Authorization"); the temporary `/debug/*` wiring-verification endpoints are locked down to Admin. Public routes: health, signup/login/logout, providers, the redirect flow and the API docs. The license will be Elastic License 2.0; `LICENSE` is a placeholder until the real text is added.

## Commands

### Full stack (Docker)

```sh
docker compose -f deploy/docker-compose.yml up --build   # start everything
docker compose -f deploy/docker-compose.yml down         # stop
```

Endpoints once running: web http://localhost:3000, API health http://localhost:3010/health, Postgres localhost:5432 (user/password/db all `minerva`), Redis localhost:6379, RustFS S3 API :9000 / dashboard :9001.

The repo-root `.env` (copy of `.env.example`) holds only the variables the compose file substitutes (`DATABASE_URL`, `PORT`, `REDIS_URL`); pass it explicitly with `docker compose --env-file .env -f deploy/docker-compose.yml up --build`, since compose resolves `.env` relative to `deploy/`. Every value is optional. The server's own settings (OIDC, cookies, migrations) come from `minerva.toml` (see `minerva.example.toml`) or `MINERVA_`-prefixed environment variables — see README.md, "Configuration".

### Frontend (`apps/web`, SvelteKit + TypeScript)

```sh
bun run dev      # Vite dev server on :5173
bun run build    # production build via adapter-node -> apps/web/build, served with `bun build/index.js` (PORT/HOST env vars)
bun run check    # svelte-check type checking
```

The frontend uses **Bun** as its package manager (`bun install`; lockfile is `apps/web/bun.lock`).

No test framework is configured yet.

### Backend (`server`, Rust Cargo workspace)

```sh
cargo build --workspace
cargo run -p interface   # Actix server on $PORT (default 8080); verify with curl localhost:8080/health
```

The `interface` crate's binary is named `minerva-server`. For a local (non-Docker) run, point `DATABASE_URL` at a running Postgres, or build it and run `./server/target/release/minerva-server --config minerva.toml`; the compose file supplies all env vars to the container.

Migrations are embedded in the binary and applied at server startup (`server.run_migrations = false` in `minerva.toml`, or `MINERVA_SERVER__RUN_MIGRATIONS=false`, skips them, e.g. when a dedicated migration job owns the schema). They live in `server/migrations/`; generate a new one with the diesel CLI from `server/` with `DATABASE_URL` set: `diesel migration generate <name>`.

CI (`.github/workflows/ci.yml`) runs `cargo fmt --all -- --check`, clippy with `-D warnings`, and `cargo test --workspace` against Postgres/Redis services, plus the web `check` and `build`.

## Architecture

Monorepo: `apps/web` (SvelteKit frontend), `server` (Rust backend), `deploy` (compose + Dockerfiles), `docs`. The frontend talks to the backend over HTTP only.

The backend is a DDD-layered Cargo workspace; full details in `docs/architecture.md`:

- `domain` — core business concepts/invariants, pure logic. No I/O and no framework dependencies; `uuid`, `chrono` and `serde` are the allowed value-type exceptions (see `server/domain/Cargo.toml`).
- `application` — use cases and orchestration. Depends only on `domain`.
- `infrastructure` — adapters for Postgres (Diesel) and Redis; `object_store` is declared but the S3 adapter is not built yet (roadmap 7.1).
- `interface` — Actix-web HTTP API and the composition root that wires the other crates together at startup. It is also the only crate that reads files or environment variables: typed, validated configuration lives in its `config` module (`src/config.rs`).

Rules to preserve when adding code:

- Dependencies point inward toward `domain`; only `interface` may depend on actix-web, and HTTP types never leak into the other crates.
- Redis is optional at runtime; S3 support lands with roadmap 7.1. All configuration is loaded by the `interface` crate's config module (defaults < `minerva.toml` < `DATABASE_URL`/`REDIS_URL`/`PORT` aliases < `MINERVA_*` env vars), with local-dev defaults in `deploy/docker-compose.yml` — never hardcoded in Rust source.

## Roadmap

`docs/roadmap.md` is the project checklist and the source of truth for scope. Tick an item when a change completes it, and do not build anything that is not in the roadmap — including its Future releases / Out of scope sections — without asking first.

### Docker build notes

- Both Dockerfiles use the **repository root** as build context (the compose file lives in `deploy/`, so it sets `context: ..`). Keep `.dockerignore` (root) covering `node_modules`, `.svelte-kit`, and `server/target`.
- `server.Dockerfile` copies only Cargo manifests plus stub `src/` files before `cargo fetch` to cache dependency downloads, then copies real sources. If you add a workspace member crate, update both the manifest-copy lines and the stub-src line.
- The web image builds with adapter-node on an `oven/bun` base image; if you change adapters, update `deploy/docker/web.Dockerfile` accordingly (it expects `build/` output runnable via `bun build/index.js` — note bare `bun build` is Bun's bundler command).

## Environment notes

- Host port **8080 on the dev machine is occupied by gluetun** (the user's always-on media stack: qbittorrent/sonarr/radarr/prowlarr also hold 6881, 7878, 8989, 9696). Compose therefore maps the API to host port **3010** (`3010:8080`; container-internal port stays 8080 via `PORT`). Don't bind new services to those ports.

## graphify

Graphify is an **optional local tool**: the knowledge graph at `graphify-out/`
is generated locally and git-ignored, and any hooks for it belong in
`.claude/settings.local.json`, never in the committed `.claude/settings.json`.
Example hook entry (substitute your own install path):

```json
{ "type": "command", "command": "/path/to/graphify hook-guard read" }
```

When graphify is installed and `graphify-out/graph.json` exists:

Rules:
- For codebase questions, first run `graphify query "<question>"`. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
