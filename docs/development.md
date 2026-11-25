# Developing justdeploy

## Development

You need Docker (Docker Desktop on Windows and macOS), Rust (stable), Node 24 and pnpm, and
`sqlx-cli` (`cargo install sqlx-cli --no-default-features --features postgres`).

```bash
# 1. Postgres (published on 127.0.0.1:5433) and Traefik (port 80), which sends request
#    traces to the API running on your machine
echo JD_OTLP_TRACES_URL=http://host.docker.internal:4318/v1/traces >> .env
docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d postgres traefik

# 2. The builder image, tagged :dev
docker build -t justdeploy-builder:dev builder

# 3. The API on :8080 (migrations run at startup)
cd api
cp .env.example .env            # then set JD_MASTER_KEY: openssl rand -base64 32
cargo run

# 4. The dashboard on :5173, proxying /api to :8080
cd web
pnpm install
pnpm dev
```

Open <http://localhost:5173>. Deployed services answer at `http://<service>-<project>.localhost`,
which browsers and curl resolve to your machine.

Checks, all run in CI:

```bash
cd api && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd web && pnpm typecheck && pnpm build
```

`sqlx` checks every query against the database at compile time. After changing a query or a
migration, refresh the offline cache that CI and the Docker build use, and commit it:

```bash
cd api && cargo sqlx prepare
```

To try the production image locally against the same database, build it, stop `cargo run`, and start
it with your development key in a `.env` at the repository root:

```bash
docker build -t ghcr.io/vedantnimbarte/justdeploy:latest .
docker tag justdeploy-builder:dev ghcr.io/vedantnimbarte/justdeploy-builder:latest
docker compose up -d justdeploy      # dashboard at http://localhost:8080
```

### Code map

| Path | What |
|---|---|
| `api/src/handlers/` | HTTP: sign-in and accounts, team and activity (with the middleware that records it), projects and services, streams (SSE logs, metrics, terminal), settings, GitHub, backups, templates, webhooks |
| `api/src/services.rs` | Projects, services, variables, domains, deployments |
| `api/src/queue/` | Job queue, the deploy pipeline (build, release, removal), maintenance loops and reconciler |
| `api/src/docker/` | Docker helpers, and `spec.rs`: the pure builder of container definitions and Traefik labels |
| `api/src/engines.rs` | Database engines: images, credentials, URLs, dump and restore commands |
| `api/src/backups.rs` | Destinations, schedules, backup and restore |
| `api/src/templates.rs` | Template catalog and placeholder wiring |
| `api/src/webhooks.rs` | Signature checks, push deploys, previews |
| `api/src/pitr.rs` | Point-in-time recovery: wal-g settings, base backups, restoring a moment into a new database |
| `api/src/logs.rs` | Kept logs: a follower per production workload, search, pruning |
| `api/src/uptime.rs` | Uptime checks, incidents, down and recovered notifications, status pages |
| `api/src/usage.rs` | Hourly usage rollups, volume sizes, the usage series |
| `api/src/runs.rs` | One-off and scheduled runs of a service's image |
| `api/src/notify.rs` | Notification channels and delivery to Slack, Discord, email or a webhook; the SMTP server |
| `api/src/data.rs` | Database copies, replacing data, PostgreSQL read replicas |
| `api/src/ha.rs` | Edge routing on every manager, the standby of justdeploy's database, `justdeploy promote` |
| `api/src/auth.rs`, `crypto.rs` | People and roles, sessions, API tokens, invites, two-factor, rate limit; AES-GCM secrets |
| `api/src/cluster.rs` | The Swarm: start-up, overlay networks, node agents, registry, placement, servers, autoscaling |
| `api/src/github.rs` | The GitHub App: manifest flow, installation tokens, repositories, app webhooks, commit statuses |
| `cli/src/main.rs` | `jd`: the command-line client over the API |
| `api/migrations/` | Schema |
| `postgres/` | The `justdeploy-postgres` image: `postgres:17-alpine` plus a static wal-g |
| `builder/` | The builder image: `build.sh` (clone, Dockerfile or Railpack) and `healthcheck.sh` |
| `web/src/` | The dashboard: `ui.tsx` (design system components), `api.ts` (typed client), `pages/` |
| `templates/catalog.json` | Templates |
| `examples/` | Source of the code templates |
| `deploy/install.sh` | Server installer |

### Design

The dashboard is deliberately quiet so that state stands out. Greys carry the structure, actions are
solid ink like the logo, and colour only ever means status: green running, amber building, red failed,
grey stopped, always paired with a word. The type is one family, Archivo, with page titles set wide
through its width axis; JetBrains Mono appears only for machine data such as commit hashes, addresses
and logs. A sidebar lists every project with a live status dot, so trouble shows from any page.
Light and dark themes follow the system setting, with a switch in the sidebar; logs and the terminal
stay dark in both. The tokens are in [`web/src/styles.css`](../web/src/styles.css).
Template cards show each technology's mark from [Simple Icons](https://simpleicons.org) (CC0), in its
brand colour; marks too dark to read on dark surfaces use the text colour instead.

---

## Repository layout

```
api/          Rust control plane (axum, sqlx, bollard)
web/          React dashboard (Vite, TypeScript, Tailwind, TanStack Query)
builder/      Builder image: git, Docker CLI + BuildKit, Railpack
postgres/     PostgreSQL image with wal-g, for point-in-time recovery
templates/    Template catalog
examples/     Example apps the code templates build
cli/          jd, the command-line client
deploy/       Installers (server and CLI)
docker-compose.yml       justdeploy + Postgres + Traefik
docker-compose.tls.yml   HTTPS overlay (Let's Encrypt)
docker-compose.dev.yml   Development overrides
Dockerfile               Production image (API + dashboard)
```

---

---

## Releasing

Pushing a version tag builds the three images for amd64 and arm64 and publishes them to GitHub
Container Registry as `ghcr.io/vedantnimbarte/justdeploy`, `ghcr.io/vedantnimbarte/justdeploy-builder`
and `ghcr.io/vedantnimbarte/justdeploy-postgres`, tagged with the version, `major.minor` and `latest`
(and `17`, the PostgreSQL major, for the last):

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The Git tag keeps its `v`, but the image tags drop it: `v0.1.0` publishes `0.1.0` and `0.1`, so
servers pin a release with `JD_VERSION=0.1.0`.

After the first release, make the three packages public in GitHub (*Packages → Package settings → Change
visibility*) so servers can pull them without logging in.
