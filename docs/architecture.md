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

## Authorization

Authentication answers "who is this?"; authorization answers "may they do
this?". The policy lives in two layers:

- `domain::Role` (`admin`, `staff`, `read_only`) and `domain::Permission`
  (`ViewContent`, `EditContent`, `ManageUsers`), with an exhaustive,
  wildcard-free `Role::allows` matrix that lists every (role, permission)
  pair.
- `application::authz::authorize(user, permission)` — the single place an
  access decision is made. Future checks (deactivated accounts, ownership)
  land here, so no handler or extractor compares roles itself.

HTTP-level enforcement is three request extractors in
`interface/src/access.rs`, each wrapping the `AuthenticatedUser` session
extractor and asking `authz` for its permission:

| Extractor | Permission | Used by |
|---|---|---|
| `ViewAccess` | `ViewContent` | the GET goal/milestone/task routes |
| `EditAccess` | `EditContent` | the POST/PUT/DELETE goal/milestone/task routes |
| `AdminAccess` | `ManageUsers` | the temporary `/debug/*` routes (until 3.4/3.6 replace them) |

A missing or invalid session is a 401; a valid session whose role lacks the
permission is a 403 with the standard error envelope (`forbidden`). Handlers
declare their required level in their signature — no inline role checks.
`GET /api/auth/me` requires only a session (any role); signup, login,
logout, providers, the redirect flow, `/health` and the API docs stay
public. The public list is an explicit allowlist in
`interface/src/access_tests.rs`: adding a route without an extractor or an
allowlist entry fails the tests.

Roles are stored on `users.role`; a change takes effect on the next request,
because every authenticated request re-resolves the user row. Until the
Users API exists (roadmap 2.4), roles are changed directly in the database.

## API documentation

The `interface` crate generates an OpenAPI 3 document from code annotations
(utoipa) and serves it alongside a Swagger UI, both outside the `/api` scope:

- Swagger UI: http://localhost:8080/api-docs/swagger-ui/
- Raw OpenAPI JSON: http://localhost:8080/api-docs/openapi.json

Under Docker compose the API is mapped to host port 3010, so use
http://localhost:3010/api-docs/swagger-ui/ there. All `/api/*` endpoints
(auth, goals, milestones, tasks) are documented; the temporary `/debug/*`
routes are not.
