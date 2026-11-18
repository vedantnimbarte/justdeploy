<p align="center">
  <img src="docs/logo.svg" alt="justdeploy" height="56">
</p>

<p align="center">
  Self-hosted deploys for apps, backends and databases on your own server, with plain Docker.<br>
  Push to Git or pick a template and get a built, routed, HTTPS service, without Kubernetes.
</p>

---

## Features

- **Apps from Git.** Uses your Dockerfile, or detects Node, Vite, Next.js, Python, Go, Rust and static
  sites with [Railpack](https://railpack.com).
- **One-click databases.** PostgreSQL, MySQL, MariaDB, Redis and MongoDB with volumes, generated
  credentials and connection URLs.
- **Any Docker image**, such as Uptime Kuma, n8n or Umami.
- **Zero-downtime deploys** behind a health check, push to deploy, pull request previews and
  one-click rollback.
- **Backups** to S3-compatible storage or a local folder, and point-in-time recovery for PostgreSQL.
- **Logs, metrics, a web terminal**, uptime checks, status pages and Slack, Discord or email alerts.
- **Teams** with roles, two-factor sign-in and API tokens, plus the `jd` CLI.
- **Multiple servers** through Docker Swarm, with autoscaling and a standby for failover.
- **18 templates**, from a static site to FastAPI + PostgreSQL.

justdeploy is MIT licensed and meant for you and your team, not for hosting untrusted code.

## Requirements

- A Linux server (x86-64 or arm64) you can reach over SSH, with root or `sudo`
- Docker Engine 24+ with the Compose plugin
- Ports **80** and **8080** open (and **443** for HTTPS)
- Optional: a domain whose DNS you control

## Quick start

**1. Install Docker** (skip if `docker compose version` already works):

```bash
curl -fsSL https://get.docker.com | sudo sh
```

**2. Open the ports.** With `ufw`, for example (keep SSH open):

```bash
sudo ufw allow OpenSSH && sudo ufw allow 80,443,8080/tcp && sudo ufw enable
```

**3. Install justdeploy:**

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh | sudo sh
```

The installer writes everything to `/opt/justdeploy`, generates a `.env` with a master key and
database password, starts a single-server Docker Swarm, and runs the containers.

**4. Open `http://<server-ip>:8080`** and create the owner account.

**5. Deploy something.** Open **Templates**, pick *FastAPI + PostgreSQL*, and deploy it into a new
project. In a minute or two the API is live at `api-<project>.<base domain>`. To deploy your own app,
use **Add service → Git repository** and set the port it listens on (justdeploy also sets `PORT`).

Without a domain, apps get hostnames under `<server-ip>.sslip.io`, which works with no DNS setup.

## Configuration

### Use your own domain

Every public service gets `<service>-<project>.<base domain>`. Point a wildcard record at the server:

```
*.apps.example.com.   A   203.0.113.10
```

Then install with it (or change it later under **Settings**):

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh \
  | sudo JD_BASE_DOMAIN=apps.example.com sh
```

### Turn on HTTPS

Give an email for Let's Encrypt and, optionally, a hostname for the dashboard:

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install.sh \
  | sudo JD_BASE_DOMAIN=apps.example.com JD_ACME_EMAIL=you@example.com JD_DASHBOARD_DOMAIN=deploy.example.com sh
```

Port 80 must stay reachable for certificate challenges. On an existing install, add the same
variables to `/opt/justdeploy/.env`, then apply them and redeploy your public services:

```bash
cd /opt/justdeploy && sudo docker compose -f docker-compose.yml -f docker-compose.tls.yml up -d
```

Once the dashboard has its own domain, stop exposing port 8080 with `JD_PORT=127.0.0.1:8080`.

### Settings in `.env`

All settings live in `/opt/justdeploy/.env`. The ones you are most likely to change:

| Variable | Default | Purpose |
|---|---|---|
| `JD_MASTER_KEY` | generated | Encrypts every stored secret. **Back it up; if lost, secrets cannot be read.** |
| `JD_BASE_DOMAIN` | `<ip>.sslip.io` | Domain apps are served under |
| `JD_PUBLIC_URL` | `http://<ip>:8080` | Public address of the dashboard, used in webhook URLs |
| `JD_ACME_EMAIL` | unset | Turns on HTTPS with Let's Encrypt |
| `JD_DASHBOARD_DOMAIN` | unset | Serve the dashboard at this hostname over HTTPS |
| `JD_PORT` | `8080` | Host port (or `ip:port`) for the dashboard |
| `JD_VERSION` | `latest` | justdeploy image tag to run, e.g. `0.1.0` (no `v`) |
| `JD_DEPLOY_WORKERS` | `2` | Deploys that run at once |
| `JD_LOG_DAYS` | `7` | Days service logs are kept for search |

After editing, apply with `sudo docker compose up -d` in `/opt/justdeploy` (add
`-f docker-compose.yml -f docker-compose.tls.yml` when HTTPS is on). Every variable is in the
[configuration reference](docs/reference.md#configuration-reference).

### Push to deploy

Under **Settings → GitHub**, *Create GitHub App* and install it on your repositories: pushes deploy
automatically and pull requests get previews. GitLab and plain GitHub webhooks also work. GitHub must
be able to reach `JD_PUBLIC_URL`. Details in the [user guide](docs/guide.md#push-to-deploy-and-pull-request-previews).

## Maintenance

**Upgrade** (migrations run on start-up; add `-f docker-compose.yml -f docker-compose.tls.yml` to
both commands when HTTPS is on):

```bash
cd /opt/justdeploy && sudo docker compose pull && sudo docker compose up -d
```

**Back up justdeploy itself:** keep a copy of `.env`, and dump its database:

```bash
cd /opt/justdeploy && sudo docker compose exec -T postgres pg_dump -U justdeploy justdeploy | gzip > justdeploy-$(date +%F).sql.gz
```

Your apps' databases are backed up from each database's **Backups** tab.

**Forgot the password:** this prints a new one for the first owner and turns off their two-factor
sign-in (pass an email to reset someone else):

```bash
cd /opt/justdeploy && sudo docker compose exec justdeploy justdeploy reset-password
```

## Command line

```bash
curl -fsSL https://raw.githubusercontent.com/vedantnimbarte/justdeploy/main/deploy/install-cli.sh | sh
jd login https://deploy.example.com <api-token>   # token from Account → API tokens
jd deploy shop/web
```

In CI, set `JD_URL` and `JD_TOKEN` instead of running `jd login`. All commands are in the
[user guide](docs/guide.md#command-line-jd).

## Development

You need Docker, Rust (stable), Node 24 with pnpm, and
`cargo install sqlx-cli --no-default-features --features postgres`.

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d postgres traefik
docker build -t justdeploy-builder:dev builder
cd api && cp .env.example .env    # then set JD_MASTER_KEY in api/.env: openssl rand -base64 32
cargo run                          # API on :8080
cd web && pnpm install && pnpm dev # dashboard on :5173, in a second terminal from the repo root
```

Open <http://localhost:5173>. Checks, code map and release steps are in
[docs/development.md](docs/development.md).

## Documentation

| | |
|---|---|
| [User guide](docs/guide.md) | Projects, builds, deploys, variables, domains, databases, backups, cron jobs, logs, team, CLI, templates |
| [Operations](docs/operations.md) | Multiple servers, autoscaling, failover, security, upgrades, known limits |
| [Reference](docs/reference.md) | Every configuration variable and the HTTP API |
| [Architecture](docs/architecture.md) | How the control plane, Swarm and Traefik fit together |
| [Development](docs/development.md) | Local setup, checks, code map, releasing |

## Security

The dashboard controls Docker on the host, so treat every account as server access: use strong
passwords and two-factor sign-in, and keep the dashboard behind HTTPS. Report security problems
privately to the maintainer through GitHub rather than in a public issue. More in
[Operations](docs/operations.md#security).

## License

[MIT](LICENSE)
