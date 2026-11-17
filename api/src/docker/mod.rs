//! Thin helpers over bollard. Everything justdeploy creates carries `jd.managed`.

pub mod spec;

use anyhow::{anyhow, Result};
use bollard::{
    errors::Error as DockerError,
    models::{ContainerCreateBody, ContainerSummary, NetworkCreateRequest, VolumeCreateRequest},
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
        LogsOptionsBuilder, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
        StopContainerOptionsBuilder,
    },
    container::LogOutput,
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
    matches!(error, DockerError::DockerResponseServerError { status_code: 404, .. })
}

fn is_conflict(error: &DockerError) -> bool {
    matches!(error, DockerError::DockerResponseServerError { status_code: 409, .. })
}

pub async fn ensure_network(docker: &Docker, name: &str) -> Result<()> {
    match docker.inspect_network(name, None).await {
        Ok(_) => return Ok(()),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e.into()),
    }
    let request = NetworkCreateRequest {
        name: name.to_string(),
        driver: Some("bridge".into()),
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

/// Containers carrying every given label (`key=value`).
pub async fn list(docker: &Docker, labels: &[(&str, &str)]) -> Result<Vec<ContainerSummary>> {
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

pub async fn create_and_start(docker: &Docker, name: &str, body: ContainerCreateBody) -> Result<String> {
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

/// The last lines a container printed, for explaining why it failed.
pub async fn tail_logs(docker: &Docker, id: &str, lines: usize) -> Vec<String> {
    let options = LogsOptionsBuilder::new()
        .stdout(true)
        .stderr(true)
        .tail(&lines.to_string())
        .build();
    let mut splitter = LineSplitter::default();
    let mut out = Vec::new();
    let mut stream = docker.logs(id, Some(options));
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
                Ok(chunk) => splitter.push(&output_bytes(&chunk)).into_iter().for_each(&mut on_line),
                Err(_) => break,
            }
        }
        splitter.finish().into_iter().for_each(&mut on_line);
    };
    let timed_out = tokio::time::timeout(timeout, follow).await.is_err();
    if timed_out {
        remove(docker, &id).await?;
        return Err(anyhow!("timed out after {} minutes", timeout.as_secs() / 60));
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
