//! The cluster. Every install is a Docker Swarm: a single server is a cluster of
//! one, and more join with the command on the Servers page.
//!
//! - `jd-mesh`, an overlay network, joins Traefik and every routed service, so
//!   Traefik reaches copies on any server.
//! - `jd-control`, an encrypted overlay, joins only the control plane and the
//!   node agents. Each server other than this one runs an agent
//!   (`jd-node-<id>`) that forwards its Docker API there, so logs, terminals,
//!   metrics and backups work wherever a copy runs.
//! - With HTTPS on and more than one server, built images go to a private
//!   registry at `registry.<base domain>` (behind Traefik, with basic auth), so
//!   every server can pull them. Without it, apps built here stay on this server.

use crate::{
    docker::{self, spec, LABEL_KIND, LABEL_SERVICE, MANAGED},
    error::{bad_request, Error, Result},
    models::Service,
    names, services,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use base64::{engine::general_purpose::STANDARD, Engine};
use bollard::{
    auth::DockerCredentials,
    models::{
        EndpointSettings, LocalNodeState, Mount, MountType, NetworkAttachmentConfig,
        NetworkConnectRequest, NodeSpecAvailabilityEnum, ServiceSpec, ServiceSpecMode,
        ServiceSpecModeReplicated, SwarmInitRequest, TaskSpec, TaskSpecContainerSpec,
        TaskSpecLogDriver, TaskSpecPlacement, TaskSpecRestartPolicy,
        TaskSpecRestartPolicyConditionEnum,
    },
    query_parameters::{
        DeleteNodeOptionsBuilder, ListContainersOptionsBuilder, ListNodesOptions,
        ListTasksOptionsBuilder, UpdateNodeOptionsBuilder,
    },
    Docker, API_DEFAULT_VERSION,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const MESH: &str = "jd-mesh";
pub const CONTROL: &str = "jd-control";
const REGISTRY: &str = "jd-registry";
const REGISTRY_USER: &str = "justdeploy";
const AGENT_PORT: u16 = 2375;
const LABEL_REGISTRY_HOST: &str = "jd.registry-host";

static SELF_NODE: OnceLock<String> = OnceLock::new();
/// The control plane's container name, when it runs in one: how the status
/// page proxy on `jd-mesh` reaches it.
static CONTROL_PLANE: OnceLock<String> = OnceLock::new();
/// The Traefik and Postgres started with justdeploy (compose): container id and name.
static COMPOSE_TRAEFIK: OnceLock<(String, String)> = OnceLock::new();
static COMPOSE_POSTGRES: OnceLock<(String, String)> = OnceLock::new();
const LABEL_NODE: &str = "jd.node";

/// The compose Traefik, which issues certificates: (container id, name).
pub fn compose_traefik() -> Option<(&'static str, &'static str)> {
    COMPOSE_TRAEFIK
        .get()
        .map(|(id, name)| (id.as_str(), name.as_str()))
}

/// The compose Postgres holding justdeploy's data: its container id.
pub fn compose_postgres() -> Option<&'static str> {
    COMPOSE_POSTGRES.get().map(|(id, _)| id.as_str())
}

/// Its name, which reaches it on `jd-control`.
pub fn compose_postgres_name() -> Option<&'static str> {
    COMPOSE_POSTGRES.get().map(|(_, name)| name.as_str())
}
const STATUS: &str = "jd-status";
const LABEL_STATUS_HOSTS: &str = "jd.status-hosts";

/// This server's Swarm node id: the one the control plane runs on.
pub fn self_node() -> &'static str {
    SELF_NODE.get().map(String::as_str).unwrap_or_default()
}

/// At startup: a Swarm (started here if there is none), the two overlays, the
/// agents and the registry.
pub async fn ensure(state: &SharedState) -> anyhow::Result<()> {
    let swarm = state.docker.info().await?.swarm.unwrap_or_default();
    if swarm.local_node_state != Some(LocalNodeState::ACTIVE) {
        tracing::info!("starting a Swarm on this server");
        state
            .docker
            .init_swarm(SwarmInitRequest {
                listen_addr: Some("0.0.0.0:2377".into()),
                advertise_addr: state.config.advertise_addr.clone(),
                ..Default::default()
            })
            .await
            .map_err(|e| {
                anyhow!(
                    "could not start a Swarm on this server ({e}). If it has several addresses, \
                     set JD_ADVERTISE_ADDR to the one other servers reach it on"
                )
            })?;
    } else if swarm.control_available != Some(true) {
        bail!("this server is a Swarm worker; justdeploy has to run on a manager");
    }
    let node = state
        .docker
        .info()
        .await?
        .swarm
        .and_then(|s| s.node_id)
        .ok_or_else(|| anyhow!("Docker didn't report this server's Swarm node id"))?;
    crate::ha::claim(&state.docker, &node).await?;
    let _ = SELF_NODE.set(node);
    docker::ensure_overlay(&state.docker, MESH, false).await?;
    docker::ensure_overlay(&state.docker, CONTROL, true).await?;
    join_networks(state).await;
    refresh(state.clone())
        .await
        .map_err(|e| anyhow!("setting up the cluster: {e}"))?;
    Ok(())
}

/// The control plane joins `jd-control`, and Traefik (from the same compose
/// project) joins `jd-mesh`. Skipped when the control plane isn't in a
/// container, as in development.
async fn join_networks(state: &SharedState) {
    let Ok(hostname) = std::env::var("HOSTNAME") else {
        return;
    };
    let Ok(me) = state.docker.inspect_container(&hostname, None).await else {
        return;
    };
    let id = me.id.unwrap_or_default();
    connect(state, CONTROL, &id).await;
    // Status pages reach the control plane over jd-mesh.
    connect(state, MESH, &id).await;
    if let Some(name) = me.name.as_deref() {
        let _ = CONTROL_PLANE.set(name.trim_start_matches('/').to_string());
    }
    let project = me
        .config
        .and_then(|c| c.labels)
        .and_then(|l| l.get("com.docker.compose.project").cloned());
    let Some(project) = project else { return };
    // Traefik routes Swarm services on jd-mesh; a standby copies Postgres over jd-control.
    for (service, network, slot) in [
        ("traefik", MESH, &COMPOSE_TRAEFIK),
        ("postgres", CONTROL, &COMPOSE_POSTGRES),
    ] {
        let filters = HashMap::from([(
            "label",
            vec![
                format!("com.docker.compose.project={project}"),
                format!("com.docker.compose.service={service}"),
            ],
        )]);
        let options = ListContainersOptionsBuilder::new()
            .filters(&filters)
            .build();
        for found in state
            .docker
            .list_containers(Some(options))
            .await
            .unwrap_or_default()
        {
            let id = found.id.unwrap_or_default();
            connect(state, network, &id).await;
            let name = found
                .names
                .and_then(|n| n.into_iter().next())
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string();
            let _ = slot.set((id, name));
        }
    }
}

async fn connect(state: &SharedState, network: &str, container: &str) {
    let request = NetworkConnectRequest {
        container: container.to_string(),
        endpoint_config: Some(EndpointSettings::default()),
    };
    if let Err(error) = state.docker.connect_network(network, request).await {
        // Already connected is the usual answer after the first start.
        if !error.to_string().contains("already exists") {
            tracing::warn!(%network, %error, "could not join the network");
        }
    }
}

/// Every minute: an agent on each other server (and none for servers that
/// left), and the registry when HTTPS is on.
pub async fn refresh(state: SharedState) -> Result<()> {
    let nodes = state.docker.list_nodes(None::<ListNodesOptions>).await?;
    let wanted: Vec<String> = nodes
        .iter()
        .filter_map(|n| n.id.clone())
        .filter(|id| id != self_node())
        .collect();
    let agents = docker::workloads(&state.docker, &[(LABEL_KIND, "agent")]).await?;
    for node in &wanted {
        let name = names::node_agent(node);
        if !agents.iter().any(|a| a.name == name) {
            docker::create_service(
                &state.docker,
                agent_spec(state.config.builder_image.clone(), node),
                None,
            )
            .await?;
        }
    }
    for agent in agents {
        if !wanted.iter().any(|n| names::node_agent(n) == agent.name) {
            docker::remove_service(&state.docker, &agent.name).await?;
        }
    }
    if let Some(control_plane) = CONTROL_PLANE.get() {
        let hosts = crate::uptime::status_hosts(&state).await?.join(",");
        let current = docker::workloads(&state.docker, &[(LABEL_KIND, "status")]).await?;
        let shown = current.first().and_then(|s| s.label(LABEL_STATUS_HOSTS));
        let here = current.first().and_then(|s| s.label(LABEL_NODE)) == Some(self_node());
        if hosts.is_empty() {
            if !current.is_empty() {
                docker::remove_service(&state.docker, STATUS).await?;
            }
        } else if shown != Some(hosts.as_str()) || !here {
            let spec = status_spec(&state, control_plane, &hosts);
            docker::create_service(&state.docker, spec, None).await?;
        }
    }
    if state.config.tls() {
        let host = registry_host(&services::base_domain(&state).await?);
        let current = docker::workloads(&state.docker, &[(LABEL_KIND, "registry")]).await?;
        let here = current.first().and_then(|r| r.label(LABEL_NODE)) == Some(self_node());
        if current.first().and_then(|r| r.label(LABEL_REGISTRY_HOST)) != Some(host.as_str())
            || !here
        {
            docker::create_service(&state.docker, registry_spec(&state, &host), None).await?;
        }
    }
    // Routing on other managers and the standby: each failing alone.
    if let Err(error) = crate::ha::refresh_routing(&state).await {
        tracing::warn!(%error, "edge routing not updated");
    }
    if let Err(error) = crate::ha::refresh_standby(&state).await {
        tracing::warn!(%error, "standby not updated");
    }
    Ok(())
}

fn replicated(replicas: i64) -> Option<ServiceSpecMode> {
    Some(ServiceSpecMode {
        replicated: Some(ServiceSpecModeReplicated {
            replicas: Some(replicas),
        }),
        ..Default::default()
    })
}

fn restart_always() -> Option<TaskSpecRestartPolicy> {
    Some(TaskSpecRestartPolicy {
        condition: Some(TaskSpecRestartPolicyConditionEnum::ANY),
        delay: Some(5_000_000_000),
        ..Default::default()
    })
}

fn bounded_logs() -> Option<TaskSpecLogDriver> {
    let logs = spec::bounded_logs();
    Some(TaskSpecLogDriver {
        name: logs.typ,
        options: logs.config,
    })
}

/// Forwards a server's Docker API onto `jd-control`, where only the control
/// plane and the other agents are.
fn agent_spec(image: String, node: &str) -> ServiceSpec {
    let labels = HashMap::from([
        (MANAGED.to_string(), "true".to_string()),
        (LABEL_KIND.to_string(), "agent".to_string()),
    ]);
    ServiceSpec {
        name: Some(names::node_agent(node)),
        labels: Some(labels),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(image),
                args: Some(vec![
                    "socat".into(),
                    format!("TCP-LISTEN:{AGENT_PORT},fork,reuseaddr"),
                    "UNIX-CONNECT:/var/run/docker.sock".into(),
                ]),
                mounts: Some(vec![Mount {
                    target: Some("/var/run/docker.sock".into()),
                    source: Some("/var/run/docker.sock".into()),
                    typ: Some(MountType::BIND),
                    ..Default::default()
                }]),
                labels: Some(HashMap::from([(MANAGED.to_string(), "agent".to_string())])),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={node}")]),
                ..Default::default()
            }),
            networks: Some(vec![NetworkAttachmentConfig {
                target: Some(CONTROL.into()),
                ..Default::default()
            }]),
            restart_policy: restart_always(),
            log_driver: bounded_logs(),
            ..Default::default()
        }),
        mode: replicated(1),
        ..Default::default()
    }
}

/// Routes every published status page hostname to the control plane, which
/// isn't a Swarm service itself, through a forwarder on `jd-mesh`.
fn status_spec(state: &SharedState, control_plane: &str, hosts: &str) -> ServiceSpec {
    let rule = hosts
        .split(',')
        .map(|h| format!("Host(`{h}`)"))
        .collect::<Vec<_>>()
        .join(" || ");
    let r = STATUS;
    let mut labels = HashMap::from([
        (MANAGED.to_string(), "true".to_string()),
        (LABEL_KIND.to_string(), "status".to_string()),
        (LABEL_STATUS_HOSTS.to_string(), hosts.to_string()),
        (LABEL_NODE.to_string(), self_node().to_string()),
        ("traefik.enable".to_string(), "true".to_string()),
        ("traefik.swarm.network".to_string(), MESH.to_string()),
        (format!("traefik.http.routers.{r}.rule"), rule),
        (
            format!("traefik.http.services.{r}.loadbalancer.server.port"),
            "8080".to_string(),
        ),
    ]);
    if state.config.tls() {
        labels.insert(
            format!("traefik.http.routers.{r}.entrypoints"),
            "websecure".into(),
        );
        labels.insert(
            format!("traefik.http.routers.{r}.tls.certresolver"),
            "le".into(),
        );
    } else {
        labels.insert(
            format!("traefik.http.routers.{r}.entrypoints"),
            "web".into(),
        );
    }
    ServiceSpec {
        name: Some(STATUS.into()),
        labels: Some(labels),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some(state.config.builder_image.clone()),
                args: Some(vec![
                    "socat".into(),
                    "TCP-LISTEN:8080,fork,reuseaddr".into(),
                    format!("TCP:{control_plane}:8080"),
                ]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={}", self_node())]),
                ..Default::default()
            }),
            networks: Some(vec![NetworkAttachmentConfig {
                target: Some(MESH.into()),
                ..Default::default()
            }]),
            restart_policy: restart_always(),
            log_driver: bounded_logs(),
            ..Default::default()
        }),
        mode: replicated(1),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- registry

pub fn registry_host(base_domain: &str) -> String {
    format!("registry.{base_domain}")
}

/// Derived from the master key, so it never has to be stored or shown.
fn registry_password(state: &SharedState) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(&state.config.master_key)
        .expect("HMAC takes keys of any length");
    mac.update(b"justdeploy registry");
    hex::encode(mac.finalize().into_bytes())[..32].to_string()
}

fn registry_spec(state: &SharedState, host: &str) -> ServiceSpec {
    // Traefik checks the password; the registry itself is only on jd-mesh.
    let digest = STANDARD.encode(Sha1::digest(registry_password(state).as_bytes()));
    let r = "jd-registry";
    let labels = HashMap::from([
        (MANAGED.to_string(), "true".to_string()),
        (LABEL_KIND.to_string(), "registry".to_string()),
        (LABEL_REGISTRY_HOST.to_string(), host.to_string()),
        (LABEL_NODE.to_string(), self_node().to_string()),
        ("traefik.enable".to_string(), "true".to_string()),
        ("traefik.swarm.network".to_string(), MESH.to_string()),
        (
            format!("traefik.http.routers.{r}.rule"),
            format!("Host(`{host}`)"),
        ),
        (
            format!("traefik.http.routers.{r}.entrypoints"),
            "websecure".to_string(),
        ),
        (
            format!("traefik.http.routers.{r}.tls.certresolver"),
            "le".to_string(),
        ),
        (
            format!("traefik.http.routers.{r}.middlewares"),
            format!("{r}-auth"),
        ),
        (
            format!("traefik.http.middlewares.{r}-auth.basicauth.users"),
            format!("{REGISTRY_USER}:{{SHA}}{digest}"),
        ),
        (
            format!("traefik.http.services.{r}.loadbalancer.server.port"),
            "5000".to_string(),
        ),
    ]);
    ServiceSpec {
        name: Some(REGISTRY.into()),
        labels: Some(labels),
        task_template: Some(TaskSpec {
            container_spec: Some(TaskSpecContainerSpec {
                image: Some("registry:2".into()),
                env: Some(vec!["REGISTRY_STORAGE_DELETE_ENABLED=true".into()]),
                mounts: Some(vec![Mount {
                    target: Some("/var/lib/registry".into()),
                    source: Some(REGISTRY.into()),
                    typ: Some(MountType::VOLUME),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            placement: Some(TaskSpecPlacement {
                constraints: Some(vec![format!("node.id=={}", self_node())]),
                ..Default::default()
            }),
            networks: Some(vec![NetworkAttachmentConfig {
                target: Some(MESH.into()),
                ..Default::default()
            }]),
            restart_policy: restart_always(),
            log_driver: bounded_logs(),
            ..Default::default()
        }),
        mode: replicated(1),
        ..Default::default()
    }
}

/// Where built images go, when they need to go anywhere: with HTTPS on and
/// more than one server. Otherwise they stay on this server, and so do the
/// services built from them.
pub async fn registry(state: &SharedState) -> Result<Option<(String, DockerCredentials)>> {
    if !state.config.tls() {
        return Ok(None);
    }
    let servers = state
        .docker
        .list_nodes(None::<ListNodesOptions>)
        .await?
        .len();
    if servers < 2 {
        return Ok(None);
    }
    let host = registry_host(&services::base_domain(state).await?);
    Ok(Some((host.clone(), credentials(state, &host))))
}

/// What a service's servers pull its image with. None for public images.
pub async fn pull_credentials(state: &SharedState, image: &str) -> Option<DockerCredentials> {
    let host = registry_host(&services::base_domain(state).await.ok()?);
    image
        .starts_with(&format!("{host}/"))
        .then(|| credentials(state, &host))
}

fn credentials(state: &SharedState, host: &str) -> DockerCredentials {
    DockerCredentials {
        username: Some(REGISTRY_USER.into()),
        password: Some(registry_password(state)),
        serveraddress: Some(host.to_string()),
        ..Default::default()
    }
}

/// Daily: tags in the registry that no deployment needs any more (not running,
/// not among a service's newest kept for rollback) are deleted, then the
/// registry's garbage collector frees their layers.
pub async fn clean_registry(state: SharedState) -> Result<()> {
    if !state.config.tls() {
        return Ok(());
    }
    let Some(container) = docker::local_task(&state.docker, REGISTRY).await? else {
        return Ok(());
    };
    let host = registry_host(&services::base_domain(&state).await?);
    let keep: std::collections::HashSet<String> = sqlx::query_scalar!(
        r#"SELECT DISTINCT image_ref AS "image!" FROM (
             SELECT image_ref, status, row_number() OVER (
               PARTITION BY service_id, kind, pr_number ORDER BY created_at DESC) AS n
             FROM deployments WHERE image_ref IS NOT NULL) d
           WHERE n <= $1 OR status IN ('queued', 'building', 'deploying', 'running')"#,
        crate::queue::deploy::KEEP_IMAGES
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();

    // In its container the control plane is on jd-mesh with the registry;
    // otherwise (development) it goes the public way, with the password.
    let (base, auth) = if CONTROL_PLANE.get().is_some() {
        (format!("http://{REGISTRY}:5000"), None)
    } else {
        (format!("https://{host}"), Some(registry_password(&state)))
    };
    let deleted = prune_registry(&base, auth.as_deref(), &host, &keep)
        .await
        .map_err(Error::Internal)?;
    if deleted == 0 {
        return Ok(());
    }
    // Collecting while an image is pushed could drop its new layers; wait for a quiet day.
    let busy = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM deployments WHERE status IN ('building', 'deploying')"#
    )
    .fetch_one(&state.db)
    .await?;
    if busy > 0 {
        return Ok(());
    }
    let (code, output) = docker::exec(
        &state.docker,
        &container,
        "root",
        "registry garbage-collect --delete-untagged /etc/docker/registry/config.yml",
    )
    .await
    .map_err(Error::Internal)?;
    if code != 0 {
        tracing::warn!(output = output.trim(), "registry garbage collection failed");
    } else {
        tracing::info!(deleted, "removed old images from the registry");
    }
    Ok(())
}

/// Deletes every manifest in the registry at `base` that no tag in `keep`
/// (as `host/repo:tag`) points at. Returns how many went.
async fn prune_registry(
    base: &str,
    password: Option<&str>,
    host: &str,
    keep: &std::collections::HashSet<String>,
) -> anyhow::Result<usize> {
    #[derive(serde::Deserialize)]
    struct Catalog {
        repositories: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Tags {
        tags: Option<Vec<String>>,
    }
    const MANIFESTS: &str = "application/vnd.oci.image.index.v1+json, \
        application/vnd.docker.distribution.manifest.list.v2+json, \
        application/vnd.oci.image.manifest.v1+json, \
        application/vnd.docker.distribution.manifest.v2+json";
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let request = |method: reqwest::Method, url: String| {
        let r = client.request(method, url);
        match password {
            Some(p) => r.basic_auth(REGISTRY_USER, Some(p)),
            None => r,
        }
    };
    // The registry hands the catalog out a page (at most 1000) at a time.
    let mut repositories: Vec<String> = Vec::new();
    loop {
        let after = repositories
            .last()
            .map(|r| format!("&last={r}"))
            .unwrap_or_default();
        let page: Catalog = request(
            reqwest::Method::GET,
            format!("{base}/v2/_catalog?n=1000{after}"),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
        let full = page.repositories.len() == 1000;
        repositories.extend(page.repositories);
        if !full {
            break;
        }
    }
    let mut deleted = 0;
    for repo in repositories {
        let tags: Tags = request(reqwest::Method::GET, format!("{base}/v2/{repo}/tags/list"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // A digest goes only when no kept tag shares it.
        let mut kept = std::collections::HashSet::new();
        let mut stale = std::collections::HashSet::new();
        for tag in tags.tags.unwrap_or_default() {
            let digest = request(
                reqwest::Method::HEAD,
                format!("{base}/v2/{repo}/manifests/{tag}"),
            )
            .header(reqwest::header::ACCEPT, MANIFESTS)
            .send()
            .await?
            .error_for_status()?
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
            let Some(digest) = digest else { continue };
            if keep.contains(&format!("{host}/{repo}:{tag}")) {
                kept.insert(digest);
            } else {
                stale.insert(digest);
            }
        }
        for digest in stale.difference(&kept) {
            request(
                reqwest::Method::DELETE,
                format!("{base}/v2/{repo}/manifests/{digest}"),
            )
            .send()
            .await?
            .error_for_status()?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

// ---------------------------------------------------------------- placement

/// Where a service's copies may run: its pinned server, this server for an
/// image only this server has, and servers with its label.
pub fn constraints(service: &Service, image: &str) -> Vec<String> {
    let mut out = Vec::new();
    match &service.node_id {
        Some(node) => out.push(format!("node.id=={node}")),
        None if names::is_built_image(image) && !image.contains('/') => {
            out.push(format!("node.id=={}", self_node()))
        }
        None => {}
    }
    if let Some((key, value)) = service
        .node_label
        .as_deref()
        .and_then(|l| l.split_once('='))
    {
        out.push(format!("node.labels.{}=={}", key.trim(), value.trim()));
    }
    out
}

/// A Docker client for one server: this one directly, others through their agent.
pub fn node_docker(state: &SharedState, node: &str) -> anyhow::Result<Docker> {
    if node.is_empty() || node == self_node() {
        return Ok(state.docker.clone());
    }
    let address = format!("http://{}:{AGENT_PORT}", names::node_agent(node));
    Ok(Docker::connect_with_http(
        &address,
        120,
        API_DEFAULT_VERSION,
    )?)
}

/// A running copy of a service's production workload: its server's Docker and
/// the container. Falls back to a plain container on this server, as services
/// ran before the cluster.
pub async fn running_container(
    state: &SharedState,
    service_id: Uuid,
) -> anyhow::Result<Option<(Docker, String)>> {
    let id = service_id.to_string();
    let labels = [(LABEL_SERVICE, id.as_str()), (LABEL_KIND, "production")];
    for workload in docker::workloads(&state.docker, &labels).await? {
        for task in docker::tasks(&state.docker, &workload.name).await? {
            if !docker::task_running(&task) {
                continue;
            }
            let container = task
                .status
                .as_ref()
                .and_then(|s| s.container_status.as_ref())
                .and_then(|c| c.container_id.clone());
            if let (Some(container), Some(node)) = (container, task.node_id.as_deref()) {
                return Ok(Some((node_docker(state, node)?, container)));
            }
        }
    }
    let legacy = docker::list(&state.docker, &labels)
        .await?
        .into_iter()
        .find(docker::is_running)
        .and_then(|c| c.id);
    Ok(legacy.map(|c| (state.docker.clone(), c)))
}

// ---------------------------------------------------------------- servers

/// The address a server is reached on. Swarm sometimes reports a manager's as
/// 0.0.0.0; its management address says where it really is.
pub fn node_address(node: &bollard::models::Node) -> String {
    let reported = node
        .status
        .as_ref()
        .and_then(|s| s.addr.clone())
        .unwrap_or_default();
    if reported.is_empty() || reported == "0.0.0.0" {
        if let Some(addr) = node.manager_status.as_ref().and_then(|m| m.addr.as_deref()) {
            return addr
                .rsplit_once(':')
                .map_or(addr, |(host, _)| host)
                .to_string();
        }
    }
    reported
}

/// The addresses of every manager that's up: each one routes traffic.
pub async fn manager_addresses(state: &SharedState) -> Vec<std::net::IpAddr> {
    let Ok(nodes) = state.docker.list_nodes(None::<ListNodesOptions>).await else {
        return vec![];
    };
    nodes
        .into_iter()
        .filter(|n| n.manager_status.is_some())
        .filter_map(|n| node_address(&n).parse().ok())
        .collect()
}

#[derive(Serialize)]
pub struct Server {
    id: String,
    hostname: String,
    /// manager | worker
    role: String,
    leader: bool,
    /// The server justdeploy itself runs on.
    this: bool,
    /// ready | down | unknown | disconnected
    state: String,
    /// active | drain | pause
    availability: String,
    address: String,
    cpus: f64,
    memory_bytes: i64,
    platform: String,
    docker_version: String,
    labels: HashMap<String, String>,
    /// Copies running on it.
    tasks: usize,
    usage: Option<Usage>,
}

#[derive(Serialize)]
struct Usage {
    cpu_percent: f64,
    memory_used: i64,
    memory_total: i64,
    disk_used: i64,
    disk_total: i64,
    sampled_at: DateTime<Utc>,
}

pub async fn servers(state: &SharedState) -> Result<Vec<Server>> {
    let nodes = state.docker.list_nodes(None::<ListNodesOptions>).await?;
    let running = HashMap::from([("desired-state", vec!["running".to_string()])]);
    let tasks = state
        .docker
        .list_tasks(Some(
            ListTasksOptionsBuilder::new().filters(&running).build(),
        ))
        .await?;
    let samples: HashMap<String, Usage> = sqlx::query!(
        "SELECT DISTINCT ON (node_id) node_id, ts, cpu_percent, memory_used, memory_total,
             disk_used, disk_total
         FROM host_samples ORDER BY node_id, ts DESC"
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|r| {
        (
            r.node_id,
            Usage {
                cpu_percent: r.cpu_percent,
                memory_used: r.memory_used,
                memory_total: r.memory_total,
                disk_used: r.disk_used,
                disk_total: r.disk_total,
                sampled_at: r.ts,
            },
        )
    })
    .collect();
    let mut samples = samples;
    let mut out: Vec<Server> = nodes
        .into_iter()
        .map(|n| {
            let address = node_address(&n);
            let id = n.id.unwrap_or_default();
            let spec = n.spec.unwrap_or_default();
            let description = n.description.unwrap_or_default();
            let resources = description.resources.unwrap_or_default();
            let platform = description.platform.unwrap_or_default();
            let status = n.status.unwrap_or_default();
            Server {
                hostname: description.hostname.unwrap_or_default(),
                role: spec.role.map(|r| r.to_string()).unwrap_or_default(),
                leader: n.manager_status.and_then(|m| m.leader).unwrap_or(false),
                this: id == self_node(),
                state: status.state.map(|s| s.to_string()).unwrap_or_default(),
                availability: spec.availability.map(|a| a.to_string()).unwrap_or_default(),
                address,
                cpus: resources.nano_cpus.unwrap_or(0) as f64 / 1e9,
                memory_bytes: resources.memory_bytes.unwrap_or(0),
                platform: format!(
                    "{} {}",
                    platform.os.unwrap_or_default(),
                    platform.architecture.unwrap_or_default()
                ),
                docker_version: description
                    .engine
                    .and_then(|e| e.engine_version)
                    .unwrap_or_default(),
                // justdeploy's own (`jd.`) aren't the server's to edit.
                labels: spec
                    .labels
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(k, _)| !k.starts_with("jd."))
                    .collect(),
                tasks: tasks
                    .iter()
                    .filter(|t| t.node_id.as_deref() == Some(id.as_str()))
                    .filter(|t| docker::task_running(t))
                    .count(),
                usage: samples.remove(&id),
                id,
            }
        })
        .collect();
    // This server first, then by name.
    out.sort_by(|a, b| b.this.cmp(&a.this).then(a.hostname.cmp(&b.hostname)));
    Ok(out)
}

#[derive(Serialize)]
pub struct Join {
    /// Run on the new server.
    command: String,
    /// What other servers reach this one on.
    manager_address: String,
}

pub async fn join(state: &SharedState) -> Result<Join> {
    let swarm = state.docker.inspect_swarm().await?;
    let token = swarm
        .join_tokens
        .and_then(|t| t.worker)
        .ok_or_else(|| Error::Internal(anyhow!("Docker gave no join token")))?;
    let address = state
        .docker
        .info()
        .await?
        .swarm
        .and_then(|s| s.node_addr)
        .unwrap_or_default();
    Ok(Join {
        command: format!("docker swarm join --token {token} {address}:2377"),
        manager_address: address,
    })
}

/// `availability`: active, drain (move everything off) or pause (take nothing
/// new). `role`: manager (can run the cluster, routes traffic) or worker.
/// `labels` replaces the server's own labels; justdeploy's (`jd.`) stay.
pub async fn update_server(
    state: &SharedState,
    id: &str,
    availability: Option<&str>,
    role: Option<&str>,
    labels: Option<HashMap<String, String>>,
) -> Result<()> {
    let node = state.docker.inspect_node(id).await?;
    let version = node.version.and_then(|v| v.index).unwrap_or(0);
    let mut spec = node.spec.unwrap_or_default();
    if let Some(r) = role {
        if id == self_node() && r != "manager" {
            return Err(bad_request(
                "justdeploy runs on this server; it has to stay a manager",
            ));
        }
        spec.role = Some(match r {
            "manager" => bollard::models::NodeSpecRoleEnum::MANAGER,
            "worker" => bollard::models::NodeSpecRoleEnum::WORKER,
            _ => return Err(bad_request("role must be manager or worker")),
        });
    }
    if let Some(a) = availability {
        if id == self_node() && a != "active" {
            return Err(bad_request(
                "justdeploy runs on this server; it has to stay active",
            ));
        }
        spec.availability = Some(match a {
            "active" => NodeSpecAvailabilityEnum::ACTIVE,
            "drain" => NodeSpecAvailabilityEnum::DRAIN,
            "pause" => NodeSpecAvailabilityEnum::PAUSE,
            _ => return Err(bad_request("availability must be active, drain or pause")),
        });
    }
    if let Some(labels) = labels {
        for key in labels.keys() {
            let ok = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !ok {
                return Err(bad_request(format!("{key:?} is not a valid label name")));
            }
        }
        let mut merged: HashMap<String, String> = spec
            .labels
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| k.starts_with("jd."))
            .collect();
        merged.extend(labels.into_iter().filter(|(k, _)| !k.starts_with("jd.")));
        spec.labels = Some(merged);
    }
    let options = UpdateNodeOptionsBuilder::new()
        .version(version as i64)
        .build();
    state.docker.update_node(id, spec, options).await?;
    Ok(())
}

/// Takes a server out of the cluster. A server still up has to be drained and
/// leave first (`docker swarm leave` on it); one that's down is removed outright.
pub async fn remove_server(state: &SharedState, id: &str) -> Result<()> {
    if id == self_node() {
        return Err(bad_request(
            "justdeploy runs on this server; it can't be removed",
        ));
    }
    let pinned = sqlx::query_scalar!(
        "SELECT name FROM services WHERE node_id = $1 AND desired_state <> 'deleting'",
        id
    )
    .fetch_all(&state.db)
    .await?;
    if !pinned.is_empty() {
        return Err(bad_request(format!(
            "{} keep their data on this server; move or delete them first",
            pinned.join(", ")
        )));
    }
    let standby = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'standby_node'")
        .fetch_optional(&state.db)
        .await?;
    if standby.as_deref() == Some(id) {
        return Err(bad_request(
            "it keeps justdeploy's standby database; pick another standby first",
        ));
    }
    let node = state.docker.inspect_node(id).await?;
    let down = node
        .status
        .and_then(|s| s.state)
        .map(|s| s.to_string() != "ready")
        .unwrap_or(true);
    if !down {
        return Err(bad_request(
            "it's still connected: drain it, run `docker swarm leave` on it, then remove it",
        ));
    }
    let options = DeleteNodeOptionsBuilder::new().force(true).build();
    state.docker.delete_node(id, Some(options)).await?;
    Ok(())
}

// ---------------------------------------------------------------- autoscaling

/// What autoscaling decides from the load per copy, in percent of what one
/// copy may use: one copy more when the last three minutes averaged above the
/// target, one fewer when the last ten minutes never reached half of it.
pub fn decide(
    replicas: i32,
    min: i32,
    max: i32,
    target: f64,
    recent_average: f64,
    ten_minute_peak: f64,
) -> Option<i32> {
    if recent_average > target && replicas < max {
        Some(replicas + 1)
    } else if ten_minute_peak < target / 2.0 && replicas > min {
        Some(replicas - 1)
    } else if replicas < min || replicas > max {
        Some(replicas.clamp(min, max))
    } else {
        None
    }
}

/// Every minute, for services with autoscaling on.
pub async fn autoscale(state: SharedState) -> Result<()> {
    static LAST: OnceLock<Mutex<HashMap<Uuid, Instant>>> = OnceLock::new();
    let rows = sqlx::query!(
        r#"SELECT s.id, s.name, s.replicas, s.autoscale_min AS "min!", s.autoscale_max AS "max!",
               s.autoscale_cpu, s.cpu_millis,
               (SELECT avg(cpu_percent) FROM metric_samples m
                 WHERE m.service_id = s.id AND m.ts > now() - interval '3 minutes') AS recent,
               (SELECT count(*) FROM metric_samples m
                 WHERE m.service_id = s.id AND m.ts > now() - interval '3 minutes') AS "samples!",
               (SELECT max(cpu_percent) FROM metric_samples m
                 WHERE m.service_id = s.id AND m.ts > now() - interval '10 minutes') AS peak
           FROM services s
           WHERE s.autoscale_max IS NOT NULL AND s.autoscale_min IS NOT NULL
             AND s.desired_state = 'running' AND s.current_deployment_id IS NOT NULL"#
    )
    .fetch_all(&state.db)
    .await?;
    for row in rows {
        // Thirty-second samples: wait for most of three minutes of them.
        let (Some(recent), Some(peak)) = (row.recent, row.peak) else {
            continue;
        };
        if row.samples < 4 {
            continue;
        }
        let since = LAST.get_or_init(Default::default);
        let cooled = since
            .lock()
            .map(|m| {
                m.get(&row.id)
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(180))
            })
            .unwrap_or(true);
        if !cooled {
            continue;
        }
        // Samples add up every copy; 100 is one core, unless a limit says less.
        let copies = row.replicas.max(1) as f64;
        let capacity = row.cpu_millis.map_or(100.0, |m| m as f64 / 10.0);
        let load = |total: f64| total / copies / capacity * 100.0;
        let Some(next) = decide(
            row.replicas,
            row.min,
            row.max,
            row.autoscale_cpu as f64,
            load(recent),
            load(peak),
        ) else {
            continue;
        };
        let id = row.id.to_string();
        let labels = [(LABEL_SERVICE, id.as_str()), (LABEL_KIND, "production")];
        for workload in docker::workloads(&state.docker, &labels).await? {
            docker::scale(&state.docker, &workload.name, next as i64).await?;
        }
        sqlx::query!(
            "UPDATE services SET replicas = $2 WHERE id = $1",
            row.id,
            next
        )
        .execute(&state.db)
        .await?;
        if let Ok(mut m) = since.lock() {
            m.insert(row.id, Instant::now());
        }
        let phrase = format!("scaled to {next} copies:");
        services::audit(&state.db, None, &phrase, &row.name, None).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoscaling_adds_under_load_and_removes_when_quiet() {
        assert_eq!(decide(2, 1, 5, 70.0, 85.0, 90.0), Some(3), "busy: one more");
        assert_eq!(
            decide(5, 1, 5, 70.0, 95.0, 99.0),
            None,
            "already at the most"
        );
        assert_eq!(
            decide(3, 1, 5, 70.0, 20.0, 30.0),
            Some(2),
            "quiet for ten minutes"
        );
        assert_eq!(
            decide(3, 1, 5, 70.0, 20.0, 40.0),
            None,
            "a peak above half the target holds"
        );
        assert_eq!(
            decide(1, 1, 5, 70.0, 5.0, 5.0),
            None,
            "already at the fewest"
        );
        assert_eq!(
            decide(1, 2, 5, 70.0, 50.0, 60.0),
            Some(2),
            "below the minimum after a change"
        );
    }

    /// Against a registry with `jd-test:keep` and `jd-test:same` (one image) and
    /// `jd-test:old` (another): only `old` goes.
    #[tokio::test]
    #[ignore = "needs a registry; set JD_TEST_REGISTRY=http://127.0.0.1:25000"]
    async fn stale_registry_tags_go_and_shared_images_stay() {
        let base = std::env::var("JD_TEST_REGISTRY").unwrap();
        let host = base.trim_start_matches("http://").to_string();
        let keep = std::collections::HashSet::from([format!("{host}/jd-test:keep")]);
        assert_eq!(prune_registry(&base, None, &host, &keep).await.unwrap(), 1);
        let tags: serde_json::Value = reqwest::get(format!("{base}/v2/jd-test/tags/list"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut tags: Vec<&str> = tags["tags"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        tags.sort();
        assert_eq!(tags, vec!["keep", "same"]);
    }

    #[test]
    fn placement_follows_pins_labels_and_local_images() {
        let mut s = crate::docker::spec::tests::service();
        s.node_id = Some("abc".into());
        s.node_label = Some("region = eu".into());
        assert_eq!(
            constraints(&s, "nginx:alpine"),
            vec![
                "node.id==abc".to_string(),
                "node.labels.region==eu".to_string()
            ]
        );
        s.node_id = None;
        s.node_label = None;
        assert!(
            constraints(&s, "nginx:alpine").is_empty(),
            "public images run anywhere"
        );
        assert!(constraints(&s, "registry.example.com/jd-x:1").is_empty());
        assert_eq!(
            constraints(&s, "jd-x:1").len(),
            1,
            "a local build stays here"
        );
    }
}
