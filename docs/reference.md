# Reference

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
| `JD_LOG_DAYS` | `7` | Days service logs, requests and traces are kept (1 to 365) |
| `JD_OTLP_TRACES_URL` | `http://justdeploy:4318/v1/traces` | Where the compose Traefik sends its traces. Change it only when the API doesn't run in the compose project, as in development |
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
[Team, roles and access](guide.md#team-roles-and-access)); a missing one answers 403 naming the role. Webhooks
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
| `GET` | `/projects/{id}/requests?service=&env=&status=&method=&q=&hours=1&before=id` | Requests, newest 200. `env`: `production` or `preview`; `status`: `2xx` to `5xx`; `q`: text in the path or host. Instead of `hours`, `from` and `to` (RFC 3339; `to` defaults to now) pick a fixed window |
| `GET` | `/projects/{id}/requests/volume?service=&env=&method=&q=&hours=1` | Requests per step over the same window (at most 60 steps), counted by status class. Also takes `from` and `to` |
| `GET` | `/projects/{id}/logs?service=&env=&level=&q=&hours=24&before=id` | Kept log lines containing `q`, newest 200. `level`: `debug`, `info`, `warn` or `error`. Also takes `from` and `to` |
| `GET` | `/projects/{id}/observability?service=&hours=24` | Requests, 5xx answers, p50/p95 latency and error lines over time, the routes that fail most, and the commonest error messages |
| `GET` | `/traces/{id}` | A trace's spans and the log lines that name it |
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
