# Running justdeploy in production

- [Servers and scaling](#servers-and-scaling)
- [When the main server goes down](#when-the-main-server-goes-down)
- [Security](#security)
- [Upgrading and backing up justdeploy itself](#upgrading-and-backing-up-justdeploy-itself)
- [Known limits](#known-limits)

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
docker compose exec -T postgres pg_dump -U justdeploy justdeploy | gzip > justdeploy-$(date +%F).sql.gz
```

Your apps' data lives in their database services and volumes; back those up with the Backups tab.

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
