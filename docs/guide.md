# User guide

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
- [Requests, logs and traces](#requests-logs-and-traces)
- [Usage, uptime and status pages](#usage-uptime-and-status-pages)
- [Team, roles and access](#team-roles-and-access)
- [Command line: jd](#command-line-jd)
- [Notifications](#notifications)
- [System overview](#system-overview)
- [Templates](#templates)

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

`PORT` is set to the service's port unless you set it yourself. Public services also get the
`OTEL_*` variables described in [Tracing your app](#tracing-your-app).

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

- **Logs** follows the live container's output (the last 500 lines, then live). To search what was
  printed earlier, use [Observability](#requests-logs-and-traces).
- **Metrics** charts CPU, memory (excluding reclaimable page cache) and network rate per service.
  Samples are taken every 30 seconds and kept for 24 hours.
- **Terminal** opens a shell (`bash` if present, otherwise `sh`) inside the running container. Each
  session is written to the audit log.

---

## Requests, logs and traces

Every project, and every service on its own page, has an **Observability** tab with three views. Each
can be narrowed to a time range, a service, and production or previews.

- **Requests** lists every request a public service answered: time, status, method, path, host,
  service and duration, newest first. Filter by status class (2xx to 5xx), method, or text in the
  path or host. Over the last hour the list follows along every 5 seconds. Query string values
  are redacted (`?token=REDACTED`).
- **Logs** searches everything production and preview services printed: any text, case-insensitive,
  and a level. A JSON line gets its level from `level`, `severity`, `lvl` or `levelname` (pino's
  numbers too), and its fields show when you click it. A plain line is info unless it starts with a
  level such as `ERROR`, `[warn]` or `error:`, or carries `level=…`.
- **Overview** charts requests with their share of 5xx answers, p50 and p95 latency, and lines logged
  at error level. Below the charts are the routes that fail most and the most frequent error messages.

Click a request to see its details, its **trace** as a timeline, and the lines logged while serving it.
Requests, traces and log lines are kept in justdeploy's Postgres for `JD_LOG_DAYS` (7) days.

### Tracing your app

Traefik traces every request, so each one shows how long it took at the edge with no change to your
app. For the work inside a request, such as database queries and outgoing calls, add an
[OpenTelemetry](https://opentelemetry.io) SDK to the app. justdeploy sets what the SDK needs on every
public service: `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`,
`OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES` (if you set any of these yourself, yours wins; to
`OTEL_RESOURCE_ATTRIBUTES` justdeploy adds its deployment id). For example:

- **Node.js:** install `@opentelemetry/api` and `@opentelemetry/auto-instrumentations-node`, and set
  the variable `NODE_OPTIONS=--require @opentelemetry/auto-instrumentations-node/register`.
- **Python:** install `opentelemetry-distro` and `opentelemetry-exporter-otlp`, run
  `opentelemetry-bootstrap -a install` in the build, and start the app with
  `opentelemetry-instrument`, as in `opentelemetry-instrument uvicorn main:app`.

The app's spans appear under Traefik's span for the request. To group log lines under their request,
log JSON with the trace id in a `trace_id` (or `traceId`) field. Most OpenTelemetry logging
integrations do this for you.

Private services and workers aren't on the network the receiver listens on, so their traces don't
reach justdeploy.

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

Code templates build from the [`examples/`](../examples) folder of this repository, so each one doubles as
a worked example. Templates can be deployed into a new project or added to an existing one.

### Adding a template

Templates live in [`templates/catalog.json`](../templates/catalog.json) and are compiled into the binary.
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
