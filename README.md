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
- [Environments](#environments)
- [Builds](#builds)
- [Deploys, health checks and rollback](#deploys-health-checks-and-rollback)
- [Push to deploy and pull request previews](#push-to-deploy-and-pull-request-previews)
- [Environment variables](#environment-variables)
- [Domains and HTTPS](#domains-and-https)
- [Databases](#databases)
- [Backups and restore](#backups-and-restore)
- [Runs and cron jobs](#runs-and-cron-jobs)
- [Logs, metrics and the terminal](#logs-metrics-and-the-terminal)
- [Usage, uptime and status pages](#usage-uptime-and-status-pages)
- [Team, roles and access](#team-roles-and-access)
- [Command line: jd](#command-line-jd)
- [Notifications](#notifications)
- [Servers and scaling](#servers-and-scaling)
- [When the main server goes down](#when-the-main-server-goes-down)
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
                 ┌──────────── main server (Swarm manager) ──────────────────────────┐
  browser ──────▶│ :8080  justdeploy (Rust API + dashboard) ── Postgres (its state)    │
                 │            │ Docker API                                            │
                 │            ▼                                                       │
  visitors ─────▶│ :80/:443  Traefik ──▶ copies of your services   (overlay jd-mesh)  │
                 └────────────┼──────────────────────┬────────────────────────────────┘
                              │ jd-mesh / project    │ jd-control (encrypted):
                              │ overlay networks     │ control plane ⇄ node agents
                 ┌────────────▼──────────────────────▼──── more servers (optional) ───┐
                 │  copies of your services             jd-node agent                  │
                 └─────────────────────────────────────────────────────────────────────┘
```

Every install is a **Docker Swarm**. A single server is a cluster of one, and more servers join with
one command (see [Servers and scaling](#servers-and-scaling)). Three containers make up justdeploy
itself, all in one `docker-compose.yml` on the main server:

| Container | Role |
|---|---|
| `justdeploy` | The control plane: a Rust (axum) API that also serves the React dashboard. It drives the Swarm through Docker's API to build and run everything. |
| `postgres` | justdeploy's own records: projects, services, deployments, encrypted secrets, backups, metrics. |
| `traefik` | The reverse proxy. Each service carries routing labels; Traefik routes hostnames to its copies on any server and gets Let's Encrypt certificates. |

Each deployment of a service runs as a Swarm service with one or more copies. Projects get their own
overlay network, so services reach each other by name wherever they run. Builds run on the main server
in short-lived **builder** containers (`justdeploy-builder`: git, the Docker CLI with BuildKit, and
Railpack). With more than one server and HTTPS on, built images go to a private registry so every
server can pull them; otherwise they stay on the main server, and so do the services built from them.

Work that takes time (builds, releases, removals, backups) goes through a job queue in Postgres, so
it survives a restart. Swarm restarts copies that exit and moves them off a server that goes down. A
reconciler runs every minute on top: it redeploys a service whose workload was removed by hand, scales
stopped services to nothing, and removes what belongs to nothing.

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

The project page shows services as a list or as a **canvas**: cards laid out so that each service sits
left of what it references, joined by arrows (hover one to see which variables make the link).

Status on the dashboard is one of *Running*, *Scheduled* (a cron job with a deployed image),
*Deploying*, *Crashed* (the container exists but is not running), *Failed* (nothing has deployed
successfully), *Stopped* or *Not deployed*.

---

## Environments

A project starts with one environment, **production**. *New environment* on the project page copies
it into, say, **staging**: every service with its settings and variables, and the shared variables,
into its own network with fresh, empty databases. Apps built from Git can track another branch there
(`develop`, for example). Not copied: data, domains, host ports, previews and backup schedules. The new
environment then deploys, and its services answer at `<service>-<project>-<environment>.<base domain>`.
Inside it, services reach each other by the same names as in production, so `db:5432` is staging's
database.

Each environment is its own project underneath (`shop-staging`), so everything else (variables,
domains, scheduled runs, roles) works the same. The switcher at the top of the project page moves
between them.

**Copy settings from…** brings an environment in line with another: settings (source, build, image,
port, health check, limits, schedule, pre-deploy command) are copied onto services of the same name,
services it doesn't have are added, and variables it doesn't have are added. Kept as they are:
branches, the values of its existing variables, domains, host ports, previews, data, and services only
it has. Services that change redeploy.

Deleting the production project deletes its environments too.

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
keeps running. A preview runs it against its own database copies (see below); a preview whose service
reaches its database by a fixed address rather than a reference skips it, since that address is
production's.

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
pull requests from forks; closing it removes the container and its images.

**Preview databases.** The first time a pull request deploys, it gets its own copy of each database in
the project: same image and credentials, its own volumes, filled from that database's newest
successful backup (or empty when there is none yet; Redis copies always start empty). The copy answers
on the project network as `<database>-pr-<number>`, and a preview's references point at it:
`${{db.URL}}` in a preview is the pull request's database. Closing the pull request deletes the
copies and their volumes. A value that names the database by a fixed address, like
`postgres://…@db:5432/app`, still reaches production's, so use references for anything a preview
writes to.

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

### Copies, read replicas and replacing data

A database's **Copies and replicas** tab has three tools:

- **Copy.** A new database beside this one (`<name>-copy`), with the same engine, image and
  credentials, filled with its data as it is now. Nothing links them afterwards: try a migration on
  the copy first. Any engine.
- **Read replicas** (PostgreSQL). `<name>-replica` follows every change through streaming
  replication, usually within a second, and refuses writes. It has its own connection URL, for
  reports and other read-heavy work. Before it first starts, a helper copies the primary with
  `pg_basebackup`; the primary gets a replication slot and a rule in `pg_hba.conf`, and keeps at most
  10 GB of changes for a replica that falls behind. The tab shows each replica's lag. Deleting a
  replica drops its slot; a primary with replicas can't be deleted until they are.
- **Replace its data** (admins). Fills the database with another one's from the same project, same
  engine: a dump of the other is restored here, so its tables (or collections) are dropped and
  recreated, and for Redis the whole dataset is swapped. This is how a
  [point-in-time](#point-in-time-recovery-postgresql) copy goes back over the original once you've
  checked it. Each copy's progress and outcome are listed, and written to the activity log.

Copies pass through a scratch file on the control plane, gzipped, so the main server needs room for
the compressed dump.

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

### Point-in-time recovery (PostgreSQL)

On a PostgreSQL database's **Backups** tab, *Point-in-time recovery* → pick an S3 destination →
*Turn on*. The database is redeployed on `justdeploy-postgres`, the official `postgres:17-alpine`
image with [wal-g](https://github.com/wal-g/wal-g) added, with `archive_mode` on: every change is
pushed to `<prefix>/pitr/<service id>` within a minute (`archive_timeout` is 60 seconds). A full base
backup is taken within ten minutes, then daily, and the last 7 are kept. The card shows when the last
change was archived, the last base backup, failed archive attempts, and the earliest moment you can
restore to.

**Restore to a moment.** Pick a date and time (at least a minute ago) and *Restore into a new
database*. A new service named `<name>-at-<MMDD-HHMM>` is created in the same project with the same
variables; before it starts, a helper container fetches the newest base backup from before that
moment, replays the archived changes up to it, and promotes it. The original database is untouched:
check the copy, then point your app at it or copy what you need back. Turning archiving off redeploys
the database on the stock image; what was archived stays in the bucket.

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

- **Logs** follows the live container's output (the last 500 lines, then live). *Search* looks
  through everything production services have printed, kept in justdeploy's Postgres for
  `JD_LOG_DAYS` (7) days: any text, case-insensitive, over the last hour to 30 days, newest first,
  200 lines at a time.
- **Metrics** charts CPU, memory (excluding reclaimable page cache) and network rate per service.
  Samples are taken every 30 seconds and kept for 24 hours.
- **Terminal** opens a shell (`bash` if present, otherwise `sh`) inside the running container. Each
  session is written to the audit log.

---

## Usage, uptime and status pages

**Usage.** A project's **Usage** tab charts CPU (100% is one core), memory and disk per service,
every copy added up, over the last day (15-minute steps), week or 90 days. Samples are rolled up into
hourly averages kept for 90 days; disk is the size of each service's volumes, measured hourly. A
table under the charts gives each service's averages for the range.

**Uptime.** Every public service is requested once a minute at its health check path (or `/`) on its
generated hostname. An answer below 500 within 10 seconds is up. Two misses in a row send
`service.down` to your notification channels, and the first success after that sends
`service.recovered`. The **Uptime** tab shows each service's state and response time, uptime over 24
hours, 7 and 90 days, a bar per day for 90 days, and recent incidents. Checks are kept 90 days.

**Status page.** On the same tab, *Publish the status page* and tick the services to show. The page
needs no sign-in and is served at `<public URL>/status/<project>`, at
`status-<project>.<base domain>`, and at a custom domain if you set one (add a CNAME pointing at the
`status-` hostname; the tab shows whether it's connected). It shows an overall state and each
service's 90 days, and refreshes itself every minute.

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
when it was last used.

Turning two-factor on shows ten **recovery codes** (`abcde-12345`) once: each signs in once in place
of the authenticator (*Lost your phone? Use a recovery code* on the sign-in screen). They're stored
hashed; the Account page shows how many are left and makes new ones, which replaces the old, after
asking for the password. Without codes or phone, an owner turns two-factor off for that person under
Team, or `reset-password <email>` does it from the server.

**Activity.** Admins see every change made through the dashboard or the API under Team: who did what
to which service, project or person, plus sign-ins, terminal sessions and what justdeploy itself did
(finishing a deletion or a restore).

---

## Command line: jd

`jd` deploys, follows logs, edits variables and runs commands from a terminal or CI, with an API
token (Account → API tokens).

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install-cli.sh | sh
jd login https://deploy.example.com jd_…
```

Each release has builds for Linux, macOS and Windows; elsewhere,
`cargo install --git https://github.com/vedantnimbarte/justdeploy justdeploy-cli`.

| Command | |
|---|---|
| `jd projects` | Projects, their environments and services, with status |
| `jd status shop/web` | A service's status and latest deploy |
| `jd deploy shop/web` | Deploy the latest commit or image and follow the build log; exits non-zero if it fails. `--no-wait` returns once it's queued |
| `jd restart shop/web` | Run the current image again |
| `jd logs shop/web` | Follow the running service's output |
| `jd env shop/web` | Print variables as `KEY=value` |
| `jd env set shop/web KEY=value …` · `jd env unset shop/web KEY …` | Change variables; the service redeploys |
| `jd run shop/web -- npm run db:seed` | Run a command once from the live image; prints its output and exits with its code |
| `jd whoami` | Who the token belongs to |

An environment is addressed by its project name: `shop-staging/web`. In CI, set `JD_URL` and
`JD_TOKEN` instead of running `jd login`:

```yaml
- run: curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install-cli.sh | JD_INSTALL_DIR=$HOME/.local/bin sh
- run: jd deploy shop/web
  env:
    JD_URL: https://deploy.example.com
    JD_TOKEN: ${{ secrets.JD_TOKEN }}
```

---

## Servers and scaling

**Servers.** The **Servers** page lists every server in the cluster with its CPU, memory and disk,
and how many copies run on it. **Add a server** walks through it:

1. Install Docker 24 or newer on the new server.
2. Open the cluster ports between the servers: `2377/tcp` to the main server, `7946/tcp`, `7946/udp`
   and `4789/udp` between all of them. Keep them closed to the internet.
3. Run the `docker swarm join …` command the page shows on the new server.
4. The page notices when it joins.

Admins can label a server (`region=eu`, `disk=ssd`), **drain** it (its copies move to other servers,
for maintenance), **make it a manager** (or a worker again), and remove one that is down. A server still up has to leave first
(`docker swarm leave` on it). justdeploy's own server can't be drained or removed.

**Copies.** Under a service's **Settings → Copies and servers**, run it as several copies; the
cluster spreads them across servers and Traefik shares traffic between them. A deploy starts the new
copies beside the old ones and switches over once they're healthy, as before. Anything with a volume
(every database), a host port or a schedule runs exactly one copy.

**Autoscaling.** Instead of a fixed count, give the fewest and most copies and a CPU target. Every
minute justdeploy adds a copy when the average CPU per copy has stayed above the target for three
minutes, and removes one when it hasn't reached half the target for ten. The target is a percentage
of each copy's CPU limit, or of one core without a limit. Each change shows in the activity log.

**Placement.** A service runs on any server unless you pin it to one or to servers with a label.
A service with a volume is pinned to the server it first ran on, because its data lives there:
justdeploy can't move volumes between servers.

**Failover.** When a server goes down, Swarm restarts its copies on the others within seconds,
except copies pinned to it (databases), which come back with their server.

**Images across servers.** With HTTPS on and more than one server, built images are pushed to a
private registry at `registry.<base domain>`, behind Traefik with a password derived from
`JD_MASTER_KEY`, and every server pulls from it. Without HTTPS, apps built from Git keep running on
the main server; images from Docker Hub and other public registries run anywhere.

**Reaching other servers.** Each server other than the main one runs a small agent (`jd-node-…`) that
forwards its Docker API onto `jd-control`, an encrypted overlay network only the control plane and
the agents join. That is how logs, terminals, metrics and backups work wherever a copy runs.

**Old images.** Once a day, images in the registry that no deployment needs any more (not running,
not among a service's five newest kept for rollback) are deleted, and the registry's garbage
collector frees their space. It waits for a moment with no build or deploy running.

---

## When the main server goes down

Copies on other servers keep running when the main server fails. The **Servers** page, under *If
this server goes down*, sets up the two things that keep them reachable and let justdeploy carry on
elsewhere.

**1. Traffic through every manager.** Make two more servers managers (three or five managers in all:
the cluster can only reschedule copies while more than half of them are up). Each manager other
than the main one then runs its own Traefik, `jd-edge`, on ports 80 and 443 of that server, routing
the same services. Give your domains an A record per manager, or put a load balancer in front, so
visitors reach one that's up.

With HTTPS, certificates are still issued by the main server's Traefik only. Every minute justdeploy
copies them to the edges (a Swarm secret), and the edges pass Let's Encrypt's
`/.well-known/acme-challenge/` requests to the main Traefik, whichever server they arrive at. Edges
keep serving the certificates they have while the main server is down; renewals wait for an issuer.

**2. A standby of justdeploy's database.** Pick another manager and *Start the standby*: a copy of
justdeploy's own Postgres (`jd-pg-<server>`) streams every change from it, with its lag shown on the
page. It also gets justdeploy's settings, master key included, as a Swarm secret.

**Taking over.** If the main server is lost for good, run the command the page shows on the
standby's server:

```bash
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock ghcr.io/vedantnimbarte/justdeploy:<version> justdeploy promote
```

It promotes the standby to the primary database, marks that server as where justdeploy runs, and
starts justdeploy there as a Swarm service (`jd-main`) on port 8080, with the settings it had. The new
main server gets a Traefik of its own (`jd-ingress`) that issues certificates from then on; the
dashboard's domain, if you had one, routes to it once its DNS points there; so does the host in
`JD_PUBLIC_URL`, which webhooks use. Then pick a new standby.

If the old server comes back, its justdeploy refuses to start (the server label `jd.control-plane`
says it moved), so two never manage the cluster at once. Run `docker compose down` there, then
drain and remove it under Servers, or keep it as a manager that only routes.

Moving justdeploy is a command you run, not automatic: a network split must not leave two copies
both believing they're in charge.

---

## Notifications

Under **Settings → Notifications**, add a Slack or Discord incoming webhook, any URL, or **email**
addresses. A channel gets a message when a deploy fails (previews included), a scheduled run or
command fails, a backup fails, a service crashes (at most one crash message per service an hour) or
goes down, or a custom domain stops pointing at your servers (checked every ten minutes; it says
so again when the domain comes back). Tick *Also tell me when
deploys succeed* for successful deploys too. A test message is sent when you add a channel, and the
channel is only saved if it arrives. Webhook URLs are stored encrypted. Any URL receives a JSON
`POST` of `{event, text, url}`, where `event` is `deploy.failed`, `deploy.succeeded`,
`service.crashed`, `service.down`, `service.recovered`, `run.failed`, `backup.failed`,
`domain.unreachable` or `domain.reachable`. A public service going down (two failed uptime checks in
a row) and coming back are always sent.

**Email** goes through an SMTP server set under **Settings → Email**: host, port, encryption
(STARTTLS on 587, TLS on 465, or none for a trusted relay), username and password (stored
encrypted), and the sender. It's saved once a test message is accepted. The subject is the first
line of the message, the body has a link to the page in question.

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
| `JD_POSTGRES_IMAGE` | `ghcr.io/vedantnimbarte/justdeploy-postgres:17` | PostgreSQL with wal-g, used for databases with point-in-time recovery on |
| `JD_LOG_DAYS` | `7` | Days service logs are kept for search (1 to 365) |
| `JD_ENV_FILE` | unset | A file of further settings, read at startup; set by `justdeploy promote` |
| `JD_TEMPLATES_REPO` | this repository | Repository code templates build from |
| `JD_TEMPLATES_BRANCH` | `main` | Its branch |
| `JD_BACKUP_DIR` | `/backups` | Root of "folder on this server" backup destinations |
| `JD_DEPLOY_WORKERS` | `2` | Deploys (builds included) that run at once, each for a different service |
| `JD_ADVERTISE_ADDR` | unset | The address other servers reach this one on, when Docker can't choose (a server with several). The installer passes the first address when it starts the Swarm |
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
| `POST` | `/auth/two-factor/confirm` | `{code}`; returns the ten recovery codes, once |
| `POST` | `/auth/two-factor/recovery-codes` | `{password}`: new recovery codes, replacing the old |
| `GET` `POST` `DELETE` | `/auth/tokens`, `/auth/tokens/{id}` | Your API tokens · create `{name}` returns the value once |
| `GET` `POST` | `/auth/invites/{token}` | What an invite is for · accept it `{password}` |
| `GET` | `/users` | Everyone, with role and two-factor state |
| `GET` | `/servers` | Every server: role, state, resources, labels, copies running, latest usage |
| `GET` | `/servers/join` | Admin: `{command, manager_address}` to add a server |
| `PATCH` `DELETE` | `/servers/{id}` | Admin: `{availability?: active \| drain \| pause, role?: manager \| worker, labels?}` · remove a server that's down |
| `GET` | `/cluster/resilience` | Edge routers per manager, certificates copied, the standby and its lag, the takeover command |
| `PUT` | `/cluster/standby` | Admin: `{node}` (a manager's id) or `{node: null}` |
| `PATCH` `DELETE` | `/users/{id}` | Owner: `{role?, reset_two_factor?}` · remove |
| `GET` `POST` `DELETE` | `/invites`, `/invites/{id}` | Owner: pending invites · create `{email, role}` returns the link once · revoke |
| `GET` | `/activity?before=id` | Admin: the 100 newest entries before `before` |
| `GET` `POST` | `/projects` | List (with services and status) · create `{name}` |
| `GET` `DELETE` | `/projects/{id}` | |
| `POST` | `/projects/{id}/services` | Create a service and deploy it |
| `GET` `PUT` | `/projects/{id}/env` | Shared variables, `[{key, value}]`, replaced as a whole |
| `POST` | `/projects/{id}/environments` | `{environment, branch?}`: a new environment copied from this one; returns it |
| `POST` | `/projects/{id}/sync` | `{from}`: copy settings from another environment; returns `{updated, added}` |
| `GET` | `/projects/{id}/graph` | `[{from, to, keys}]`: which services reference which |
| `GET` | `/projects/{id}/usage?range=day\|week\|quarter` | `[{t, service_id, cpu, memory, disk}]` |
| `GET` | `/projects/{id}/uptime` | Each public service: state, latency, uptime windows, 90 daily values, incidents |
| `PUT` | `/projects/{id}/status-page` | `{enabled, domain, services: [id]}` |
| `GET` | `/status/{project}` | No sign-in: a published status page |
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
| `GET` `PUT` | `/services/{id}/pitr` | Point-in-time recovery state · admin: `{destination_id}` or `null` (redeploys) |
| `POST` | `/services/{id}/pitr/restore` | `{target}` (a time): creates and returns the restored copy |
| `POST` | `/services/{id}/copy` | A new database with this one's data; returns it |
| `GET` `POST` | `/services/{id}/copies` | Copies into this database · admin: replace its data `{from}` |
| `GET` `POST` | `/services/{id}/replicas` | Read replicas with their lag · add one (PostgreSQL); returns it |
| `GET` | `/services/{id}/logs/search?q=&hours=24&before=id` | Kept log lines containing `q`, newest 200 |
| `GET` `POST` `DELETE` | `/backup-destinations`, `/backup-destinations/{id}` | |
| `GET` | `/deployments/{id}` · `/deployments/{id}/logs` | Build log as server-sent events; resumes from `Last-Event-ID` |
| `POST` | `/deployments/{id}/rollback` | |
| `GET` `POST` | `/templates` · `/templates/{id}/deploy` | `{project_name}` or `{project_id}` |
| `GET` `PUT` | `/settings` | `{base_domain}` |
| `GET` `POST` `DELETE` | `/notification-channels`, `/notification-channels/{id}` | `{name, kind: slack \| discord \| webhook \| email, url, notify_success}` (`url` holds the addresses for email); sends a test first |
| `GET` `PUT` `DELETE` | `/settings/smtp` | Admin: the email server · `{host, port, security: starttls \| tls \| none, username, password?, from, test_to}`, saved once the test is accepted |
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
- **Accounts come from invites.** There is no signup. Login attempts (one-time and recovery codes
  included) are limited to 10 per 15 minutes per email address. Sessions, API tokens and invite links are random tokens stored
  hashed; sessions last 30 days and end on sign-out, `reset-password` or removing the person.
  Two-factor secrets and the GitHub App's key are encrypted with `JD_MASTER_KEY`.
- **Roles limit the dashboard and API, not the server.** A member can open a terminal and run commands
  in containers, and builds see the Docker socket, so give the member role only to people you'd trust
  with the server.
- **Secrets are encrypted at rest** (AES-256-GCM): variables, git tokens, database credentials and
  backup credentials. The key is `JD_MASTER_KEY` in `.env`, which the installer creates with
  owner-only permissions. With a standby, justdeploy's settings, the key included, are also a Swarm
  secret (`jd-env-…`), encrypted in the cluster's store and readable only by services that mount it;
  anyone who controls a manager's Docker can mount it, as they could read `.env`.
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

**Upgrading from before servers and scaling.** That version changed the compose files (Traefik
watches the Swarm too), so fetch them again by re-running the installer, which keeps your `.env`:

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh | sh
```

It starts a Swarm on the server. Within a minute of starting, the control plane moves each service
from its old container onto the cluster: each restarts once, and databases briefly stop while they
move. Data stays in the same volumes.

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
stay dark in both. The tokens are in [`web/src/styles.css`](web/src/styles.css).
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

## Known limits

- **One control plane at a time.** Moving it to the standby is a command you run, not automatic. Until
  then, apps keep serving through the other managers, but nothing deploys or scales. The registry
  lives on the main server: after a takeover, images only it had are gone (each server keeps what it
  pulled), so redeploy apps built from Git. Swarm itself needs more than half its managers up to
  reschedule anything.
- **Edges and certificates.** Edge routers serve certificates copied from the main one and log
  "nonexistent certificate resolver" for each HTTPS route; harmless. A few hundred certificates fill
  the 500 KB a Swarm secret can hold.
- **Volumes stay on their server.** Services with a volume run one copy, pinned where their data is.
- **Roles are install-wide**: there is no per-project access yet.
- **Run output** is saved when a run finishes, not streamed while it runs.
- **Logs, terminals and metrics on other servers** go through the control plane's own container, so
  in development (the API run with `cargo run`) only the main server's are visible.
- **Previews** are GitHub only; GitLab merge requests do not get previews. Their database copies come
  from backups, so they are as fresh as the latest backup.
- **Not tested end to end:** real Let's Encrypt issuance (it needs public DNS and port 80; the
  development setup runs plain HTTP on `*.localhost`), webhooks delivered by GitHub or GitLab
  themselves (tested with correctly signed requests), and AWS S3 itself (tested with SeaweedFS's S3
  API, which uses the same client and request signing).
- **Release images for arm64** are built under emulation in CI, which is slow; the amd64 path is the
  one exercised in development.
- **Restores** handle backups up to 8 GiB compressed.
- **Redis** keeps up to a minute of writes at risk (RDB snapshots, no AOF).
- **Domain checks** compare DNS with the managers' addresses; a domain behind a proxy or load
  balancer (Cloudflare's orange cloud, say) reads as pointing elsewhere.
- **Point-in-time recovery** is PostgreSQL only and needs an S3-compatible destination. A restore
  creates a new database; *Replace its data* puts it back over the original as a dump, not in place.
- **Read replicas** are PostgreSQL only, can't be promoted to a primary from the dashboard, and stay on
  the server they first ran on, like every database.
- **Not tested end to end (cluster):** edges serving real Let's Encrypt certificates and passing
  real challenges (the copying and the challenge route were checked without public DNS).
- **Disk usage** reads volume sizes from Docker 29 or newer; older Docker shows no disk.
- **Kept logs** cover production services, not previews or one-off runs, and are stored in
  justdeploy's own Postgres, so a chatty service grows it: lower `JD_LOG_DAYS` if it does.
- **Uptime checks** run from the main server against generated hostnames, so they test routing on
  that server rather than reachability from the internet. They are off while the base domain is
  `localhost`.

---

## Releasing

Pushing a version tag builds the three images for amd64 and arm64 and publishes them to GitHub
Container Registry as `ghcr.io/vedantnimbarte/justdeploy`, `ghcr.io/vedantnimbarte/justdeploy-builder`
and `ghcr.io/vedantnimbarte/justdeploy-postgres`, tagged with the version, `major.minor` and `latest`
(and `17`, the PostgreSQL major, for the last):

```bash
git tag v0.1.0 && git push origin v0.1.0
```

After the first release, make the three packages public in GitHub (*Packages → Package settings → Change
visibility*) so servers can pull them without logging in.

---

## License

[MIT](LICENSE)
