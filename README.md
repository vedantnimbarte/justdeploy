<img src="web/public/logo-full.svg" alt="justdeploy" height="56">

# justdeploy

A self-hosted platform for deploying apps, backends and databases on one server with plain Docker.
Push to Git or pick a template, and get a built, routed, TLS-terminated service, without
Kubernetes.

- **Frontends and backends from Git.** A Dockerfile is used when the repository has one; otherwise
  [Railpack](https://railpack.com) detects Node, Vite, Next.js, Python, Go, Rust, static sites and more.
- **Databases in one click.** PostgreSQL, MySQL, MariaDB, Redis and MongoDB, each with a persistent
  volume, generated credentials and a ready-made connection URL.
- **Any Docker image.** Self-hosted tools like Uptime Kuma, n8n or Umami run as-is.
- **Zero-downtime deploys.** The new container must pass a health check before it takes traffic,
  and a failed deploy leaves the old one serving.
- **Push to deploy, pull request previews, one-click rollback.**
- **Backups** of every database engine to S3-compatible storage or a local folder, on a schedule,
  with restore.
- **Logs, metrics and a web terminal** for every service, and an **Analytics** page: server CPU, memory, disk and network, all apps together, activity, Docker's disk usage,
  and warnings when something needs attention.
- **Templates** for 18 stacks, from a static site to FastAPI + PostgreSQL.

justdeploy is MIT licensed and is meant for you and your team on your own server, with roles for
who may do what, not for hosting strangers.

---

## Contents

- [How it works](#how-it-works)
- [Install on a server](#install-on-a-server)
- [First steps](#first-steps)
- [Projects and services](#projects-and-services)
- [Builds](#builds)
- [Deploys, health checks and rollback](#deploys-health-checks-and-rollback)
- [Push to deploy and pull request previews](#push-to-deploy-and-pull-request-previews)
- [Environment variables](#environment-variables)
- [Domains and HTTPS](#domains-and-https)
- [Databases](#databases)
- [Backups and restore](#backups-and-restore)
- [Runs and cron jobs](#runs-and-cron-jobs)
- [Logs, metrics and the terminal](#logs-metrics-and-the-terminal)
- [Team, roles and access](#team-roles-and-access)
- [Notifications](#notifications)
- [System overview](#system-overview)
- [Templates](#templates)
- [Configuration reference](#configuration-reference)
- [HTTP API](#http-api)
- [Security](#security)
- [Upgrading and backing up justdeploy itself](#upgrading-and-backing-up-justdeploy-itself)
- [Development](#development)
- [Repository layout](#repository-layout)
- [Known limits](#known-limits)
- [Releasing](#releasing)

---

## How it works

```
                 ┌──────────────── your server ─────────────────────────────────────┐
  browser ──────▶│ :8080  justdeploy (Rust API + dashboard) ── Postgres (its state)    │
                 │            │                                                       │
                 │            │ Docker API (/var/run/docker.sock)                     │
                 │            ▼                                                       │
  visitors ─────▶│ :80/:443  Traefik ──▶ app containers   (network jd-proxy)         │
                 │                        │                                           │
                 │                        └─ project network jd-proj-…               │
                 │                             api ◀──▶ db ◀──▶ cache                 │
                 └───────────────────────────────────────────────────────────────────┘
```

Three containers make up justdeploy itself, all in one `docker-compose.yml`:

| Container | Role |
|---|---|
| `justdeploy` | The control plane: a Rust (axum) API that also serves the React dashboard. It talks to the host's Docker daemon to build and run everything. |
| `postgres` | justdeploy's own records: projects, services, deployments, encrypted secrets, backups, metrics. |
| `traefik` | The reverse proxy. justdeploy puts labels on each container; Traefik routes hostnames to them and gets Let's Encrypt certificates. |

Builds run in short-lived **builder** containers (`justdeploy-builder`: git, the Docker CLI with
BuildKit, and Railpack). Built images stay in the server's local Docker image store; no registry is
involved.

Work that takes time (builds, releases, removals, backups) goes through a job queue in Postgres, so
it survives a restart. A reconciler runs every minute and brings Docker back in line with the
database: it restarts stopped containers, redeploys a service whose container was removed by hand,
and removes containers that belong to nothing.

---

## Install on a server

You need a Linux server (x86-64 or arm64) with [Docker Engine and the Compose plugin](https://docs.docker.com/engine/install/),
and ports 80 and 8080 open (443 too for HTTPS).

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh | sh
```

The script installs into `/opt/justdeploy`: it downloads the compose files, writes a `.env` with a
freshly generated master key and database password, pulls the images and starts everything. Then open
`http://<server-ip>:8080` and create the owner account. Invite the rest of your team under **Team**.

> The images are published to GitHub Container Registry when a version is tagged (see
> [Releasing](#releasing)). Until the first release exists, build them on the server instead:
> clone the repository, then `docker build -t ghcr.io/vedantnimbarte/justdeploy:latest .` and
> `docker build -t ghcr.io/vedantnimbarte/justdeploy-builder:latest builder`, and run
> `docker compose up -d`.

### Choosing a domain

Every public service gets a hostname of the form `service-project.<base domain>`. Point a wildcard
DNS record at the server:

```
*.apps.example.com.   A   203.0.113.10
```

and install with that base domain:

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh | JD_BASE_DOMAIN=apps.example.com sh
```

Without a domain, the installer uses `<server-ip>.sslip.io`, a public wildcard DNS service that
resolves any `*.1.2.3.4.sslip.io` name to `1.2.3.4`, so routing works immediately. You can change the
base domain later under **Settings**; public services redeploy onto the new hostnames.

### HTTPS

Set an email for Let's Encrypt, and optionally a hostname for the dashboard itself:

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh \
  | JD_BASE_DOMAIN=apps.example.com JD_ACME_EMAIL=you@example.com JD_DASHBOARD_DOMAIN=deploy.example.com sh
```

With `JD_ACME_EMAIL` set, the installer adds `docker-compose.tls.yml`: Traefik listens on 443,
redirects HTTP to HTTPS, and issues a certificate per hostname over the HTTP-01 challenge, so port 80
must stay reachable. Once the dashboard is served at `JD_DASHBOARD_DOMAIN`, you can stop publishing
port 8080 by setting `JD_PORT=127.0.0.1:8080` in `.env`.

To switch an existing install to HTTPS, add `JD_ACME_EMAIL` (and `JD_DASHBOARD_DOMAIN`) to
`/opt/justdeploy/.env`, then:

```bash
cd /opt/justdeploy && docker compose -f docker-compose.yml -f docker-compose.tls.yml up -d
```

and redeploy your public services so they pick up the HTTPS routes.

### Forgot the password

```bash
cd /opt/justdeploy && docker compose exec justdeploy justdeploy reset-password
```

prints a new random password for the first owner, turns off their two-factor sign-in and signs their
sessions out. Add an email (`reset-password sam@example.com`) to reset someone else.

---

## First steps

1. **Deploy a template.** Open **Templates**, pick *FastAPI + PostgreSQL*, and deploy it into a new
   project. Within a minute or two the project shows a database and an API, and the API is live at
   `api-<project>.<base domain>`.
2. **Deploy your own app.** Create a project, choose **Add service → Git repository**, paste the
   HTTPS clone URL, and set the port your app listens on. justdeploy also sets `PORT` to it, which most
   frameworks read.
3. **Add a database** to the same project with **Add service → Database**, copy its connection URL from
   the service page, and add it to your app as `DATABASE_URL` under **Variables**.

---

## Projects and services

A **project** is a private Docker network. Every service in it is reachable from the others by its
name: an `api` service connects to a `db` service at `db:5432`. Services in different projects cannot
see each other.

A **service** is one of three kinds:

| Kind | Source | Notes |
|---|---|---|
| Git repository | A branch of a repository, optionally a subdirectory | Built on every deploy |
| Docker image | Any image reference, e.g. `nginx:alpine` | Pulled on every deploy |
| Database | An engine | Credentials generated, data on a volume |

Per service you can set:

- **Port.** What the app listens on. A service with no port is a **worker**: it gets no route and no
  port check, and is considered healthy if it is still running five seconds after start.
- **Public.** Route a hostname to it through Traefik. Private services are only reachable inside
  their project.
- **Health check path.** Optional, e.g. `/health`; without it an open port counts as healthy.
- **CPU and memory limits** (millicores and MB).
- **Volumes**, for image services that keep data (`/app/data`, for example). Databases always have one.
- **Host port**, to publish the service's port on the server itself, typically to reach a database
  from outside.
- **How it runs.** *Always on* (the default), or *On a schedule*: a cron job that starts on each
  scheduled time and exits. See [Runs and cron jobs](#runs-and-cron-jobs).
- **Pre-deploy command**, run before each release, typically database migrations. See below.

Status on the dashboard is one of *Running*, *Scheduled* (a cron job with a deployed image),
*Deploying*, *Crashed* (the container exists but is not running), *Failed* (nothing has deployed
successfully), *Stopped* or *Not deployed*.

---

## Builds

For a Git service, a builder container clones the branch at the exact commit, changes into the root
directory, and builds:

- **Dockerfile** when *Build with* is Auto and a `Dockerfile` exists (the path is configurable), or when
  *Build with* is Dockerfile.
- **Railpack** otherwise. It inspects the code and produces an image with the right runtime, install
  step, build step and start command. Single-page apps (Vite, CRA) and static sites are served by
  Caddy; servers start with the framework's usual command and listen on `$PORT`.

The build log streams live to the **Deployments** tab and is kept for 30 days.

**Build-time variables.** All of a service's variables are available during the build: as
`--build-arg` for Dockerfiles (declare them with `ARG`), and as build secrets for Railpack, which
rebuilds the affected layers when their values change. This is how a Vite app gets its `VITE_*`
values.

**Private repositories.** Add an access token (a GitHub fine-grained token with read access to
contents, or a GitLab token with `read_repository`). It is stored encrypted and passed to git as an
HTTP header, so it never appears in a URL or a log.

**Monorepos.** Set *Root directory* to the app's folder, e.g. `apps/web`.

---

## Deploys, health checks and rollback

A deploy builds (or pulls) the image, then releases it:

1. The new container starts beside the old one, on the project network and, if public, behind Traefik.
2. A health check runs from inside the project network: the port must accept connections, or the
   health check path must answer with 2xx/3xx, within two minutes.
3. Each deployment has its own Traefik router, ranked by deploy time, so the new container takes all
   new requests as soon as Traefik sees it. The old container is then stopped gracefully, and Traefik's
   retry middleware sends any refused request on to the new one.
4. If the health check fails, the new container is removed and the old one keeps serving. The
   deployment is marked failed with the container's last output in its log.

Tested by polling a service continuously through several redeploys, including one that added a
domain: no request failed.

**Services that cannot run twice.** A service with a volume or a host port (every database, for
instance) is stopped before the new container starts, so there is a short gap. If the new one fails,
the old one is started again.

**Redeploys without a build.** *Restart*, changing variables, adding a domain and most settings
re-release the current image. Changing the source (repository, branch, root directory, build
settings, image) triggers a full build. A change made while a deploy is still building is picked up by
that deploy.

**Rollback.** Any earlier successful production deployment has a *Roll back to this* button, which
releases its image again without rebuilding. The five most recent built images per service are kept;
rolling back further rebuilds that exact commit.

**Pre-deploy command.** When set (for example `npm run db:migrate`), it runs once in a container from
the new image, with the service's variables and on its project network, before anything is released.
Its output is in the deployment log. If it exits non-zero the deployment fails and the current version
keeps running. It is skipped for previews, which share the production database.

**Several deploys at once.** Up to `JD_DEPLOY_WORKERS` deploys (default 2) run at the same time, each
for a different service. Deploys of the same service always run one at a time, in order.

---

## Push to deploy and pull request previews

**With the GitHub App (recommended for GitHub).** Under **Settings → GitHub**, *Create GitHub App*
sends you to GitHub with everything filled in; confirm, then install the app on the accounts and
repositories you deploy. From then on:

- **Add a service** offers a repository picker, and private repositories clone without a token.
- Every push to a service's branch deploys it, and pull requests build previews when they are on for
  that service; no per-service webhook is needed. This applies to existing services built from those
  repositories too.
- Each deploy marks its commit with a status (*justdeploy/web*, or *justdeploy/web preview*) that links
  to the preview or the service.

GitHub has to reach this server, so `JD_PUBLIC_URL` must be its public address. Disconnecting the app
in Settings forgets it here; delete it on GitHub as well if you no longer need it.

**Per-service webhooks** work for GitHub and GitLab without the app. Each Git service has its own
webhook URL and secret, shown under **Settings → Automatic deploys**. A commit delivered by both the
app and a webhook deploys once.

**GitHub:** in the repository's *Settings → Webhooks → Add webhook*, use the GitHub URL, content type
`application/json`, the secret, and the *push* event (add *pull requests* for previews).
**GitLab:** *Settings → Webhooks*, the GitLab URL, the secret as *Secret token*, and *Push events*.

A push to the tracked branch deploys that commit. Pushes to other branches are ignored. A request
whose signature does not match is refused with 403.

**Previews (GitHub).** Turn on *Preview deploys for pull requests* for the service. Each opened or
updated pull request is built and served at `pr-<number>-<service>-<project>.<base domain>`, including
pull requests from forks; closing it removes the container and its images. Previews use the service's
variables, **so they talk to the same database as production**. Keep that in mind before turning
them on for an app that writes data.

The webhook URL uses `JD_PUBLIC_URL`, which must be reachable from GitHub or GitLab.

---

## Environment variables

Under **Variables**: add them one by one, or paste a `.env` file. Values are encrypted at rest with
AES-256-GCM under `JD_MASTER_KEY`. Saving re-releases the current image with the new values; use
**Deploy latest** when a build step reads them. Names starting with `JD_` are reserved.

`PORT` is set to the service's port unless you set it yourself.

**Shared variables.** On a project's page, variables every service in the project gets, at build and run
time. A service's own variable with the same name wins. Saving redeploys the project's running services.

**References.** A value can point at another service in the project with `${{service.KEY}}`, filled in
each time a container starts or a build runs:

- `${{db.URL}}` is a database's connection URL;
- `${{api.HOST}}` and `${{api.PORT}}` are another service's address on the project network;
- `${{api.NAME}}` is any of its variables, shared ones included.

A reference to a service or variable that doesn't exist fails the deploy with a message naming it.
Changing the referenced service doesn't redeploy the ones that point at it; redeploy them to pick it up.

---

## Domains and HTTPS

Every public service answers on its generated hostname. Add custom domains under **Domains**: each one
shows the DNS record to set (a `CNAME` onto the generated hostname for subdomains, an `A` record for an
apex) and checks every 15 seconds until it resolves to this server, then shows **Connected**. Settings
does the same for the base domain's wildcard record. With HTTPS on, Traefik obtains a certificate the
first time the hostname is requested.

---

## Databases

| Engine | Image | Port | Credentials | Connection URL |
|---|---|---|---|---|
| PostgreSQL | `postgres:17-alpine` | 5432 | `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` | `postgresql://app:…@db:5432/app` |
| MySQL | `mysql:8.4` | 3306 | `MYSQL_ROOT_PASSWORD`, `MYSQL_USER`, `MYSQL_PASSWORD`, `MYSQL_DATABASE` | `mysql://app:…@db:3306/app` |
| MariaDB | `mariadb:11.8` | 3306 | `MARIADB_*` equivalents | `mysql://app:…@db:3306/app` |
| Redis | `redis:8-alpine` | 6379 | `REDIS_PASSWORD` | `redis://default:…@cache:6379` |
| MongoDB | `mongo:8.0` | 27017 | `MONGO_INITDB_ROOT_USERNAME`, `MONGO_INITDB_ROOT_PASSWORD` | `mongodb://app:…@mongo:27017/?authSource=admin` |

Credentials are generated when the database is created and stored as ordinary (encrypted) variables
on the service. The service page shows the connection URL for other services in the project. The
image tag can be changed under **Settings** (to pin a version, for example). Changing a password
variable after the database was initialised does not change the password inside the database.

Redis is configured for RDB snapshots (`--save 60 1`, no AOF): up to a minute of writes can be lost in
a crash, in exchange for single-file backups.

To reach a database from your laptop, set a **Host port** on it (e.g. 15432) and connect to
`server-ip:15432`, or deploy the **Adminer** template into the same project.

---

## Backups and restore

**Destinations** (Settings → Backup destinations) are either:

- **S3-compatible storage**: AWS S3, Cloudflare R2, Backblaze B2, MinIO, SeaweedFS, and so on. Leave
  the endpoint empty for AWS. Credentials are stored encrypted.
- **A folder on the server**, under the `backups` volume (`/backups` in the container). Copy it off the
  server too.

A destination is saved only after a test write succeeds.

**Backing up.** On a database's **Backups** tab, *Back up now*, or set a schedule with a standard
five-field cron expression in UTC (`0 3 * * *` is 03:00 daily) and how many backups to keep. Older
backups at that destination (manual ones included) are deleted from storage once a new one succeeds.

A backup runs the engine's own tool inside the database container (`pg_dump`, `mysqldump`,
`mariadb-dump`, `mongodump --archive`, or a Redis RDB snapshot), gzips the output in the control plane
and streams it to the destination, so it never has to fit in memory. Objects are named
`<prefix>/<project>/<service>/<timestamp>.<ext>.gz`.

**Restore.** Pick a backup and *Restore*. The file is copied into the database container and fed to
`psql`, `mysql`, `mariadb` or `mongorestore --drop`. For PostgreSQL and MySQL/MariaDB the dump drops
and recreates the tables it contains; for MongoDB the collections. For Redis, the server is stopped,
`dump.rdb` is replaced and the server started again. The outcome and any error are shown on the
backup and written to the audit log.

Every engine has been tested with a full round trip (write, back up, change, restore, verify) against
both an S3-compatible store and a local folder.

---

## Runs and cron jobs

The **Runs** tab of an app or image service starts a container from the image that is live now, with
the service's variables, on its project network, runs one command and exits: a seed script, a data
fix, a report. Each run keeps its status, exit code, duration and the last 2,000 lines of output, shown
once it finishes. Runs are stopped after 12 hours.

**Cron jobs.** Set *How it runs* to *On a schedule* with a five-field cron expression in UTC
(`0 3 * * *` is every day at 03:00). Nothing then stays up: a deploy builds or pulls the image (and
runs the pre-deploy command), and on each scheduled time the image's own command runs. A run is
skipped while the previous one is still going, and stopping the service pauses the schedule. **Run
now** starts one straight away. A scheduled service takes no traffic, so it can't be public, publish a
host port or have previews.

---

## Logs, metrics and the terminal

- **Logs** follows the live container's output (the last 500 lines, then live).
- **Metrics** charts CPU, memory (excluding reclaimable page cache) and network rate per service.
  Samples are taken every 30 seconds and kept for 24 hours.
- **Terminal** opens a shell (`bash` if present, otherwise `sh`) inside the running container. Each
  session is written to the audit log.

---

## Team, roles and access

The first account is the **owner**. Under **Team**, owners invite people: an invite is a one-time link
for one email address, valid for 7 days, that you send however you like (there is no email sending).
Each person has one role, everywhere:

| Role | Can |
|---|---|
| Viewer | See projects, services, deploys, logs, metrics and runs. No secrets: variables, connection URLs and webhook secrets are hidden. |
| Member | Everything a viewer can, plus deploy, change services, variables and domains, run commands and open terminals. |
| Admin | Everything a member can, plus delete projects and services, restore backups, and change settings, GitHub and notifications. |
| Owner | Everything an admin can, plus invite people, change their roles, turn off their two-factor sign-in and remove them. There is always at least one owner. |

**Account.** Each person changes their own password, turns on **two-factor sign-in** (any TOTP app:
scan the QR code or type the key), and creates **API tokens**. A token is sent as
`Authorization: Bearer jd_…`, acts with its person's role, is shown once and stored hashed, and shows
when it was last used. A lost phone: an owner turns two-factor off for that person under Team, or
`reset-password <email>` does it from the server.

**Activity.** Admins see every change made through the dashboard or the API under Team: who did what
to which service, project or person, plus sign-ins, terminal sessions and what justdeploy itself did
(finishing a deletion or a restore).

---

## Notifications

Under **Settings → Notifications**, add a Slack or Discord incoming webhook, or any URL. A channel
gets a message when a deploy fails (previews included), a scheduled run or command fails, a backup
fails, or a service crashes (at most one crash message per service an hour). Tick *Also tell me when
deploys succeed* for successful deploys too. A test message is sent when you add a channel, and the
channel is only saved if it arrives. Webhook URLs are stored encrypted. Any URL receives a JSON
`POST` of `{event, text, url}`, where `event` is `deploy.failed`, `deploy.succeeded`,
`service.crashed`, `run.failed` or `backup.failed`.

---

## System overview

The **Analytics** tab shows the state of the whole server:

- **Server:** CPU (percent of all cores), memory in use (reclaimable cache excluded), disk used and
  free on the filesystem Docker stores its data on, network in and out, and uptime.
- **Charts** over the last hour, 6 hours or 24 hours: the server's CPU and memory next to the same
  figures for all your services added together, and the server's network traffic.
- **Activity:** services running out of the total (with any crashed, failed, deploying or stopped),
  deploys today and how many failed, the build success rate over 7 days, and the last backup.
- **Docker disk usage:** space taken by images, build cache, volumes and containers, and how much of
  each is reclaimable. It is measured every ten minutes, because sizing every volume can take a while.
- **Heaviest services** by memory, with their CPU.
- **Warnings** when the disk is over 85% full (critical over 95%), memory is over 90% used, a service
  has crashed, or a backup failed in the last 24 hours. Each links to the place to look.

Server figures come from a small idle container named `jd-agent` (the builder image, on the host
network), which justdeploy reads `/proc` and `df` through every 30 seconds. justdeploy itself runs in
a container and could not see the server's network or disk otherwise. Samples are kept for 24 hours.
On Docker Desktop the figures describe Docker's virtual machine rather than your computer.

## Templates

| Category | Template | Services |
|---|---|---|
| Frontend | Vite + React | Railpack, served as static files |
| Frontend | Next.js | Railpack, server-rendered |
| Frontend | Static site | Railpack, Caddy |
| Backend | Express API | Railpack |
| Backend | FastAPI | Railpack |
| Backend | Go API | Dockerfile (distroless) |
| Backend | Rust + Axum | Dockerfile |
| Full-stack | FastAPI + PostgreSQL | todo API + database, wired by `DATABASE_URL` |
| Full-stack | Express + Redis | visit counter + Redis, wired by `REDIS_URL` |
| Self-hosted | Uptime Kuma | image + volume |
| Self-hosted | n8n | image + volume |
| Self-hosted | Umami | image + PostgreSQL |
| Self-hosted | Adminer | image; deploy into a project with a database |
| Database | PostgreSQL, MySQL, MariaDB, Redis, MongoDB | one database each |

Code templates build from the [`examples/`](examples) folder of this repository, so each one doubles as
a worked example. Templates can be deployed into a new project or added to an existing one.

### Adding a template

Templates live in [`templates/catalog.json`](templates/catalog.json) and are compiled into the binary.
An entry looks like this:

```json
{
  "id": "fastapi-postgres",
  "name": "FastAPI + PostgreSQL",
  "category": "fullstack",
  "description": "A todo API backed by PostgreSQL, connected through DATABASE_URL.",
  "services": [
    { "name": "db", "kind": "database", "engine": "postgres" },
    {
      "name": "api",
      "kind": "app",
      "root_dir": "examples/fastapi-postgres",
      "port": 8080,
      "public": true,
      "health_path": "/health",
      "env": { "DATABASE_URL": "${db.url}" }
    }
  ]
}
```

Service fields mirror the *Add service* form: `kind` (`app`, `image`, `database`), `root_dir` and
`build_type` for code, `image`, `engine`, `port`, `public`, `health_path`, `env` and `volumes`.
Values in `env` may use placeholders, resolved at deploy time:

| Placeholder | Becomes |
|---|---|
| `${random}` | a new 32-character secret |
| `${db.url}` | the connection URL of the template's service named `db` |
| `${db.POSTGRES_USER}` | one of that service's variables |

A placeholder may only refer to a service listed before it. `cargo test` checks the catalog. Code
templates build from `JD_TEMPLATES_REPO` at `JD_TEMPLATES_BRANCH`, so a fork can point them at itself.

---

## Configuration reference

The control plane reads its configuration from the environment (`.env` next to the compose file).

| Variable | Default | Meaning |
|---|---|---|
| `JD_MASTER_KEY` | *required* | Base64 of 32 random bytes (`openssl rand -base64 32`). Encrypts every stored secret. **If it changes or is lost, stored secrets cannot be read.** |
| `DATABASE_URL` | set by compose | Postgres connection string for justdeploy's own data |
| `JD_DB_PASSWORD` | `justdeploy` | Password of that Postgres (compose) |
| `JD_BASE_DOMAIN` | `localhost` | Initial base domain; editable in Settings afterwards |
| `JD_PUBLIC_URL` | `http://localhost:8080` | Where the dashboard is reached; used in webhook URLs |
| `JD_ACME_EMAIL` | unset | Enables HTTPS routes and secure cookies; use with `docker-compose.tls.yml` |
| `JD_DASHBOARD_DOMAIN` | unset | Hostname for the dashboard over HTTPS (TLS overlay) |
| `JD_PORT` | `8080` | Host port (or `ip:port`) the dashboard is published on |
| `JD_VERSION` | `latest` | Image tag of justdeploy to run |
| `JD_BUILDER_IMAGE` | `ghcr.io/vedantnimbarte/justdeploy-builder:latest` | Image used for builds and health checks |
| `JD_PROXY_NETWORK` | `jd-proxy` | Docker network Traefik watches |
| `JD_TEMPLATES_REPO` | this repository | Repository code templates build from |
| `JD_TEMPLATES_BRANCH` | `main` | Its branch |
| `JD_BACKUP_DIR` | `/backups` | Root of "folder on this server" backup destinations |
| `JD_DEPLOY_WORKERS` | `2` | Deploys (builds included) that run at once, each for a different service |
| `JD_BIND` | `0.0.0.0:8080` | Address the API listens on inside its container |
| `JD_WEB_DIR` | `/app/web` in the image | Built dashboard to serve; unset in development |
| `JD_COOKIE_SECURE` | `false` | Force `Secure` cookies (automatic when `JD_ACME_EMAIL` is set) |
| `RUST_LOG` | `justdeploy=info` | Log filter |

---

## HTTP API

The dashboard uses a JSON API under `/api/v1`, authenticated with the session cookie; scripts send an
API token as `Authorization: Bearer jd_…` instead. Each call needs a role (see
[Team, roles and access](#team-roles-and-access)); a missing one answers 403 naming the role. Webhooks
authenticate by signature.

| Method | Path | |
|---|---|---|
| `GET` | `/health` | Database and Docker reachability |
| `GET` | `/overview?minutes=60` | Server, apps-combined series, activity, Docker disk usage, warnings |
| `GET` | `/auth/status` | Whether setup is needed and whether signed in |
| `POST` | `/auth/setup` · `/auth/login` · `/auth/logout` | `{email, password, code?}`; 401 from login means the account needs its code |
| `GET` | `/auth/me` | `{id, email, role, two_factor}` |
| `PUT` | `/auth/password` | `{current, new}` |
| `POST` `DELETE` | `/auth/two-factor` | Start setup: `{secret, otpauth_url, qr_svg}` · turn off `{password}` |
| `POST` | `/auth/two-factor/confirm` | `{code}` |
| `GET` `POST` `DELETE` | `/auth/tokens`, `/auth/tokens/{id}` | Your API tokens · create `{name}` returns the value once |
| `GET` `POST` | `/auth/invites/{token}` | What an invite is for · accept it `{password}` |
| `GET` | `/users` | Everyone, with role and two-factor state |
| `PATCH` `DELETE` | `/users/{id}` | Owner: `{role?, reset_two_factor?}` · remove |
| `GET` `POST` `DELETE` | `/invites`, `/invites/{id}` | Owner: pending invites · create `{email, role}` returns the link once · revoke |
| `GET` | `/activity?before=id` | Admin: the 100 newest entries before `before` |
| `GET` `POST` | `/projects` | List (with services and status) · create `{name}` |
| `GET` `DELETE` | `/projects/{id}` | |
| `POST` | `/projects/{id}/services` | Create a service and deploy it |
| `GET` `PUT` | `/projects/{id}/env` | Shared variables, `[{key, value}]`, replaced as a whole |
| `GET` `PATCH` `DELETE` | `/services/{id}` | Detail · update settings (redeploys) · delete |
| `POST` | `/services/{id}/actions` | `{action: deploy | restart | stop | start}` |
| `GET` | `/services/{id}/deployments` | Last 50 deployments |
| `GET` `PUT` | `/services/{id}/env` | `[{key, value}]`, replaced as a whole |
| `POST` `DELETE` | `/services/{id}/domains`, `/services/{id}/domains/{domain_id}` | Custom domains |
| `GET` | `/dns-check?host=app.example.com` | Whether a hostname (or `*.wildcard`) resolves to this server |
| `GET` `POST` | `/services/{id}/runs` | `{next_run_at, runs}` · start a run `{command?}` (none: the image's own) |
| `GET` | `/services/{id}/logs` | Runtime output (server-sent events) |
| `GET` | `/services/{id}/metrics?minutes=60` | Samples |
| `GET` | `/services/{id}/terminal` | WebSocket: binary frames are terminal bytes, a text frame `{cols, rows}` resizes |
| `GET` `POST` | `/services/{id}/backups` | List · back up now `{destination_id}` |
| `GET` `PUT` | `/services/{id}/backup-schedule` | `{destination_id, cron, keep}` or `null` |
| `POST` | `/backups/{id}/restore` | |
| `GET` `POST` `DELETE` | `/backup-destinations`, `/backup-destinations/{id}` | |
| `GET` | `/deployments/{id}` · `/deployments/{id}/logs` | Build log as server-sent events; resumes from `Last-Event-ID` |
| `POST` | `/deployments/{id}/rollback` | |
| `GET` `POST` | `/templates` · `/templates/{id}/deploy` | `{project_name}` or `{project_id}` |
| `GET` `PUT` | `/settings` | `{base_domain}` |
| `GET` `POST` `DELETE` | `/notification-channels`, `/notification-channels/{id}` | `{name, kind: slack \| discord \| webhook, url, notify_success}`; sends a test first |
| `GET` | `/github` | The GitHub App and the accounts it's installed on |
| `POST` | `/github/manifest` | Admin: `{org?}` returns the form that creates the app on GitHub |
| `POST` `DELETE` | `/github/app` | Admin: finish creation `{code, state}` · forget the app |
| `GET` | `/github/repos` | Member: repositories the app can deploy |
| `POST` | `/webhooks/github/{service_id}` · `/webhooks/gitlab/{service_id}` · `/webhooks/github-app` | |

---

## Security

- **The Docker socket is root on the host.** justdeploy needs it to run containers, so anyone who can
  sign in to the dashboard (or run code in the control plane) controls the server. Use a strong
  password, keep the dashboard behind HTTPS, and do not expose port 8080 once a dashboard domain is set.
- **Accounts come from invites.** There is no signup. Login attempts (and one-time codes) are limited to
  10 per 15 minutes per email address. Sessions, API tokens and invite links are random tokens stored
  hashed; sessions last 30 days and end on sign-out, `reset-password` or removing the person.
  Two-factor secrets and the GitHub App's key are encrypted with `JD_MASTER_KEY`.
- **Roles limit the dashboard and API, not the server.** A member can open a terminal and run commands
  in containers, and builds see the Docker socket, so give the member role only to people you'd trust
  with the server.
- **Secrets are encrypted at rest** (AES-256-GCM): variables, git tokens, database credentials and
  backup credentials. The key is `JD_MASTER_KEY` in `.env`, which the installer creates with
  owner-only permissions.
- **Apps are isolated by network.** Each project has its own Docker network; only public services join
  the proxy network. justdeploy does not sandbox untrusted code: deploy only code you trust.
- **Webhooks** are verified with HMAC-SHA256 (GitHub, and the GitHub App with its own secret) or a
  constant-time token comparison (GitLab).
- Builds see the Docker socket (they build with the host's BuildKit), so a build can do anything Docker
  can. That is fine for your own repositories and the reason justdeploy is single-tenant.

Please don't open a public issue for a security problem; contact the maintainer privately through
GitHub first.

---

## Upgrading and backing up justdeploy itself

```bash
cd /opt/justdeploy
docker compose pull && docker compose up -d        # add -f docker-compose.tls.yml if you use HTTPS
```

Migrations run at startup. Deployments in progress during an upgrade resume when the control plane
comes back.

To back up justdeploy's own state, keep a copy of `.env` (above all `JD_MASTER_KEY`) and dump its
database:

```bash
docker compose exec postgres pg_dump -U justdeploy justdeploy | gzip > justdeploy-$(date +%F).sql.gz
```

Your apps' data lives in their database services and volumes; back those up with the Backups tab.

---

## Development

You need Docker (Docker Desktop on Windows and macOS), Rust (stable), Node 24 and pnpm, and
`sqlx-cli` (`cargo install sqlx-cli --no-default-features --features postgres`).

```bash
# 1. Postgres (published on 127.0.0.1:5433) and Traefik (port 80)
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
| `api/src/runs.rs` | One-off and scheduled runs of a service's image |
| `api/src/notify.rs` | Notification channels and delivery to Slack, Discord or a webhook |
| `api/src/auth.rs`, `crypto.rs` | People and roles, sessions, API tokens, invites, two-factor, rate limit; AES-GCM secrets |
| `api/src/github.rs` | The GitHub App: manifest flow, installation tokens, repositories, app webhooks, commit statuses |
| `api/migrations/` | Schema |
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
stay dark in both. The tokens are in [`web/src/styles.css`](web/src/styles.css).
Template cards show each technology's mark from [Simple Icons](https://simpleicons.org) (CC0), in its
brand colour; marks too dark to read on dark surfaces use the text colour instead.

---

## Repository layout

```
api/          Rust control plane (axum, sqlx, bollard)
web/          React dashboard (Vite, TypeScript, Tailwind, TanStack Query)
builder/      Builder image: git, Docker CLI + BuildKit, Railpack
templates/    Template catalog
examples/     Example apps the code templates build
deploy/       Installer
docker-compose.yml       justdeploy + Postgres + Traefik
docker-compose.tls.yml   HTTPS overlay (Let's Encrypt)
docker-compose.dev.yml   Development overrides
Dockerfile               Production image (API + dashboard)
```

---

## Known limits

- **One server.** justdeploy manages the Docker host it runs on. Remote hosts are not supported yet.
- **Roles are install-wide**: there is no per-project access yet.
- **No recovery codes** for two-factor sign-in; an owner or `reset-password` turns it off instead.
- **Run output** is saved when a run finishes, not streamed while it runs.
- **Previews share the production database** (they reuse the service's variables), and are GitHub only;
  GitLab merge requests do not get previews.
- **Not tested end to end:** real Let's Encrypt issuance (it needs public DNS and port 80; the
  development setup runs plain HTTP on `*.localhost`), webhooks delivered by GitHub or GitLab
  themselves (tested with correctly signed requests), and AWS S3 itself (tested with SeaweedFS's S3
  API, which uses the same client and request signing).
- **Release images for arm64** are built under emulation in CI, which is slow; the amd64 path is the
  one exercised in development.
- **Restores** handle backups up to 8 GiB compressed.
- **Redis** keeps up to a minute of writes at risk (RDB snapshots, no AOF).
- **Notifications** go to Slack, Discord or a webhook; there is no email.

---

## Releasing

Pushing a version tag builds both images for amd64 and arm64 and publishes them to GitHub Container
Registry as `ghcr.io/vedantnimbarte/justdeploy` and `ghcr.io/vedantnimbarte/justdeploy-builder`,
tagged with the version, `major.minor` and `latest`:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

After the first release, make both packages public in GitHub (*Packages → Package settings → Change
visibility*) so servers can pull them without logging in.

---

## License

[MIT](LICENSE)
