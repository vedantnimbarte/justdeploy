# How justdeploy works

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
one command (see [Servers and scaling](operations.md#servers-and-scaling)). Three containers make up justdeploy
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
