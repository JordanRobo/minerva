# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Minerva is an open source project-management platform for schools, organized goal/milestone-first. The repo is at the scaffolding stage: no business logic exists yet — the only backend endpoint is `GET /health`. The license will be Elastic License 2.0; `LICENSE` is a placeholder until the real text is added.

## Commands

### Full stack (Docker)

```sh
docker compose -f deploy/docker-compose.yml up --build   # start everything
docker compose -f deploy/docker-compose.yml down         # stop
```

Endpoints once running: web http://localhost:3000, API health http://localhost:3010/health, Postgres localhost:5432 (user/password/db all `minerva`), Redis localhost:6379, RustFS S3 API :9000 / dashboard :9001.

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

The `interface` crate's binary is named `minerva-server`. For a local (non-Docker) run, point `DATABASE_URL` at a running Postgres; the compose file supplies all env vars to the container.

Diesel migrations live in `server/migrations/` (currently empty). Generate one with the diesel CLI from `server/` with `DATABASE_URL` set: `diesel migration generate <name>`.

## Architecture

Monorepo: `apps/web` (SvelteKit frontend), `server` (Rust backend), `deploy` (compose + Dockerfiles), `docs`. The frontend talks to the backend over HTTP only.

The backend is a DDD-layered Cargo workspace; full details in `docs/architecture.md`:

- `domain` — core business concepts/invariants, pure logic. **Zero external dependencies**; depends on nothing.
- `application` — use cases and orchestration. Depends only on `domain`.
- `infrastructure` — adapters for Postgres (Diesel), Redis, and S3-compatible object storage (`object_store`).
- `interface` — Actix-web HTTP API and the composition root that wires the other crates together at startup.

Rules to preserve when adding code:

- Dependencies point inward toward `domain`; only `interface` may depend on actix-web, and HTTP types never leak into the other crates.
- Redis and S3 are optional at runtime. All their configuration (`REDIS_URL`, `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`) is read from environment variables, with local-dev defaults in `deploy/docker-compose.yml` — never hardcoded in Rust source.

### Docker build notes

- Both Dockerfiles use the **repository root** as build context (the compose file lives in `deploy/`, so it sets `context: ..`). Keep `.dockerignore` (root) covering `node_modules`, `.svelte-kit`, and `server/target`.
- `server.Dockerfile` copies only Cargo manifests plus stub `src/` files before `cargo fetch` to cache dependency downloads, then copies real sources. If you add a workspace member crate, update both the manifest-copy lines and the stub-src line.
- The web image builds with adapter-node on an `oven/bun` base image; if you change adapters, update `deploy/docker/web.Dockerfile` accordingly (it expects `build/` output runnable via `bun build/index.js` — note bare `bun build` is Bun's bundler command).

## Environment notes

- Host port **8080 on the dev machine is occupied by gluetun** (the user's always-on media stack: qbittorrent/sonarr/radarr/prowlarr also hold 6881, 7878, 8989, 9696). Compose therefore maps the API to host port **3010** (`3010:8080`; container-internal port stays 8080 via `PORT`). Don't bind new services to those ports.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
