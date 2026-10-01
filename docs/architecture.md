# Architecture

Minerva's backend is a Rust Cargo workspace (`server/`) organized around
domain-driven design in four crates. The frontend (`apps/web`, SvelteKit)
communicates with the backend over HTTP only.

## Layers

| Crate | Kind | Responsibility | May depend on |
|---|---|---|---|
| `domain` | lib | Core business concepts and invariants (goals, milestones, ...). Pure logic, no I/O. | nothing but the `uuid`, `chrono`, `serde` value-type exceptions |
| `application` | lib | Use cases: orchestrate domain objects to fulfill a request. Transaction boundaries live here. | `domain` |
| `infrastructure` | lib | Adapters for external systems: Postgres (Diesel), Redis; S3 (`object_store`) declared but not yet implemented (roadmap 7.1). Configuration is read from environment variables. | `domain`, `application` (ports) |
| `interface` | bin | HTTP API server (Actix-web). Routing and request/response mapping; the composition root that wires the other crates together at startup. | all of the above |

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
- Redis is optional at runtime; S3 support lands with roadmap 7.1. Their
  configuration (`REDIS_URL`, `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`,
  `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`) comes from environment
  variables, with local-dev defaults provided in `deploy/docker-compose.yml`
  — never hardcoded in Rust source.

## Database migrations

Diesel migrations live in `server/migrations/` and are embedded in the
binary; the server applies any pending ones at startup (skip with
`RUN_MIGRATIONS=false`, e.g. when a dedicated migration job owns the schema).
Concurrent startups are safe — application is serialized on a Postgres
advisory lock. Generate a new migration using the diesel CLI:

```sh
cd server
diesel migration generate add_first_table
```

## Sessions and authentication

Email/password sign-in issues a cookie session (`minerva_session`); the
server stores only the SHA-256 hash of the token, never the raw value.
Sessions sit behind the `SessionRepository` port: Redis when `REDIS_URL` is
set, Postgres otherwise. OIDC sign-in sits behind the `OidcProvider` port
(`OpenIdConnectProvider` implements it) and is fully disabled unless
`OIDC_ISSUER_URL` is set — see `.env.example` for all the variables.

## API documentation

The `interface` crate generates an OpenAPI 3 document from code annotations
(utoipa) and serves it alongside a Swagger UI, both outside the `/api` scope:

- Swagger UI: http://localhost:8080/api-docs/swagger-ui/
- Raw OpenAPI JSON: http://localhost:8080/api-docs/openapi.json

Under Docker compose the API is mapped to host port 3010, so use
http://localhost:3010/api-docs/swagger-ui/ there. All `/api/*` endpoints
(auth, goals, milestones, tasks) are documented; the temporary `/debug/*`
routes are not.
