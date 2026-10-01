<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img alt="Minerva: open source project management for schools and non-profits" src="docs/assets/banner-light.svg">
</picture>

---
Minerva is an open source project management platform built for schools. It
is goal/milestone-first: teams start from what they are trying to achieve,
break it into milestones, and track progress against those — rather than
managing a flat list of tasks.

## Tech stack

- **Frontend:** SvelteKit (TypeScript) — `apps/web`
- **Backend:** Rust with Actix-web — `server`
- **Database:** PostgreSQL via Diesel — migrations in `server/migrations`
- **Cache / optional services:** Redis
- **Object storage:** any S3-compatible service (RustFS for local dev),
  accessed through a trait abstraction in the infrastructure layer

See [docs/architecture.md](docs/architecture.md) for the DDD layering of the
backend.

## Repository layout

```
apps/web/            SvelteKit frontend
server/              Rust Cargo workspace
  domain/            core business logic (no external deps)
  application/       use cases
  infrastructure/    Postgres / Redis / S3 adapters
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

Configuration lives in a `.env` file at the **repo root**. Copy
[`.env.example`](.env.example) and pass it explicitly — Compose resolves
`.env` relative to the compose file's directory (`deploy/`), not the repo
root:

```sh
cp .env.example .env
docker compose --env-file .env -f deploy/docker-compose.yml up --build
```

Every value in `.env` is optional for local dev: the compose file falls back
to built-in dev defaults, and OIDC sign-in stays off until you set
`OIDC_ISSUER_URL`.

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

### Optional: single sign-on (OIDC)

The server can offer OIDC sign-in alongside email/password. Set these in
`.env` — [`.env.example`](.env.example) lists them all, with defaults:

- `OIDC_ISSUER_URL` — enables OIDC; leave empty to keep it off
- `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` — the application registered at
  your provider
- `OIDC_REDIRECT_URL` — must be exactly `<API base URL>/api/auth/oidc/callback`
  and registered at the provider; for the dev compose stack that is
  `http://localhost:3010/api/auth/oidc/callback`
- `OIDC_STATE_SECRET` — at least 32 bytes; generate with `openssl rand -hex 32`
- `WEB_BASE_URL` — absolute public URL of the API origin, e.g.
  `http://localhost:3010` for the dev stack

The server fails at startup, naming the variable, if you set
`OIDC_ISSUER_URL` but forget one of the required ones. It works with any
standards-compliant OIDC provider; it has been tested with Authentik.

## License

TBD — Minerva will be released under the Elastic License 2.0 (see
[LICENSE](LICENSE)).
