<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img alt="Minerva: open source project management for schools and non-profits" src="docs/assets/banner-light.svg">
</picture>

---
Minerva is an open source project management platform built for schools. It
is goal/milestone-first: teams start from what they are trying to achieve,
break it into milestones, and track progress against those — rather than
managing a flat list of tasks.

Scope and progress live in [docs/roadmap.md](docs/roadmap.md), the project's
checklist and source of truth for what is planned.

## Tech stack

- **Frontend:** SvelteKit (TypeScript) — `apps/web`
- **Backend:** Rust with Actix-web — `server`
- **Database:** PostgreSQL via Diesel — migrations in `server/migrations`
- **Cache / optional services:** Redis
- **Object storage:** RustFS (S3-compatible) runs in the dev stack; server-side integration is roadmap item 7.1

See [docs/architecture.md](docs/architecture.md) for the DDD layering of the
backend.

## Repository layout

```
apps/web/            SvelteKit frontend
server/              Rust Cargo workspace
  domain/            core business logic (no I/O or framework deps)
  application/       use cases
  infrastructure/    Postgres / Redis adapters (S3: roadmap 7.1)
  interface/         Actix-web HTTP API (bin: minerva-server)
  migrations/        Diesel migrations
deploy/              docker-compose.yml + Dockerfiles
docs/                architecture notes
```

## Local development

Prerequisites: Docker with the Compose plugin. To work on the frontend or
backend directly instead of via containers, also install Bun and a stable
Rust toolchain.

### Full stack

```sh
docker compose -f deploy/docker-compose.yml up --build
```

This starts Postgres, Redis, RustFS (S3-compatible storage), the API server,
and the web app:

| Service  | URL                          |
|----------|------------------------------|
| Web      | http://localhost:3000        |
| API      | http://localhost:3010/health |
| Postgres | localhost:5432 (user/password/db: `minerva`) |
| Redis    | localhost:6379               |
| RustFS   | S3 API http://localhost:9000, dashboard http://localhost:9001 |

The compose stack reads a `.env` file at the **repo root** holding only the
variables the compose file itself substitutes (`DATABASE_URL`, `PORT`,
`REDIS_URL`). Copy [`.env.example`](.env.example) and pass it explicitly —
Compose resolves `.env` relative to the compose file's directory (`deploy/`),
not the repo root:

```sh
cp .env.example .env
docker compose --env-file .env -f deploy/docker-compose.yml up --build
```

Every value is optional: the compose file falls back to built-in dev
defaults. Everything else the server reads — OIDC sign-in, cookies,
migrations — is configured as described in [Configuration](#configuration).

### Frontend only

```sh
cd apps/web
bun install
bun run dev
```

Dev server at http://localhost:5173.

### Backend only

```sh
cd server
DATABASE_URL=postgresql://minerva:minerva@localhost:5432/minerva cargo run -p interface
```

The port is configurable via the `PORT` environment variable (default 8080).
Verify with `curl http://localhost:8080/health`.

> **First admin:** on a fresh database the server creates the configured
> bootstrap admin at startup — see [Configuration](#configuration), "First
> admin". The bootstrap only runs while no users exist, so for a database
> that already has accounts it never touches anything; to promote an
> existing account there, update it directly in Postgres:
>
> ```sql
> UPDATE users SET role = 'admin' WHERE email = 'you@example.com';
> ```
>
> Once an admin exists, roles and active state are managed through the
> `/api/users` endpoints (documented in Swagger UI at `/api-docs`) — no SQL
> needed.

To run the compiled binary directly instead, build it and point it at a
config file:

```sh
cargo build --release -p interface
./server/target/release/minerva-server --config minerva.toml
```

See [Configuration](#configuration) for what goes in that file.

## Configuration

The server reads its settings from one place only: the `interface` crate's
config module (`server/interface/src/config.rs`). Settings layer from lowest
to highest precedence:

1. built-in defaults,
2. an optional TOML file,
3. the legacy aliases `DATABASE_URL`, `REDIS_URL` and `PORT`,
4. `MINERVA_`-prefixed environment variables.

### Config file discovery

The server looks for a config file in this order:

1. `--config <path>` (or `--config=<path>`) on the command line — if the
   named file does not exist, startup fails;
2. the path in `MINERVA_CONFIG`;
3. `./minerva.toml` (relative to the working directory);
4. `/etc/minerva/minerva.toml`.

Locations 3 and 4 are optional: with no file at all the server runs on
defaults plus environment variables. The startup log says which file was
loaded (or that none was). [`minerva.example.toml`](minerva.example.toml) at
the repo root documents every key; copy it to `minerva.toml` and uncomment
what you need. Every key is optional, unknown keys are rejected by name, and
an empty or whitespace-only value counts as unset.

### Environment variables

Individual settings can also be set with a `MINERVA_` prefix: the section and
key are joined with `__`, e.g. `MINERVA_SERVER__PORT=9000` or
`MINERVA_OIDC__ISSUER_URL=https://auth.example.com`. Keys are
case-insensitive, and arrays arrive as JSON, e.g.
`MINERVA_OIDC__SCOPES='["openid","email"]'`. `MINERVA_CONFIG` names the
config file; it is not a setting.

The old variable names (`OIDC_*`, `COOKIE_SECURE`, `WEB_BASE_URL`,
`RUN_MIGRATIONS`) are no longer read. If any of them are present at startup,
the server prints one warning listing each with its replacement.

### Secrets

`database.url`, `redis.url`, `oidc.client_secret` and `oidc.state_secret` may
come from their key or from a `*_file` sibling holding a path to a file whose
trimmed contents are the value (the Docker/Kubernetes convention):

```toml
[database]
url_file = "/run/secrets/database-url"
```

Setting both is an error, as is an unreadable file (named in the error, never
its contents). Secrets are redacted in all logs and `Debug` output.

### First admin

On a fresh deployment the first account is created at startup: while the
database has no users at all, the server creates the configured admin once
and prints `created bootstrap admin account <email>`; on every later start
it prints `bootstrap admin skipped: users already exist` and changes
nothing. Set in `minerva.toml` (or via
`MINERVA_BOOTSTRAP__*` environment variables):

- `bootstrap.admin_email` — enables the bootstrap; blank disables it
- `bootstrap.admin_password` — at least 8 characters; belongs in the
  `MINERVA_BOOTSTRAP__ADMIN_PASSWORD` environment variable or
  `bootstrap.admin_password_file`, not a committed file
- `bootstrap.admin_display_name` — optional (default: "Administrator")

The bootstrap only takes effect while no users exist: it never resets an
existing password or modifies an existing account.

The bootstrap admin is created with `sso_role_exempt = true`, so SSO
group-to-role rules (roadmap 2.7) can never recompute or lock its role;
every other account starts with both flags false. If your database was
bootstrapped before that column existed, set the flag once by hand:
`UPDATE users SET sso_role_exempt = true WHERE email = '<admin email>';`

### Inviting people

There is no public signup: an administrator creates an invite through the API
(`POST /api/invites`) and shares the returned link with the invited email —
copied manually while SMTP delivery (roadmap 7.3) is not configured. The
invitee follows the link, sets a password and is signed in. The link's origin
comes from `server.web_base_url`; without it the link is a site-relative path.
The same API lists, revokes and re-issues invites, and an admin can issue a
password-reset link for an existing user (`POST /api/users/{id}/password-reset`).

### Enabling OIDC sign-in

The server can offer OIDC sign-in alongside email/password. It is disabled
while `oidc.issuer_url` is blank; to enable it, set in `minerva.toml` (or via
`MINERVA_OIDC__*` and `MINERVA_SERVER__WEB_BASE_URL`):

- `oidc.issuer_url` — enables OIDC; the base of your provider's
  `/.well-known/openid-configuration`
- `oidc.client_id` / `oidc.client_secret` — the application registered at
  your provider
- `oidc.redirect_url` — must be exactly `<API base URL>/api/auth/oidc/callback`
  and registered at the provider; for the dev compose stack that is
  `http://localhost:3010/api/auth/oidc/callback`
- `oidc.state_secret` — at least 32 bytes; generate with `openssl rand -hex 32`
- `oidc.groups_claim` — name of the ID-token claim holding the user's
  groups (default "groups"); read at SSO login for the group-to-role rules
  (roadmap 2.7)
- `server.web_base_url` — absolute public URL of the API origin, e.g.
  `http://localhost:3010` for the dev stack

The server fails at startup, naming every problem it finds, if you set
`oidc.issuer_url` but leave anything else required missing or invalid. It
works with any standards-compliant OIDC provider; it has been tested with
Authentik.

For the group-to-role rules (roadmap 2.7) to work, your identity provider
must release the groups claim in the ID token; depending on the provider
this may also mean adjusting `oidc.scopes` so the claim is included — check
your provider's documentation, as Minerva has not verified this against a
live IdP yet. While the claim is missing or malformed the server logs a
warning per login and proceeds with no groups.

### SSO group-to-role mapping

With OIDC enabled, administrators map IdP groups to Minerva roles through
the API: `POST /api/sso/group-rules` with `{"group_name": "teachers",
"role": "staff"}` (the same base lists rules and updates or deletes one by
id; all of it is admin-only). While at least one rule exists, every SSO
sign-in re-resolves the user's role from their groups: exact group-name
match, the least permissive matched role wins, and no match — or a missing
groups claim — gives Read-only. New SSO accounts are created with that role;
a pending invite for the same email is still consumed, but its role is
ignored while rules exist (it is honoured when none do). The bootstrap admin
is exempt from recomputation, and recomputation never demotes the last
active administrator — it logs a warning instead. Roles set this way are
marked "managed by SSO" and cannot be changed through `/api/users` while any
rule exists; delete every rule to hand-edit them again. Without any rule,
SSO sign-ins never touch roles.

### Rate limiting

Login and token-link requests are rate-limited per client IP (logins also per
IP-and-email pair) with fixed-window counters (roadmap 2.8). Counters live in
Redis when `redis.url` is configured, in Postgres otherwise; subjects are
hashed before they reach the store, and a failing counter store allows the
request rather than taking login down.

- `rate_limit.enabled` — default `true`; set `false` to disable all limits
- `rate_limit.client_ip_header` — default: none (the TCP peer address is
  used). Behind a reverse proxy, set it to the header carrying the real
  client address (e.g. `X-Forwarded-For`) or every user shares the proxy's
  own address and one blocked IP locks everyone out

## License

TBD — Minerva will be released under the Elastic License 2.0 (see
[LICENSE](LICENSE)).
