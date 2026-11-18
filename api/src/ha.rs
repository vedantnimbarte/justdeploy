//! Surviving the loss of the main server.
//!
//! - **Routing.** Every other manager runs a Traefik of its own (`jd-edge`, a
//!   global Swarm service publishing 80 and 443 on the server itself) that routes
//!   the same Swarm services, so traffic sent to any manager reaches the apps.
//!   Certificates are issued by the main server's Traefik only: justdeploy copies
//!   them to the edges as a Swarm secret, and edges pass Let's Encrypt's
//!   challenges to the main one, whichever server a challenge arrives at.
//! - **Standby.** justdeploy's own Postgres streams to a copy on a manager you
//!   pick (`jd-pg-<server>`). If the main server is lost, `justdeploy promote`,
//!   run on that server, makes the copy the primary and starts justdeploy there
//!   from the settings it was given (a Swarm secret).
//! - **One control plane.** The server justdeploy runs on carries the node label
//!   `jd.control-plane`. A copy that finds it on another server won't start, so
//!   an old main server that comes back can't act on the cluster too.

use crate::{
    cluster::{self, CONTROL, MESH},
    docker::{self, LABEL_KIND, MANAGED},
    error::{bad_request, Error, Result},
    state::SharedState,
};
use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine};
use bollard::{
    models::{
        EndpointPortConfig, EndpointPortConfigPublishModeEnum, EndpointSpec, Mount, MountType,
        NetworkAttachmentConfig, Node, SecretSpec, ServiceSpec, ServiceSpecMode,
        ServiceSpecModeReplicated, TaskSpec, TaskSpecContainerSpec, TaskSpecContainerSpecFile,
        TaskSpecContainerSpecSecrets, TaskSpecPlacement, TaskSpecRestartPolicy,
        TaskSpecRestartPolicyConditionEnum,
    },
    query_parameters::{ListNodesOptions, ListSecretsOptionsBuilder, UpdateNodeOptionsBuilder},
    Docker,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

const TRAEFIK_IMAGE: &str = "traefik:v3.7";
const POSTGRES_IMAGE: &str = "postgres:17-alpine";
const EDGE: &str = "jd-edge";
const INGRESS: &str = "jd-ingress";
const MAIN: &str = "jd-main";
const SLOT: &str = "jd_standby";
const LABEL_CONTROL_PLANE: &str = "jd.control-plane";
const LABEL_CONFIG: &str = "jd.config";
const LABEL_NODE: &str = "jd.node";
const LABEL_IMAGE: &str = "jd.image";
const LABEL_DASHBOARD: &str = "jd.dashboard-domain";
const LABEL_TLS: &str = "jd.tls";
/// Where Traefik keeps its Let's Encrypt account and certificates.
const ACME_FILE: &str = "/letsencrypt/acme.json";

/// Set on the control plane that `justdeploy promote` started.
fn promoted() -> bool {
    std::env::var("JD_PROMOTED").is_ok_and(|v| v == "1")
}

fn short(node: &str) -> &str {
    &node[..node.len().min(12)]
}

fn standby_name(node: &str) -> String {
    format!("jd-pg-{}", short(node))
}

fn label<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.spec
        .as_ref()
        .and_then(|s| s.labels.as_ref())
        .and_then(|l| l.get(key))
        .map(String::as_str)
}

fn hostname(node: &Node) -> String {
    node.description
        .as_ref()
        .and_then(|d| d.hostname.clone())
        .unwrap_or_default()
}

fn is_manager(node: &Node) -> bool {
    node.manager_status.is_some()
}

async fn set_node_label(docker: &Docker, node: &Node, value: Option<&str>) -> anyhow::Result<()> {
    let id = node.id.clone().unwrap_or_default();
    let version = node.version.as_ref().and_then(|v| v.index).unwrap_or(0);
    let mut spec = node.spec.clone().unwrap_or_default();
    let mut labels = spec.labels.unwrap_or_default();
    match value {
        Some(v) => labels.insert(LABEL_CONTROL_PLANE.into(), v.into()),
        None => labels.remove(LABEL_CONTROL_PLANE),
    };
    spec.labels = Some(labels);
    let options = UpdateNodeOptionsBuilder::new()
        .version(version as i64)
        .build();
    docker.update_node(&id, spec, options).await?;
    Ok(())
}

/// At startup, before anything else touches the cluster: refuses to run when
/// justdeploy was moved to another server, and marks this one otherwise.
pub async fn claim(docker: &Docker, me: &str) -> anyhow::Result<()> {
    let nodes = docker.list_nodes(None::<ListNodesOptions>).await?;
    if let Some(other) = nodes
        .iter()
        .find(|n| label(n, LABEL_CONTROL_PLANE).is_some() && n.id.as_deref() != Some(me))
    {
        bail!(
            "justdeploy was moved to {} with `justdeploy promote`, so this copy stays stopped. \
             Stop it here for good with `docker compose down`",
            hostname(other)
        );
    }
    if let Some(node) = nodes.iter().find(|n| n.id.as_deref() == Some(me)) {
        if label(node, LABEL_CONTROL_PLANE).is_none() {
            set_node_label(docker, node, Some("true")).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- shared helpers

fn restart_always() -> Option<TaskSpecRestartPolicy> {
    Some(TaskSpecRestartPolicy {
        condition: Some(TaskSpecRestartPolicyConditionEnum::ANY),
        delay: Some(5_000_000_000),
        ..Default::default()
    })
}

fn host_ports(ports: &[u16]) -> Option<EndpointSpec> {
    Some(EndpointSpec {
        ports: Some(
            ports
                .iter()
                .map(|&p| EndpointPortConfig {
                    target_port: Some(p as i64),
                    published_port: Some(p as i64),
                    publish_mode: Some(EndpointPortConfigPublishModeEnum::HOST),
                    ..Default::default()
                })
                .collect(),
        ),
        ..Default::default()
    })
}

fn networks(names: &[&str]) -> Option<Vec<NetworkAttachmentConfig>> {
    Some(
        names
            .iter()
            .map(|n| NetworkAttachmentConfig {
                target: Some(n.to_string()),
                ..Default::default()
            })
            .collect(),
    )
}

fn docker_socket(read_only: bool) -> Mount {
    Mount {
        target: Some("/var/run/docker.sock".into()),
        source: Some("/var/run/docker.sock".into()),
        typ: Some(MountType::BIND),
        read_only: Some(read_only),
        ..Default::default()
    }
}

fn volume(name: &str, target: &str) -> Mount {
    Mount {
        target: Some(target.into()),
        source: Some(name.into()),
        typ: Some(MountType::VOLUME),
        ..Default::default()
    }
}

fn hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())[..12].to_string()
}

/// A secret named `prefix-<hash of data>`, made if missing. Secrets can't
/// change, so new content is a new secret.
async fn ensure_secret(
    docker: &Docker,
    prefix: &str,
    kind: &str,
    data: &str,
) -> Result<(String, String)> {
    let name = format!("{prefix}-{}", hash(&[data]));
    let filters = HashMap::from([("name", vec![name.clone()])]);
    let existing = docker
        .list_secrets(Some(
            ListSecretsOptionsBuilder::new().filters(&filters).build(),
        ))
        .await?
        .into_iter()
        .find(|s| s.spec.as_ref().and_then(|s| s.name.as_deref()) == Some(name.as_str()))
        .and_then(|s| s.id);
    let id = match existing {
        Some(id) => id,
        None => {
            docker
                .create_secret(SecretSpec {
                    name: Some(name.clone()),
                    labels: Some(HashMap::from([
                        (MANAGED.to_string(), "true".to_string()),
                        (LABEL_KIND.to_string(), kind.to_string()),
                    ])),
                    data: Some(STANDARD.encode(data)),
                    ..Default::default()
                })
                .await?
                .id
        }
    };
    Ok((id, name))
}

/// Removes this kind's secrets other than `keep`. One still in use stays
/// (Docker refuses) and goes on a later pass.
async fn remove_old_secrets(docker: &Docker, kind: &str, keep: Option<&str>) {
    let filters = HashMap::from([("label", vec![format!("{LABEL_KIND}={kind}")])]);
    let Ok(secrets) = docker
        .list_secrets(Some(
            ListSecretsOptionsBuilder::new().filters(&filters).build(),
        ))
        .await
    else {
        return;
    };
    for secret in secrets {
        let name = secret.spec.and_then(|s| s.name).unwrap_or_default();
        if Some(name.as_str()) != keep {
            let _ = docker.delete_secret(&secret.id.unwrap_or_default()).await;
        }
    }
}

fn secret_file(id: String, name: String, file: &str) -> TaskSpecContainerSpecSecrets {
    TaskSpecContainerSpecSecrets {
        file: Some(TaskSpecContainerSpecFile {
            name: Some(file.into()),
            uid: Some("0".into()),
            gid: Some("0".into()),
            mode: Some(0o400),
        }),
        secret_id: Some(id),
        secret_name: Some(name),
    }
}

// ---------------------------------------------------------------- routing

/// The Traefik that issues certificates and where it is on `jd-mesh`: the one
/// started with justdeploy (compose), or `jd-ingress` after a promotion.
async fn issuer(state: &SharedState) -> Option<(Docker, String, String)> {
    if promoted() {
        let container = docker::local_task(&state.docker, INGRESS).await.ok()??;
        return Some((state.docker.clone(), container, INGRESS.to_string()));
    }
    let (container, name) = cluster::compose_traefik()?;
    Some((
        state.docker.clone(),
        container.to_string(),
        name.to_string(),
    ))
}

/// What every edge Traefik loads from its file: the issuer's certificates, and
/// a route for Let's Encrypt's challenges to the issuer.
pub fn edge_config(acme: Option<&str>, issuer: Option<&str>) -> String {
    let mut certificates = Vec::new();
    if let Some(acme) = acme.and_then(|a| serde_json::from_str::<Value>(a).ok()) {
        for resolver in acme.as_object().into_iter().flat_map(|o| o.values()) {
            for cert in resolver["Certificates"].as_array().into_iter().flatten() {
                let pem = |key: &str| {
                    cert[key]
                        .as_str()
                        .and_then(|b| STANDARD.decode(b).ok())
                        .and_then(|b| String::from_utf8(b).ok())
                };
                if let (Some(certificate), Some(key)) = (pem("certificate"), pem("key")) {
                    certificates.push(json!({ "certFile": certificate, "keyFile": key }));
                }
            }
        }
    }
    let mut config = json!({ "tls": { "certificates": certificates } });
    if let Some(issuer) = issuer {
        config["http"] = json!({
            "routers": { "jd-acme": {
                "rule": "PathPrefix(`/.well-known/acme-challenge/`)",
                "entryPoints": ["web"],
                "service": "jd-acme",
                "priority": 2147483647,
            }},
            "services": { "jd-acme": {
                "loadBalancer": { "servers": [{ "url": format!("http://{issuer}:80") }] },
            }},
        });
    }
    // JSON is YAML, which Traefik's file provider reads.
    config.to_string()
}

fn traefik_args(tls: bool) -> Vec<String> {
    let mut args = vec![
        "--providers.swarm.endpoint=unix:///var/run/docker.sock".to_string(),
        "--providers.swarm.exposedbydefault=false".to_string(),
        format!("--providers.swarm.network={MESH}"),
        "--entrypoints.web.address=:80".to_string(),
    ];
    if tls {
        args.extend([
            "--entrypoints.websecure.address=:443".to_string(),
            "--entrypoints.web.http.redirections.entrypoint.to=websecure".to_string(),
            "--entrypoints.web.http.redirections.entrypoint.scheme=https".to_string(),
            // Below the challenge route, which has to reach the issuer over HTTP.
            "--entrypoints.web.http.redirections.entrypoint.priority=1".to_string(),
        ]);
    }
    args
}

fn ports(tls: bool) -> Vec<u16> {
    if tls {
        vec![80, 443]
    } else {
        vec![80]
    }
}

// ponytail: routers keep `tls.certresolver=le`, which edges don't have, so each
// logs "nonexistent certificate resolver" and serves the copied certificate
// anyway. Move the resolver to the main entrypoint if the noise matters.
fn edge_spec(tls: bool, config: &str, secret: (String, String)) -> ServiceSpec {
    let mut args = traefik_args(tls);
    args.push("--providers.file.filename=/run/secrets/jd-edge.yml".into());
    ServiceSpec {
        name: Some(EDGE.into()),
        labels: Some(HashMap::from([
            (MANAGED.to_string(), "true".to_string()),
            (LABEL_KIND.to_string(), "edge".to_string()),
            (LABEL_CONFIG.to_string(), config.to_string()),
        ])),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(TRAEFIK_IMAGE.into()),
                args: Some(args),
                mounts: Some(vec![docker_socket(true)]),
                secrets: Some(vec![secret_file(secret.0, secret.1, "jd-edge.yml")]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![
                    "node.role==manager".to_string(),
                    format!("node.id!={}", cluster::self_node()),
                ]),
                ..Default::default()
            }),
            networks: networks(&[MESH]),
            restart_policy: restart_always(),
            ..Default::default()
        }),
        mode: Some(ServiceSpecMode {
            global: Some(HashMap::new()),
            ..Default::default()
        }),
        endpoint_spec: host_ports(&ports(tls)),
        ..Default::default()
    }
}

/// After a promotion there's no compose Traefik: this one takes its place on
/// the control plane's server, issuing certificates when HTTPS is on.
fn ingress_spec(state: &SharedState) -> ServiceSpec {
    let tls = state.config.tls();
    let mut args = traefik_args(tls);
    if let Some(email) = &state.config.acme_email {
        args.extend([
            format!("--certificatesresolvers.le.acme.email={email}"),
            format!("--certificatesresolvers.le.acme.storage={ACME_FILE}"),
            "--certificatesresolvers.le.acme.httpchallenge.entrypoint=web".to_string(),
        ]);
    }
    ServiceSpec {
        name: Some(INGRESS.into()),
        labels: Some(HashMap::from([
            (MANAGED.to_string(), "true".to_string()),
            (LABEL_KIND.to_string(), "ingress".to_string()),
            (LABEL_NODE.to_string(), cluster::self_node().to_string()),
        ])),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(TRAEFIK_IMAGE.into()),
                args: Some(args),
                mounts: Some(vec![
                    docker_socket(true),
                    volume("jd-letsencrypt", "/letsencrypt"),
                ]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={}", cluster::self_node())]),
                ..Default::default()
            }),
            networks: networks(&[MESH]),
            restart_policy: restart_always(),
            ..Default::default()
        }),
        mode: Some(ServiceSpecMode {
            replicated: Some(ServiceSpecModeReplicated { replicas: Some(1) }),
            ..Default::default()
        }),
        endpoint_spec: host_ports(&ports(tls)),
        ..Default::default()
    }
}

/// Creates a service, or updates it in place so its copies roll over rather
/// than all stopping at once.
async fn put_service(docker: &Docker, spec: ServiceSpec) -> Result<()> {
    let name = spec.name.clone().unwrap_or_default();
    match docker.inspect_service(&name, None).await {
        Ok(current) => {
            let version = current.version.and_then(|v| v.index).unwrap_or(0);
            let options = bollard::query_parameters::UpdateServiceOptionsBuilder::new()
                .version(version as i32)
                .build();
            docker.update_service(&name, spec, options, None).await?;
        }
        Err(e) if docker::is_not_found(&e) => {
            docker.create_service(spec, None).await?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Every minute: an edge Traefik on each other manager, with the current
/// certificates; none when this is the only manager.
pub async fn refresh_routing(state: &SharedState) -> Result<()> {
    if promoted() {
        let current = docker::workloads(&state.docker, &[(LABEL_KIND, "ingress")]).await?;
        if current.first().and_then(|w| w.label(LABEL_NODE)) != Some(cluster::self_node()) {
            docker::create_service(&state.docker, ingress_spec(state), None).await?;
        }
    }
    let nodes = state.docker.list_nodes(None::<ListNodesOptions>).await?;
    let others = nodes
        .iter()
        .filter(|n| is_manager(n) && n.id.as_deref() != Some(cluster::self_node()))
        .count();
    let current = docker::workloads(&state.docker, &[(LABEL_KIND, "edge")]).await?;
    if others == 0 {
        if !current.is_empty() {
            docker::remove_service(&state.docker, EDGE).await?;
        }
        remove_old_secrets(&state.docker, "edge", None).await;
        return Ok(());
    }
    let tls = state.config.tls();
    let issuer = issuer(state).await;
    let acme = match (&issuer, tls) {
        (Some((docker, container, _)), true) => {
            match docker::exec(docker, container, "root", &format!("cat {ACME_FILE}")).await {
                Ok((0, text)) => Some(text),
                _ => None,
            }
        }
        _ => None,
    };
    let config = edge_config(acme.as_deref(), issuer.as_ref().map(|i| i.2.as_str()));
    let version = hash(&[
        &config,
        if tls { "tls" } else { "plain" },
        cluster::self_node(),
    ]);
    if current.first().and_then(|w| w.label(LABEL_CONFIG)) == Some(version.as_str()) {
        return Ok(());
    }
    let secret = ensure_secret(&state.docker, "jd-edge", "edge", &config).await?;
    let keep = secret.1.clone();
    put_service(&state.docker, edge_spec(tls, &version, secret)).await?;
    remove_old_secrets(&state.docker, "edge", Some(&keep)).await;
    Ok(())
}

// ---------------------------------------------------------------- standby

/// The parts of justdeploy's DATABASE_URL a standby needs.
struct Db {
    user: String,
    password: String,
    host: String,
    port: u16,
    name: String,
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_db(url: &str) -> anyhow::Result<Db> {
    let url = reqwest::Url::parse(url).context("DATABASE_URL isn't a URL")?;
    Ok(Db {
        user: decode(url.username()),
        password: decode(url.password().unwrap_or_default()),
        host: url.host_str().unwrap_or("localhost").to_string(),
        port: url.port().unwrap_or(5432),
        name: url.path().trim_start_matches('/').to_string(),
    })
}

/// The server picked for the standby, if any.
async fn standby_node(state: &SharedState) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'standby_node'")
            .fetch_optional(&state.db)
            .await?,
    )
}

/// Picks (or, with None, drops) the manager that keeps a standby copy of
/// justdeploy's database.
pub async fn set_standby(state: &SharedState, node: Option<String>) -> Result<()> {
    match node {
        None => {
            sqlx::query!("DELETE FROM settings WHERE key = 'standby_node'")
                .execute(&state.db)
                .await?;
        }
        Some(node) => {
            if node == cluster::self_node() {
                return Err(bad_request(
                    "justdeploy already runs on that server; pick another manager",
                ));
            }
            let found = state.docker.inspect_node(&node).await?;
            if !is_manager(&found) {
                return Err(bad_request(
                    "the standby has to be a manager, so it can take over the cluster; make it one first",
                ));
            }
            sqlx::query!(
                "INSERT INTO settings (key, value) VALUES ('standby_node', $1)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                node
            )
            .execute(&state.db)
            .await?;
        }
    }
    refresh_standby(state).await
}

/// Lets the standby replicate from justdeploy's Postgres: the slot and the
/// 10 GB cap through SQL, the pg_hba.conf rule inside its container (compose).
async fn prepare_primary(state: &SharedState) -> Result<()> {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return Ok(());
    }
    if let Some(container) = cluster::compose_postgres() {
        crate::data::allow_replication(&state.docker, container, SLOT)
            .await
            .map_err(Error::Internal)?;
    } else {
        // Promoted, or in development: the rule is already in the copied files,
        // or up to whoever runs Postgres.
        sqlx::query("ALTER SYSTEM SET max_slot_wal_keep_size = '10GB'")
            .execute(&state.db)
            .await?;
        sqlx::query("SELECT pg_reload_conf()")
            .execute(&state.db)
            .await?;
        sqlx::query(
            "SELECT pg_create_physical_replication_slot($1)
             WHERE NOT EXISTS (SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
        )
        .bind(SLOT)
        .execute(&state.db)
        .await?;
    }
    DONE.store(true, Ordering::Relaxed);
    Ok(())
}

/// justdeploy's settings as an env file, for `promote` to start it with.
fn env_file() -> String {
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("JD_") || k == "DATABASE_URL" || k == "RUST_LOG")
        .filter(|(k, _)| {
            !["JD_PROMOTED", "JD_ENV_FILE", "JD_BIND", "JD_WEB_DIR"].contains(&k.as_str())
        })
        .collect();
    vars.sort();
    vars.iter()
        .map(|(k, v)| {
            let v = v.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{k}=\"{v}\"\n")
        })
        .collect()
}

/// Where the standby copies from: the compose Postgres on `jd-control`, or
/// the address justdeploy itself uses.
fn primary_address(db: &Db) -> (String, u16) {
    match cluster::compose_postgres_name() {
        Some(name) => (name.to_string(), 5432),
        None => (db.host.clone(), db.port),
    }
}

/// The image justdeploy runs from, for `promote` to start the same version.
async fn own_image(state: &SharedState) -> String {
    let fallback = "ghcr.io/vedantnimbarte/justdeploy:latest".to_string();
    let Ok(hostname) = std::env::var("HOSTNAME") else {
        return fallback;
    };
    state
        .docker
        .inspect_container(&hostname, None)
        .await
        .ok()
        .and_then(|c| c.config)
        .and_then(|c| c.image)
        .unwrap_or(fallback)
}

fn standby_spec(
    node: &str,
    db: &Db,
    primary: (String, u16),
    labels: HashMap<String, String>,
) -> ServiceSpec {
    let env = vec![
        format!("POSTGRES_USER={}", db.user),
        format!("POSTGRES_PASSWORD={}", db.password),
        format!("POSTGRES_DB={}", db.name),
        format!("PGPASSWORD={}", db.password),
        format!("JD_PRIMARY_HOST={}", primary.0),
        format!("JD_PRIMARY_PORT={}", primary.1),
        format!("JD_SLOT={SLOT}"),
    ];
    let script = format!(
        "{}\nexec docker-entrypoint.sh postgres\n",
        crate::data::BASE_BACKUP
    );
    ServiceSpec {
        name: Some(standby_name(node)),
        labels: Some(labels),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(POSTGRES_IMAGE.into()),
                args: Some(vec!["sh".into(), "-c".into(), script]),
                env: Some(env),
                mounts: Some(vec![volume("jd-pg-data", "/var/lib/postgresql/data")]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={node}")]),
                ..Default::default()
            }),
            networks: networks(&[CONTROL]),
            restart_policy: restart_always(),
            ..Default::default()
        }),
        mode: Some(ServiceSpecMode {
            replicated: Some(ServiceSpecModeReplicated { replicas: Some(1) }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Every minute: the standby on the picked server, and no other. The copy on
/// this server, after a promotion, is the primary and is never touched.
pub async fn refresh_standby(state: &SharedState) -> Result<()> {
    let wanted = standby_node(state)
        .await?
        .filter(|n| n != cluster::self_node());
    let current = docker::workloads(&state.docker, &[(LABEL_KIND, "standby")]).await?;
    for w in &current {
        let node = w.label(LABEL_NODE).unwrap_or_default();
        if node != cluster::self_node() && Some(node) != wanted.as_deref() {
            docker::remove_service(&state.docker, &w.name).await?;
        }
    }
    let Some(node) = wanted else {
        if current
            .iter()
            .all(|w| w.label(LABEL_NODE) == Some(cluster::self_node()))
        {
            remove_old_secrets(&state.docker, "env", None).await;
            let _ = sqlx::query(
                "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots
                 WHERE slot_name = $1 AND NOT active",
            )
            .bind(SLOT)
            .execute(&state.db)
            .await;
        }
        return Ok(());
    };
    prepare_primary(state).await?;
    let (_, secret) = ensure_secret(&state.docker, "jd-env", "env", &env_file()).await?;
    remove_old_secrets(&state.docker, "env", Some(&secret)).await;
    let db = parse_db(&state.config.database_url).map_err(Error::Internal)?;
    let primary = primary_address(&db);
    let mut labels = HashMap::from([
        (MANAGED.to_string(), "true".to_string()),
        (LABEL_KIND.to_string(), "standby".to_string()),
        (LABEL_NODE.to_string(), node.clone()),
        (LABEL_IMAGE.to_string(), own_image(state).await),
        (LABEL_TLS.to_string(), state.config.tls().to_string()),
    ]);
    if let Ok(domain) = std::env::var("JD_DASHBOARD_DOMAIN") {
        if !domain.is_empty() {
            labels.insert(LABEL_DASHBOARD.into(), domain);
        }
    }
    let version = hash(&[
        &primary.0,
        &primary.1.to_string(),
        &db.user,
        &db.password,
        &db.name,
        &format!("{labels:?}"),
    ]);
    labels.insert(LABEL_CONFIG.into(), version.clone());
    let name = standby_name(&node);
    if current
        .iter()
        .any(|w| w.name == name && w.label(LABEL_CONFIG) == Some(version.as_str()))
    {
        return Ok(());
    }
    put_service(&state.docker, standby_spec(&node, &db, primary, labels)).await
}

#[derive(Serialize)]
pub struct Resilience {
    /// Managers other than this server, each running an edge Traefik.
    edges: Vec<Edge>,
    /// Certificates the edges serve, copied from the issuer.
    certificates: usize,
    managers: usize,
    standby: Option<Standby>,
    /// Run on the standby's server to move justdeploy there.
    promote_command: String,
}

#[derive(Serialize)]
pub struct Edge {
    id: String,
    hostname: String,
    address: String,
    running: bool,
}

#[derive(Serialize)]
pub struct Standby {
    node: String,
    hostname: String,
    /// copying | streaming | catchup | stopped (not connected)
    state: String,
    lag_bytes: Option<i64>,
    lag_seconds: Option<f64>,
}

pub async fn status(state: &SharedState) -> Result<Resilience> {
    let nodes = state.docker.list_nodes(None::<ListNodesOptions>).await?;
    let tasks = docker::tasks(&state.docker, EDGE).await.unwrap_or_default();
    let edges = nodes
        .iter()
        .filter(|n| is_manager(n) && n.id.as_deref() != Some(cluster::self_node()))
        .map(|n| {
            let id = n.id.clone().unwrap_or_default();
            Edge {
                running: tasks
                    .iter()
                    .any(|t| t.node_id.as_deref() == Some(id.as_str()) && docker::task_running(t)),
                hostname: hostname(n),
                address: cluster::node_address(n),
                id,
            }
        })
        .collect();
    let certificates = match (&issuer(state).await, state.config.tls()) {
        (Some((docker, container, _)), true) => {
            match docker::exec(docker, container, "root", &format!("cat {ACME_FILE}")).await {
                Ok((0, text)) => {
                    let config: Value =
                        serde_json::from_str(&edge_config(Some(&text), None)).unwrap_or_default();
                    config["tls"]["certificates"].as_array().map_or(0, Vec::len)
                }
                _ => 0,
            }
        }
        _ => 0,
    };
    let standby = match standby_node(state).await? {
        Some(node) if node != cluster::self_node() => {
            let row = sqlx::query!(
                r#"SELECT r.state, pg_wal_lsn_diff(pg_current_wal_lsn(), r.replay_lsn)::bigint AS lag,
                          extract(epoch FROM r.replay_lag)::float8 AS seconds
                   FROM pg_replication_slots s JOIN pg_stat_replication r ON r.pid = s.active_pid
                   WHERE s.slot_name = $1"#,
                SLOT
            )
            .fetch_optional(&state.db)
            .await?;
            let running = docker::tasks(&state.docker, &standby_name(&node))
                .await
                .unwrap_or_default()
                .iter()
                .any(docker::task_running);
            Some(Standby {
                hostname: nodes
                    .iter()
                    .find(|n| n.id.as_deref() == Some(node.as_str()))
                    .map(hostname)
                    .unwrap_or_default(),
                state: match (&row, running) {
                    (Some(r), _) => r.state.clone().unwrap_or_default(),
                    (None, true) => "copying".into(),
                    (None, false) => "stopped".into(),
                },
                lag_bytes: row.as_ref().and_then(|r| r.lag),
                lag_seconds: row.as_ref().and_then(|r| r.seconds),
                node,
            })
        }
        _ => None,
    };
    Ok(Resilience {
        managers: nodes.iter().filter(|n| is_manager(n)).count(),
        edges,
        certificates,
        standby,
        promote_command: format!(
            "docker run --rm -v /var/run/docker.sock:/var/run/docker.sock {} justdeploy promote",
            own_image(state).await
        ),
    })
}

// ---------------------------------------------------------------- promote

/// `justdeploy promote`, run on the standby's server once the main server is
/// lost: the standby becomes the primary, and justdeploy starts here with the
/// settings it had.
pub async fn promote() -> anyhow::Result<()> {
    let docker = Docker::connect_with_defaults().context("cannot reach Docker on this server")?;
    let swarm = docker.info().await?.swarm.unwrap_or_default();
    if swarm.control_available != Some(true) {
        bail!("run this on a manager of the justdeploy cluster");
    }
    let me = swarm.node_id.unwrap_or_default();
    let name = standby_name(&me);
    let standby = docker
        .inspect_service(&name, None)
        .await
        .map_err(|_| anyhow!("there's no justdeploy standby on this server"))?
        .spec
        .unwrap_or_default();
    let labels = standby.labels.clone().unwrap_or_default();
    let env: HashMap<String, String> = standby
        .task_template
        .as_ref()
        .and_then(|t| t.container_spec.as_ref())
        .and_then(|c| c.env.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            e.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    let container = docker::local_task(&docker, &name)
        .await?
        .ok_or_else(|| anyhow!("the standby database isn't running on this server"))?;

    println!("Promoting the standby database to primary…");
    let (code, output) = docker::exec(
        &docker,
        &container,
        "postgres",
        r#"psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -qAt -c "SELECT CASE WHEN pg_is_in_recovery() THEN pg_promote(true, 120) ELSE true END""#,
    )
    .await?;
    if code != 0 || output.trim() != "t" {
        bail!("the standby didn't become the primary: {}", output.trim());
    }

    println!("Marking this server as where justdeploy runs…");
    for node in docker.list_nodes(None::<ListNodesOptions>).await? {
        let here = node.id.as_deref() == Some(me.as_str());
        if here || label(&node, LABEL_CONTROL_PLANE).is_some() {
            set_node_label(&docker, &node, here.then_some("true")).await?;
        }
    }

    let filters = HashMap::from([("label", vec![format!("{LABEL_KIND}=env")])]);
    let secret = docker
        .list_secrets(Some(
            ListSecretsOptionsBuilder::new().filters(&filters).build(),
        ))
        .await?
        .into_iter()
        .max_by_key(|s| s.created_at.clone())
        .ok_or_else(|| {
            anyhow!(
                "justdeploy's settings weren't found; was the standby set up from the dashboard?"
            )
        })?;
    let secret_id = secret.id.unwrap_or_default();
    let secret_name = secret.spec.and_then(|s| s.name).unwrap_or_default();

    let get = |k: &str| env.get(k).cloned().unwrap_or_default();
    let database_url = format!(
        "postgres://{}:{}@{name}:5432/{}",
        get("POSTGRES_USER"),
        get("POSTGRES_PASSWORD"),
        get("POSTGRES_DB")
    );
    let image = labels
        .get(LABEL_IMAGE)
        .cloned()
        .unwrap_or_else(|| "ghcr.io/vedantnimbarte/justdeploy:latest".into());
    let mut service_labels = HashMap::from([
        (MANAGED.to_string(), "true".to_string()),
        (LABEL_KIND.to_string(), "control-plane".to_string()),
    ]);
    if let Some(domain) = labels.get(LABEL_DASHBOARD) {
        let tls = labels.get(LABEL_TLS).map(String::as_str) == Some("true");
        service_labels.extend([
            ("traefik.enable".to_string(), "true".to_string()),
            ("traefik.swarm.network".to_string(), MESH.to_string()),
            (
                "traefik.http.routers.justdeploy.rule".to_string(),
                format!("Host(`{domain}`)"),
            ),
            (
                "traefik.http.routers.justdeploy.entrypoints".to_string(),
                if tls { "websecure" } else { "web" }.to_string(),
            ),
            (
                "traefik.http.services.justdeploy.loadbalancer.server.port".to_string(),
                "8080".to_string(),
            ),
        ]);
        if tls {
            service_labels.insert(
                "traefik.http.routers.justdeploy.tls.certresolver".to_string(),
                "le".to_string(),
            );
        }
    }
    let spec = ServiceSpec {
        name: Some(MAIN.into()),
        labels: Some(service_labels),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(image.clone()),
                env: Some(vec![
                    "JD_ENV_FILE=/run/secrets/jd.env".to_string(),
                    format!("DATABASE_URL={database_url}"),
                    "JD_PROMOTED=1".to_string(),
                ]),
                mounts: Some(vec![docker_socket(false), volume("jd-backups", "/backups")]),
                secrets: Some(vec![secret_file(secret_id, secret_name, "jd.env")]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={me}")]),
                ..Default::default()
            }),
            networks: networks(&[CONTROL, MESH]),
            restart_policy: restart_always(),
            ..Default::default()
        }),
        mode: Some(ServiceSpecMode {
            replicated: Some(ServiceSpecModeReplicated { replicas: Some(1) }),
            ..Default::default()
        }),
        endpoint_spec: host_ports(&[8080]),
        ..Default::default()
    };
    println!("Starting justdeploy ({image}) on this server…");
    docker::create_service(&docker, spec, None).await?;
    let address = docker
        .info()
        .await?
        .swarm
        .and_then(|s| s.node_addr)
        .unwrap_or_else(|| "this server".into());
    println!();
    println!("Done. justdeploy runs here now: http://{address}:8080");
    println!("- Point the dashboard's domain (and any DNS that named only the old server) here.");
    println!("- If the old server comes back, justdeploy won't start on it again; remove it");
    println!("  there with `docker compose down`, then drain and remove it under Servers.");
    println!("- Pick a new standby under Servers.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_get_the_issuers_certificates_and_challenges() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nBBB\n-----END PRIVATE KEY-----\n";
        let acme = json!({ "le": { "Account": {}, "Certificates": [
            { "domain": { "main": "app.example.com" },
              "certificate": STANDARD.encode(pem), "key": STANDARD.encode(key), "Store": "default" }
        ]}})
        .to_string();
        let config: Value =
            serde_json::from_str(&edge_config(Some(&acme), Some("justdeploy-traefik-1"))).unwrap();
        assert_eq!(config["tls"]["certificates"][0]["certFile"], pem);
        assert_eq!(config["tls"]["certificates"][0]["keyFile"], key);
        assert_eq!(
            config["http"]["services"]["jd-acme"]["loadBalancer"]["servers"][0]["url"],
            "http://justdeploy-traefik-1:80"
        );
        let plain: Value = serde_json::from_str(&edge_config(None, None)).unwrap();
        assert!(plain.get("http").is_none());
        assert_eq!(plain["tls"]["certificates"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn database_urls_split_with_escapes() {
        let db = parse_db("postgres://justdeploy:p%40ss@postgres:5433/justdeploy").unwrap();
        assert_eq!(db.password, "p@ss");
        assert_eq!((db.host.as_str(), db.port), ("postgres", 5433));
        assert_eq!(db.name, "justdeploy");
    }

    #[test]
    fn the_env_file_quotes_values() {
        std::env::set_var("JD_TEST_QUOTE", r#"a"b\c"#);
        assert!(env_file().contains(r#"JD_TEST_QUOTE="a\"b\\c""#));
    }
}
