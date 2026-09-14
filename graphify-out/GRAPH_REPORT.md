# Graph Report - minerva-pm  (2026-09-14)

## Corpus Check
- Corpus is ~2,477 words - fits in a single context window. You may not need a graph.

## Summary
- 111 nodes · 125 edges · 19 communities (11 shown, 3 thin omitted)
- Extraction: 89% EXTRACTED · 11% INFERRED · 0% AMBIGUOUS · INFERRED: 14 edges (avg confidence: 0.93)
- Token cost: 0 input · 75,476 output

## Community Hubs (Navigation)
- TypeScript Compiler Config
- Frontend Tooling Config
- Claude Project Guidance
- Project Overview & Identity
- Docker Compose Dev Stack
- Frontend Dev Dependencies
- DDD Architecture Rules
- Web NPM Scripts
- SvelteKit Scaffold & Bun
- Health Endpoint Handler
- Cargo Workspace Members
- Favicon & Layout Shell
- SvelteKit Type Definitions
- Robots Crawling Rule

## God Nodes (most connected - your core abstractions)
1. `CLAUDE.md Project Guidance` - 14 edges
2. `compilerOptions` - 11 edges
3. `README.md (project overview)` - 8 edges
4. `scripts` - 7 edges
5. `docker-compose.yml (full local dev stack: minerva)` - 7 edges
6. `infrastructure Crate (lib; adapters for Postgres/Diesel, Redis, S3 object_store; config from env vars)` - 7 edges
7. `interface Crate (bin; Actix-web HTTP API server; routing/request mapping; composition root wiring crates at startup)` - 7 edges
8. `DDD Layered Cargo Workspace (four crates: domain/application/infrastructure/interface)` - 6 edges
9. `Minerva Project (open source school project-management platform)` - 5 edges
10. `Minerva API Server Service (built from deploy/docker/server.Dockerfile; PORT 8080, DATABASE_URL/REDIS_URL/S3_* env vars; host port 3010:8080)` - 5 edges

## Surprising Connections (you probably didn't know these)
- `infrastructure Crate (lib; adapters for Postgres/Diesel, Redis, S3 object_store; config from env vars)` --semantically_similar_to--> `infrastructure Crate (Postgres/Diesel, Redis, S3 object_store adapters)`  [INFERRED] [semantically similar]
  docs/architecture.md → CLAUDE.md
- `interface Crate (bin; Actix-web HTTP API server; routing/request mapping; composition root wiring crates at startup)` --semantically_similar_to--> `interface Crate (Actix-web HTTP API + composition root; binary minerva-server)`  [INFERRED] [semantically similar]
  docs/architecture.md → CLAUDE.md
- `domain Crate (lib; core business concepts/invariants such as goals and milestones; pure logic, no I/O, zero external dependencies)` --semantically_similar_to--> `domain Crate (core business concepts/invariants, pure logic, zero external dependencies)`  [INFERRED] [semantically similar]
  docs/architecture.md → CLAUDE.md
- `application Crate (lib; use cases orchestrating domain objects; transaction boundaries live here)` --semantically_similar_to--> `application Crate (use cases and orchestration; depends only on domain)`  [INFERRED] [semantically similar]
  docs/architecture.md → CLAUDE.md
- `Minerva Project (goal/milestone-first project management for schools)` --semantically_similar_to--> `Minerva Project (open source school project-management platform)`  [INFERRED] [semantically similar]
  README.md → CLAUDE.md

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **DDD Layered Backend Crates (interface -> application -> domain; infrastructure implements ports)** — docs_architecture_domain, docs_architecture_application, docs_architecture_infrastructure, docs_architecture_interface [EXTRACTED 1.00]
- **Docker Compose Full-Stack Services (postgres, redis, rustfs, server, web)** — deploy_docker_compose_postgres, deploy_docker_compose_redis, deploy_docker_compose_rustfs, deploy_docker_compose_server, deploy_docker_compose_web [EXTRACTED 1.00]
- **Optional Redis/S3 Runtime Configuration (env vars with compose local-dev defaults)** — deploy_docker_compose_server, docs_architecture_optional_services_rule, claude_optional_services_env_config [INFERRED 0.85]

## Communities (19 total, 3 thin omitted)

### Community 0 - "TypeScript Compiler Config"
Cohesion: 0.14
Nodes (13): compilerOptions, allowJs, checkJs, esModuleInterop, forceConsistentCasingInFileNames, moduleResolution, resolveJsonModule, rewriteRelativeImportExtensions (+5 more)

### Community 1 - "Frontend Tooling Config"
Cohesion: 0.17
Nodes (11): name, private, type, version, svelte, svelte-check, @sveltejs/adapter-node, @sveltejs/kit (+3 more)

### Community 2 - "Claude Project Guidance"
Cohesion: 0.18
Nodes (13): CLAUDE.md Project Guidance, API Host Port Mapping 3010:8080 (container-internal port stays 8080 via PORT), application Crate (use cases and orchestration; depends only on domain), apps/web (SvelteKit + TypeScript frontend), deploy (docker-compose + Dockerfiles), docs (architecture notes), domain Crate (core business concepts/invariants, pure logic, zero external dependencies), Gluetun media stack occupies host port 8080 (also 6881/7878/8989/9696 held by qbittorrent/sonarr/radarr/prowlarr) — reason new services avoid those ports (+5 more)

### Community 3 - "Project Overview & Identity"
Cohesion: 0.20
Nodes (12): Elastic License 2.0 (planned license; LICENSE is a placeholder), Goal/Milestone-First Organization (design principle: start from goals, break into milestones, not a flat task list), GET /health (only backend endpoint at scaffolding stage), Minerva Project (open source school project-management platform), docs/architecture.md (backend DDD architecture), Diesel Migrations (server/migrations/, empty until first schema change; generated via diesel CLI with DATABASE_URL set), README.md (project overview), SvelteKit (TypeScript) Frontend in apps/web (+4 more)

### Community 4 - "Docker Compose Dev Stack"
Cohesion: 0.25
Nodes (11): Redis/S3 optional at runtime; all config (REDIS_URL, S3_*) read from env vars with local-dev defaults in deploy/docker-compose.yml, never hardcoded, docker-compose.yml (full local dev stack: minerva), Postgres Service (postgres:16-alpine, user/password/db minerva, port 5432, healthcheck pg_isready), Redis Service (redis:7-alpine, port 6379, healthcheck ping), RustFS Service (S3-compatible placeholder object storage for local dev; S3 API :9000, dashboard :9001), Minerva API Server Service (built from deploy/docker/server.Dockerfile; PORT 8080, DATABASE_URL/REDIS_URL/S3_* env vars; host port 3010:8080), Web App Service (built from deploy/docker/web.Dockerfile; PORT 3000), Optional Redis/S3 at runtime (config REDIS_URL/S3_* from env vars; local-dev defaults in deploy/docker-compose.yml; never hardcoded) (+3 more)

### Community 5 - "Frontend Dev Dependencies"
Cohesion: 0.25
Nodes (8): devDependencies, svelte, svelte-check, @sveltejs/adapter-node, @sveltejs/kit, @sveltejs/vite-plugin-svelte, typescript, vite

### Community 6 - "DDD Architecture Rules"
Cohesion: 0.50
Nodes (8): application Crate (lib; use cases orchestrating domain objects; transaction boundaries live here), DDD Layered Cargo Workspace (four crates: domain/application/infrastructure/interface), domain Crate (lib; core business concepts/invariants such as goals and milestones; pure logic, no I/O, zero external dependencies), HTTP Isolation Rule (only interface may depend on Actix-web; HTTP types never leak into other crates), infrastructure Crate (lib; adapters for Postgres/Diesel, Redis, S3 object_store; config from env vars), interface Crate (bin; Actix-web HTTP API server; routing/request mapping; composition root wiring crates at startup), Inward Dependency Rule (dependencies point inward toward domain; domain depends on nothing), Ports/Traits Adapter Pattern (infrastructure implements storage/cache traits declared by inner layers, swapped in at startup from the interface composition root)

### Community 7 - "Web NPM Scripts"
Cohesion: 0.29
Nodes (7): scripts, build, check, check:watch, dev, prepare, preview

### Community 8 - "SvelteKit Scaffold & Bun"
Cohesion: 0.40
Nodes (5): apps/web README (sv scaffold documentation), Bun (dependency install and dev/build scripts for apps/web), Svelte CLI 'sv' (project created with template minimal, types ts, install bun), SvelteKit App Shell (app.html with %sveltekit.head% / %sveltekit.body% placeholders, hover preload), Bun (frontend package manager; lockfile apps/web/bun.lock)

### Community 9 - "Health Endpoint Handler"
Cohesion: 0.40
Nodes (4): HttpResponse, Result, health(), main()

### Community 10 - "Cargo Workspace Members"
Cohesion: 0.67
Nodes (4): application, domain, infrastructure, interface

## Knowledge Gaps
- **48 isolated node(s):** `name`, `private`, `version`, `type`, `dev` (+43 more)
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 59 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **3 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `CLAUDE.md Project Guidance` connect `Claude Project Guidance` to `SvelteKit Scaffold & Bun`, `Project Overview & Identity`, `Docker Compose Dev Stack`?**
  _High betweenness centrality (0.111) - this node is a cross-community bridge._
- **Why does `README.md (project overview)` connect `Project Overview & Identity` to `Docker Compose Dev Stack`?**
  _High betweenness centrality (0.045) - this node is a cross-community bridge._
- **Why does `docs/architecture.md (backend DDD architecture)` connect `Project Overview & Identity` to `Claude Project Guidance`, `DDD Architecture Rules`?**
  _High betweenness centrality (0.044) - this node is a cross-community bridge._
- **What connects `name`, `private`, `version` to the rest of the system?**
  _48 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `TypeScript Compiler Config` be split into smaller, more focused modules?**
  _Cohesion score 0.14285714285714285 - nodes in this community are weakly interconnected._