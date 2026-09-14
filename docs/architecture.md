# Architecture

Minerva's backend is a Rust Cargo workspace (`server/`) organized around
domain-driven design in four crates. The frontend (`apps/web`, SvelteKit)
communicates with the backend over HTTP only.

## Layers

| Crate | Kind | Responsibility | May depend on |
|---|---|---|---|
| `domain` | lib | Core business concepts and invariants (goals, milestones, ...). Pure logic, no I/O. | nothing — zero external dependencies |
| `application` | lib | Use cases: orchestrate domain objects to fulfill a request. Transaction boundaries live here. | `domain` |
| `infrastructure` | lib | Adapters for external systems: Postgres (Diesel), Redis, S3-compatible object storage (`object_store`). All configuration is read from environment variables. | `domain`, `application` (ports) |
| `interface` | bin | HTTP API server (Actix-web). Routing and request/response mapping; the composition root that wires the other crates together at startup. | all of the above |

## Dependency direction

```
            interface ──────────────> application ──────> domain
                 │                          ^                ^
                 └────────> infrastructure ─┴────────────────┘
                    (implements ports/traits declared by inner layers)
```

Rules:

- Dependencies point inward, toward `domain`. `domain` depends on nothing.
- Only `interface` may depend on Actix-web; HTTP types never leak into the
  other crates.
- `infrastructure` implements the storage/cache traits (ports) declared by
  the inner layers and is swapped in at startup from the composition root in
  `interface`.
- Redis and S3 are optional at runtime. Their configuration (`REDIS_URL`,
  `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`,
  `S3_SECRET_ACCESS_KEY`) is read from environment variables, with local-dev
  defaults provided in `deploy/docker-compose.yml` — never hardcoded in Rust
  source.

## Database migrations

Diesel migrations live in `server/migrations/` (empty until the first schema
change). With `DATABASE_URL` set, generate a new migration using the diesel
CLI:

```sh
cd server
diesel migration generate add_first_table
```
