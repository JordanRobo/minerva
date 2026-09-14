# Minerva

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

## License

TBD — Minerva will be released under the Elastic License 2.0 (see
[LICENSE](LICENSE)).
