//! Thin helpers over bollard. Everything justdeploy creates carries `jd.managed`.

pub mod spec;

use anyhow::{anyhow, Result};
use bollard::{
    auth::DockerCredentials,
    container::LogOutput,
    errors::Error as DockerError,
    models::{
        ContainerCreateBody, ContainerSummary, NetworkCreateRequest, ServiceSpec, ServiceSpecMode,
        ServiceSpecModeReplicated, Task, TaskState, VolumeCreateRequest,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
        ListServicesOptionsBuilder, ListTasksOptionsBuilder, LogsOptionsBuilder,
        RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder, StopContainerOptionsBuilder,
        UpdateServiceOptionsBuilder,
    },
    Docker,
};
use futures::StreamExt;
use std::{collections::HashMap, time::Duration};

pub const MANAGED: &str = "jd.managed";
pub const LABEL_PROJECT: &str = "jd.project";
pub const LABEL_SERVICE: &str = "jd.service";
pub const LABEL_DEPLOYMENT: &str = "jd.deployment";
pub const LABEL_KIND: &str = "jd.kind";
pub const LABEL_PR: &str = "jd.pr";

pub fn is_not_found(error: &DockerError) -> bool {
    matches!(
        error,
        DockerError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn is_conflict(error: &DockerError) -> bool {
    matches!(
        error,
        DockerError::DockerResponseServerError {
            status_code: 409,
            ..
        }
    )
}

/// A network that spans the cluster. Attachable, so plain containers (builds,
/// health checks, runs) can join it as well as Swarm services.
pub async fn ensure_network(docker: &Docker, name: &str) -> Result<()> {
    ensure_overlay(docker, name, false).await
}

/// `encrypted` turns on IPsec between servers, for traffic that must not be read
/// on the wire (the control plane talking to each server's Docker).
pub async fn ensure_overlay(docker: &Docker, name: &str, encrypted: bool) -> Result<()> {
    match docker.inspect_network(name, None).await {
        Ok(_) => return Ok(()),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e.into()),
    }
    let request = NetworkCreateRequest {
        name: name.to_string(),
        driver: Some("overlay".into()),
        attachable: Some(true),
        options: encrypted.then(|| HashMap::from([("encrypted".to_string(), String::new())])),
        labels: Some(HashMap::from([(MANAGED.to_string(), "true".to_string())])),
        ..Default::default()
    };
    match docker.create_network(request).await {
        Ok(_) => Ok(()),
        Err(e) if is_conflict(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub async fn remove_network(docker: &Docker, name: &str) -> Result<()> {
    match docker.remove_network(name).await {
        Err(e) if !is_not_found(&e) => Err(e.into()),
        _ => Ok(()),
    }
}

/// Creating a volume that exists is a no-op in Docker.
pub async fn ensure_volume(docker: &Docker, name: &str) -> Result<()> {
    docker
        .create_volume(VolumeCreateRequest {
            name: Some(name.to_string()),
            labels: Some(HashMap::from([(MANAGED.to_string(), "true".to_string())])),
            ..Default::default()
        })
        .await?;
    Ok(())
}

pub async fn remove_volume(docker: &Docker, name: &str) -> Result<()> {
    match docker
        .remove_volume(name, None::<bollard::query_parameters::RemoveVolumeOptions>)
        .await
    {
        Err(e) if !is_not_found(&e) => Err(e.into()),
        _ => Ok(()),
    }
}

pub async fn image_exists(docker: &Docker, image: &str) -> bool {
    docker.inspect_image(image).await.is_ok()
}

/// Pulls an image, reporting progress as a handful of readable lines rather than
/// the per-layer byte counters Docker streams.
pub async fn pull_image(docker: &Docker, image: &str, mut log: impl FnMut(String)) -> Result<()> {
    let options = CreateImageOptionsBuilder::new().from_image(image).build();
    let mut stream = docker.create_image(Some(options), None, None);
    while let Some(item) = stream.next().await {
        let info = item.map_err(|e| anyhow!("pulling {image} failed: {e}"))?;
        if let Some(detail) = info.error_detail.and_then(|d| d.message) {
            return Err(anyhow!("pulling {image} failed: {detail}"));
        }
        // Per-layer lines carry an id; the summary lines ("Digest", "Status") don't.
        if let (Some(status), None) = (info.status, info.id) {
            log(status);
        }
    }
    Ok(())
}

pub async fn remove_image(docker: &Docker, image: &str) {
    let options = RemoveImageOptionsBuilder::new().force(false).build();
    if let Err(error) = docker.remove_image(image, Some(options), None).await {
        if !is_not_found(&error) {
            tracing::debug!(%image, %error, "image not removed");
        }
    }
}

/// Plain containers (not copies of a Swarm service) carrying every given label
/// (`key=value`): helpers, preview databases, and services from before the cluster.
pub async fn list(docker: &Docker, labels: &[(&str, &str)]) -> Result<Vec<ContainerSummary>> {
    Ok(list_all(docker, labels)
        .await?
        .into_iter()
        .filter(|c| label(c, "com.docker.swarm.task.id").is_none())
        .collect())
}

/// Every container carrying the labels, Swarm copies included.
pub async fn list_all(docker: &Docker, labels: &[(&str, &str)]) -> Result<Vec<ContainerSummary>> {
    let mut filter: Vec<String> = vec![format!("{MANAGED}=true")];
    filter.extend(labels.iter().map(|(k, v)| format!("{k}={v}")));
    let filters = HashMap::from([("label", filter)]);
    let options = ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    Ok(docker.list_containers(Some(options)).await?)
}

pub fn label<'a>(container: &'a ContainerSummary, key: &str) -> Option<&'a str> {
    container.labels.as_ref()?.get(key).map(String::as_str)
}

pub fn is_running(container: &ContainerSummary) -> bool {
    matches!(
        container.state,
        Some(bollard::models::ContainerSummaryStateEnum::RUNNING)
    )
}

pub async fn create_and_start(
    docker: &Docker,
    name: &str,
    body: ContainerCreateBody,
) -> Result<String> {
    // A container of the same name left over from a crash would block the create.
    remove(docker, name).await?;
    let options = CreateContainerOptionsBuilder::new().name(name).build();
    let created = docker.create_container(Some(options), body).await?;
    docker.start_container(&created.id, None).await?;
    Ok(created.id)
}

pub async fn stop(docker: &Docker, id: &str) -> Result<()> {
    let options = StopContainerOptionsBuilder::new().t(10).build();
    match docker.stop_container(id, Some(options)).await {
        Err(e) if !is_not_found(&e) => Err(e.into()),
        _ => Ok(()),
    }
}

pub async fn start(docker: &Docker, id: &str) -> Result<()> {
    docker.start_container(id, None).await?;
    Ok(())
}

pub async fn remove(docker: &Docker, id: &str) -> Result<()> {
    let options = RemoveContainerOptionsBuilder::new().force(true).build();
    match docker.remove_container(id, Some(options)).await {
        Err(e) if !is_not_found(&e) => Err(e.into()),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- Swarm services

/// A Swarm service justdeploy runs, with how many of its copies are up.
#[derive(Debug, Clone)]
pub struct Workload {
    pub name: String,
    pub labels: HashMap<String, String>,
    pub running: u64,
    pub desired: u64,
}

impl Workload {
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }
}

/// Services carrying every given label (`key=value`), with their task counts.
pub async fn workloads(docker: &Docker, labels: &[(&str, &str)]) -> Result<Vec<Workload>> {
    let mut filter: Vec<String> = vec![format!("{MANAGED}=true")];
    filter.extend(labels.iter().map(|(k, v)| format!("{k}={v}")));
    let filters = HashMap::from([("label", filter)]);
    let options = ListServicesOptionsBuilder::new()
        .filters(&filters)
        .status(true)
        .build();
    Ok(docker
        .list_services(Some(options))
        .await?
        .into_iter()
        .map(|s| {
            let spec = s.spec.unwrap_or_default();
            let status = s.service_status.unwrap_or_default();
            Workload {
                name: spec.name.unwrap_or_default(),
                labels: spec.labels.unwrap_or_default(),
                running: status.running_tasks.unwrap_or(0),
                desired: status.desired_tasks.unwrap_or(0),
            }
        })
        .collect())
}

/// Creates a service, replacing one of the same name left over from a crash.
/// `credentials` reach every server that pulls its image.
pub async fn create_service(
    docker: &Docker,
    spec: ServiceSpec,
    credentials: Option<DockerCredentials>,
) -> Result<String> {
    if let Some(name) = &spec.name {
        remove_service(docker, name).await?;
    }
    let created = docker.create_service(spec, credentials).await?;
    Ok(created.id.unwrap_or_default())
}

pub async fn remove_service(docker: &Docker, name: &str) -> Result<()> {
    match docker.delete_service(name).await {
        Err(e) if !is_not_found(&e) => Err(e.into()),
        _ => Ok(()),
    }
}

/// Tags a local image for the registry and pushes it there.
pub async fn push(
    docker: &Docker,
    local: &str,
    remote: &str,
    credentials: DockerCredentials,
) -> Result<()> {
    let (repo, tag) = remote
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("{remote} has no tag"))?;
    let options = bollard::query_parameters::TagImageOptionsBuilder::new()
        .repo(repo)
        .tag(tag)
        .build();
    docker.tag_image(local, Some(options)).await?;
    let options = bollard::query_parameters::PushImageOptionsBuilder::new()
        .tag(tag)
        .build();
    let mut stream = docker.push_image(repo, Some(options), Some(credentials));
    while let Some(item) = stream.next().await {
        let info = item.map_err(|e| anyhow!("pushing {remote} failed: {e}"))?;
        if let Some(detail) = info.error_detail.and_then(|d| d.message) {
            return Err(anyhow!("pushing {remote} failed: {detail}"));
        }
    }
    Ok(())
}

/// Sets how many copies of a service run.
pub async fn scale(docker: &Docker, name: &str, replicas: i64) -> Result<()> {
    let current = docker.inspect_service(name, None).await?;
    let version = current.version.and_then(|v| v.index).unwrap_or(0);
    let mut spec = current.spec.unwrap_or_default();
    spec.mode = Some(ServiceSpecMode {
        replicated: Some(ServiceSpecModeReplicated {
            replicas: Some(replicas),
        }),
        ..Default::default()
    });
    let options = UpdateServiceOptionsBuilder::new()
        .version(version as i32)
        .build();
    docker.update_service(name, spec, options, None).await?;
    Ok(())
}

/// After scaling a service to nothing, waits (up to half a minute) for its
/// copies to stop, so a volume is never used by two at once.
pub async fn wait_stopped(docker: &Docker, service: &str) {
    for _ in 0..30 {
        match tasks(docker, service).await {
            Ok(tasks) if !tasks.iter().any(task_running) => return,
            Err(_) => return,
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

/// Ids of the servers in the cluster that are up.
pub async fn ready_nodes(docker: &Docker) -> Result<Vec<String>> {
    Ok(docker
        .list_nodes(None::<bollard::query_parameters::ListNodesOptions>)
        .await?
        .into_iter()
        .filter(|n| {
            n.status.as_ref().and_then(|s| s.state) == Some(bollard::models::NodeState::READY)
        })
        .filter_map(|n| n.id)
        .collect())
}

/// Every task of a service, current and past.
pub async fn tasks(docker: &Docker, service: &str) -> Result<Vec<Task>> {
    let filters = HashMap::from([("service", vec![service.to_string()])]);
    let options = ListTasksOptionsBuilder::new().filters(&filters).build();
    Ok(docker.list_tasks(Some(options)).await?)
}

pub fn task_running(task: &Task) -> bool {
    task.status.as_ref().and_then(|s| s.state) == Some(TaskState::RUNNING)
}

/// Waits until `replicas` copies of a service run. A copy that fails or can't
/// be placed ends the wait with Swarm's reason.
pub async fn wait_running(
    docker: &Docker,
    service: &str,
    replicas: i64,
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let tasks = tasks(docker, service).await?;
        if tasks.iter().filter(|t| task_running(t)).count() as i64 >= replicas {
            return Ok(());
        }
        for task in &tasks {
            let status = task.status.clone().unwrap_or_default();
            if matches!(status.state, Some(TaskState::FAILED | TaskState::REJECTED)) {
                let why = status.err.or(status.message).unwrap_or_default();
                return Err(anyhow!("a copy failed to start: {why}"));
            }
        }
        if tokio::time::Instant::now() > deadline {
            let pending = tasks
                .iter()
                .filter_map(|t| t.status.as_ref()?.err.clone())
                .next()
                .unwrap_or_else(|| "no server could take it".into());
            return Err(anyhow!(
                "not running after {} seconds: {pending}",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The last lines a service printed, across its copies.
pub async fn service_tail_logs(docker: &Docker, service: &str, lines: usize) -> Vec<String> {
    let options = LogsOptionsBuilder::new()
        .stdout(true)
        .stderr(true)
        .tail(&lines.to_string())
        .build();
    let mut splitter = LineSplitter::default();
    let mut out = Vec::new();
    let mut stream = docker.service_logs(service, Some(options));
    while let Some(Ok(chunk)) = stream.next().await {
        out.extend(splitter.push(&output_bytes(&chunk)));
    }
    out.extend(splitter.finish());
    out
}

pub fn output_bytes(output: &LogOutput) -> bytes::Bytes {
    match output {
        LogOutput::StdOut { message }
        | LogOutput::StdErr { message }
        | LogOutput::StdIn { message }
        | LogOutput::Console { message } => message.clone(),
    }
}

/// Runs a container to completion, handing each output line to `on_line`, and
/// returns its exit code. The container is removed afterwards.
pub async fn run_to_completion(
    docker: &Docker,
    name: &str,
    body: ContainerCreateBody,
    timeout: Duration,
    mut on_line: impl FnMut(String),
) -> Result<i64> {
    let id = create_and_start(docker, name, body).await?;
    let follow = async {
        let options = LogsOptionsBuilder::new()
            .follow(true)
            .stdout(true)
            .stderr(true)
            .build();
        let mut stream = docker.logs(&id, Some(options));
        let mut splitter = LineSplitter::default();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => splitter
                    .push(&output_bytes(&chunk))
                    .into_iter()
                    .for_each(&mut on_line),
                Err(_) => break,
            }
        }
        splitter.finish().into_iter().for_each(&mut on_line);
    };
    let timed_out = tokio::time::timeout(timeout, follow).await.is_err();
    if timed_out {
        remove(docker, &id).await?;
        return Err(anyhow!(
            "timed out after {} minutes",
            timeout.as_secs() / 60
        ));
    }
    let code = match docker.wait_container(&id, None).next().await {
        Some(Ok(response)) => response.status_code,
        Some(Err(DockerError::DockerContainerWaitError { code, .. })) => code,
        Some(Err(e)) => return Err(e.into()),
        None => 0,
    };
    remove(docker, &id).await?;
    Ok(code)
}

/// Docker hands output over in arbitrary chunks; this turns them into lines.
#[derive(Default)]
pub struct LineSplitter {
    pending: String,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        let mut lines = Vec::new();
        while let Some(index) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=index).collect();
            lines.push(line.trim_end_matches(['\n', '\r']).to_string());
        }
        lines
    }

    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        (!rest.is_empty()).then_some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_across_chunks() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"hel"), Vec::<String>::new());
        assert_eq!(s.push(b"lo\r\nwor"), vec!["hello"]);
        assert_eq!(s.push(b"ld\nnext\n"), vec!["world", "next"]);
        assert_eq!(s.push(b"tail"), Vec::<String>::new());
        assert_eq!(s.finish().as_deref(), Some("tail"));
    }
}
