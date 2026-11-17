//! Build, release and removal.
//!
//! Release is zero-downtime when it can be: the new container starts beside the
//! old one under the same Traefik router, and the old one is removed only once
//! the new one answers. A service with a volume or a host port can't run twice,
//! so it is stopped first and started again if the new one fails.

use crate::{
    docker::{self, spec, LABEL_DEPLOYMENT, LABEL_KIND, LABEL_PR, LABEL_SERVICE},
    error::{Error, Result},
    models::{Deployment, Project, Service},
    names, services,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const KEEP_IMAGES: i64 = 5;

/// Deployment log writer. Lines are queued synchronously (so they can come from
/// a stream callback) and written in order by one task.
struct DeployLog {
    tx: mpsc::UnboundedSender<String>,
    task: tokio::task::JoinHandle<()>,
}

impl DeployLog {
    fn start(db: PgPool, deployment_id: Uuid) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let task = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                // build.sh reports the commit it checked out on a marker line.
                if let Some(rest) = line.strip_prefix("::jd-commit:: ") {
                    let (sha, message) = rest.split_once(' ').unwrap_or((rest, ""));
                    let _ = sqlx::query!(
                        "UPDATE deployments SET commit_sha = $2, commit_message = $3 WHERE id = $1",
                        deployment_id,
                        sha,
                        message
                    )
                    .execute(&db)
                    .await;
                    continue;
                }
                let _ = sqlx::query!(
                    "INSERT INTO deployment_logs (deployment_id, line) VALUES ($1, $2)",
                    deployment_id,
                    line
                )
                .execute(&db)
                .await;
            }
        });
        Self { tx, task }
    }

    fn line(&self, line: impl Into<String>) {
        let _ = self.tx.send(line.into());
    }

    fn sink(&self) -> impl FnMut(String) + '_ {
        move |line| self.line(line)
    }

    async fn finish(self) {
        drop(self.tx);
        let _ = self.task.await;
    }
}

pub async fn run(state: &SharedState, deployment_id: Uuid) -> Result<()> {
    let deployment = match services::get_deployment(&state.db, deployment_id).await {
        Ok(d) => d,
        Err(Error::NotFound) => return Ok(()),
        Err(e) => return Err(e),
    };
    if !matches!(
        deployment.status.as_str(),
        "queued" | "building" | "deploying"
    ) {
        return Ok(());
    }
    let service = match services::get_service(&state.db, deployment.service_id).await {
        Ok(s) => s,
        Err(Error::NotFound) => return Ok(()),
        Err(e) => return Err(e),
    };

    let log = DeployLog::start(state.db.clone(), deployment.id);
    let outcome = if service.desired_state == "deleting" {
        Err(anyhow!("the service is being deleted"))
    } else {
        pipeline(state, &service, &deployment, &log).await
    };
    if let Err(error) = &outcome {
        log.line(format!("==> Deployment failed: {error:#}"));
    }
    log.finish().await;

    if let Err(error) = outcome {
        sqlx::query!(
            "UPDATE deployments SET status = 'failed', error = $2, finished_at = now() WHERE id = $1",
            deployment.id,
            format!("{error:#}")
        )
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

async fn set_status(db: &PgPool, id: Uuid, status: &str) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE deployments SET status = $2, started_at = COALESCE(started_at, now()) WHERE id = $1",
        id,
        status
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn pipeline(
    state: &SharedState,
    service: &Service,
    deployment: &Deployment,
    log: &DeployLog,
) -> anyhow::Result<()> {
    let project = services::get_project(&state.db, service.project_id).await?;
    set_status(&state.db, deployment.id, "building").await?;

    let image = match (&deployment.image_ref, service.kind.as_str()) {
        (Some(image), _) if docker::image_exists(&state.docker, image).await => {
            log.line(format!("==> Using existing image {image}"));
            image.clone()
        }
        (Some(image), _) if names::is_built_image(image) => {
            log.line("==> That image was pruned; rebuilding the same commit");
            build(state, service, deployment, log).await?
        }
        (Some(image), _) => {
            pull(state, image, log).await?;
            image.clone()
        }
        (None, "app") => build(state, service, deployment, log).await?,
        (None, _) => {
            let image = service
                .image
                .clone()
                .ok_or_else(|| anyhow!("no image is set"))?;
            pull(state, &image, log).await?;
            image
        }
    };
    sqlx::query!(
        "UPDATE deployments SET image_ref = $2 WHERE id = $1",
        deployment.id,
        image
    )
    .execute(&state.db)
    .await?;

    release(state, service, &project, deployment, &image, log).await
}

async fn pull(state: &SharedState, image: &str, log: &DeployLog) -> anyhow::Result<()> {
    log.line(format!("==> Pulling {image}"));
    if let Err(error) = docker::pull_image(&state.docker, image, log.sink()).await {
        // Offline, or a registry hiccup: fine if an older copy is already here.
        if docker::image_exists(&state.docker, image).await {
            log.line(format!("Pull failed ({error}); using the local copy"));
        } else {
            return Err(error);
        }
    }
    Ok(())
}

/// The builder image is pulled on first use; a `:dev` tag must be built locally.
async fn ensure_builder(state: &SharedState, log: &DeployLog) -> anyhow::Result<()> {
    let image = &state.config.builder_image;
    if docker::image_exists(&state.docker, image).await {
        return Ok(());
    }
    if image.ends_with(":dev") {
        bail!("builder image {image} is missing; run `docker build -t {image} builder`");
    }
    log.line(format!("==> Pulling the builder image {image}"));
    docker::pull_image(&state.docker, image, |_| {}).await
}

async fn build(
    state: &SharedState,
    service: &Service,
    deployment: &Deployment,
    log: &DeployLog,
) -> anyhow::Result<String> {
    ensure_builder(state, log).await?;
    let tag = names::image(service.id, deployment.id);
    let repo = deployment
        .repo_url
        .clone()
        .or_else(|| service.repo_url.clone())
        .ok_or_else(|| anyhow!("no repository is set"))?;

    let mut env = vec![
        format!("JD_REPO_URL={repo}"),
        format!(
            "JD_BRANCH={}",
            deployment
                .branch
                .as_deref()
                .or(service.branch.as_deref())
                .unwrap_or("main")
        ),
        format!("JD_ROOT_DIR={}", service.root_dir),
        format!("JD_BUILD_TYPE={}", service.build_type),
        format!("JD_DOCKERFILE={}", service.dockerfile_path),
        format!("JD_IMAGE={tag}"),
    ];
    if let Some(sha) = &deployment.commit_sha {
        env.push(format!("JD_COMMIT={sha}"));
    }
    if let Some(token) = &service.git_token_enc {
        env.push(format!("JD_GIT_TOKEN={}", state.cipher.decrypt(token)?));
    }
    let vars = services::env(state, service.id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    env.push(format!(
        "JD_BUILD_ENV_KEYS={}",
        vars.iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    env.extend(vars.iter().map(|(k, v)| format!("{k}={v}")));

    let body = spec::helper(&state.config.builder_image, &["build.sh"], env, None, true);
    let name = format!("jd-build-{}", &deployment.id.simple().to_string()[..12]);
    let code =
        docker::run_to_completion(&state.docker, &name, body, BUILD_TIMEOUT, log.sink()).await?;
    if code != 0 {
        bail!("the build exited with code {code}");
    }
    Ok(tag)
}

async fn release(
    state: &SharedState,
    service: &Service,
    project: &Project,
    deployment: &Deployment,
    image: &str,
    log: &DeployLog,
) -> anyhow::Result<()> {
    set_status(&state.db, deployment.id, "deploying").await?;
    let err = |e: Error| anyhow!("{e}");

    let network = names::project_network(project.id);
    docker::ensure_network(&state.docker, &network).await?;
    docker::ensure_network(&state.docker, &state.config.proxy_network).await?;
    let volumes = services::volumes(&state.db, service.id)
        .await
        .map_err(err)?;
    for volume in &volumes {
        docker::ensure_volume(&state.docker, &volume.name).await?;
    }
    let env = services::env(state, service.id).await.map_err(err)?;
    let hosts = services::hosts(state, service, project, deployment.pr_number)
        .await
        .map_err(err)?;

    let spec = spec::Release {
        project_name: &project.name,
        service,
        deployment,
        image,
        env: &env,
        hosts: &hosts,
        volumes: &volumes,
        project_network: &network,
        proxy_network: &state.config.proxy_network,
        tls: state.config.tls(),
    };
    let name = spec.container_name();

    let service_id = service.id.to_string();
    let pr = deployment.pr_number.map(|p| p.to_string());
    let mut labels = vec![
        (LABEL_SERVICE, service_id.as_str()),
        (LABEL_KIND, deployment.kind.as_str()),
    ];
    if let Some(pr) = &pr {
        labels.push((LABEL_PR, pr.as_str()));
    }
    let deployment_id = deployment.id.to_string();
    let old: Vec<String> = docker::list(&state.docker, &labels)
        .await?
        .into_iter()
        .filter(|c| docker::label(c, LABEL_DEPLOYMENT) != Some(deployment_id.as_str()))
        .filter(|c| pr.is_some() || docker::label(c, LABEL_PR).is_none())
        .filter_map(|c| c.id)
        .collect();

    let exclusive = !volumes.is_empty() || service.public_port.is_some();
    if exclusive && !old.is_empty() {
        log.line("==> Stopping the previous container first (it holds a volume or host port)");
        for id in &old {
            docker::stop(&state.docker, id).await?;
        }
    }

    log.line(format!("==> Starting {name}"));
    let started = docker::create_and_start(&state.docker, &name, spec.body()).await;
    let healthy = match started {
        Ok(id) => match wait_healthy(state, service, &network, &name, deployment, log).await {
            Ok(()) => Ok(()),
            Err(error) => {
                log.line("==> Last output from the container:");
                for line in docker::tail_logs(&state.docker, &id, 30).await {
                    log.line(format!("    {line}"));
                }
                docker::remove(&state.docker, &id).await?;
                Err(error)
            }
        },
        Err(error) => Err(error),
    };
    if let Err(error) = healthy {
        if exclusive {
            for id in &old {
                let _ = docker::start(&state.docker, id).await;
            }
            if !old.is_empty() {
                log.line("==> Restarted the previous container");
            }
        } else if !old.is_empty() {
            log.line("==> The previous container keeps serving");
        }
        return Err(error);
    }

    for id in &old {
        docker::remove(&state.docker, id).await?;
    }

    sqlx::query!(
        "UPDATE deployments SET status = 'running', finished_at = now() WHERE id = $1",
        deployment.id
    )
    .execute(&state.db)
    .await?;
    sqlx::query!(
        "UPDATE deployments SET status = 'superseded'
         WHERE service_id = $1 AND kind = $2 AND pr_number IS NOT DISTINCT FROM $3
           AND status = 'running' AND id <> $4",
        service.id,
        deployment.kind,
        deployment.pr_number,
        deployment.id
    )
    .execute(&state.db)
    .await?;
    if deployment.pr_number.is_none() {
        sqlx::query!(
            "UPDATE services SET current_deployment_id = $2 WHERE id = $1",
            service.id,
            deployment.id
        )
        .execute(&state.db)
        .await?;
        prune_images(state, service.id, image).await?;
    }

    let scheme = if state.config.tls() { "https" } else { "http" };
    for host in &hosts {
        log.line(format!("==> Live at {scheme}://{host}"));
    }
    if hosts.is_empty() {
        log.line("==> Running");
    }
    Ok(())
}

async fn wait_healthy(
    state: &SharedState,
    service: &Service,
    network: &str,
    container: &str,
    deployment: &Deployment,
    log: &DeployLog,
) -> anyhow::Result<()> {
    let Some(port) = service.port else {
        // A worker has nothing to probe; make sure it doesn't exit straight away.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let inspect = state.docker.inspect_container(container, None).await?;
        let state = inspect.state.unwrap_or_default();
        if state.running == Some(true) && state.restarting != Some(true) {
            return Ok(());
        }
        bail!(
            "the container exited (code {})",
            state.exit_code.unwrap_or_default()
        );
    };

    ensure_builder(state, log).await?;
    let path = service.health_path.clone().unwrap_or_default();
    log.line(match path.as_str() {
        "" => format!("==> Waiting for port {port}"),
        p => format!("==> Waiting for GET {p} on port {port}"),
    });
    let env = vec![
        format!("JD_HOST={container}"),
        format!("JD_PORT={port}"),
        format!("JD_PATH={path}"),
        "JD_TIMEOUT=120".to_string(),
    ];
    let body = spec::helper(
        &state.config.builder_image,
        &["healthcheck.sh"],
        env,
        Some(network),
        false,
    );
    let name = format!("jd-check-{}", &deployment.id.simple().to_string()[..12]);
    let code = docker::run_to_completion(
        &state.docker,
        &name,
        body,
        Duration::from_secs(180),
        log.sink(),
    )
    .await?;
    if code != 0 {
        bail!("the service did not become healthy; is it listening on 0.0.0.0:{port}?");
    }
    Ok(())
}

/// Keeps the newest built images for rollback and removes the rest.
async fn prune_images(state: &SharedState, service_id: Uuid, current: &str) -> anyhow::Result<()> {
    let stale = sqlx::query_scalar!(
        r#"
        SELECT image_ref AS "image!" FROM deployments
        WHERE service_id = $1 AND kind = 'production' AND image_ref LIKE 'jd-%'
        GROUP BY image_ref
        ORDER BY max(created_at) DESC
        OFFSET $2
        "#,
        service_id,
        KEEP_IMAGES
    )
    .fetch_all(&state.db)
    .await?;
    for image in stale.iter().filter(|i| *i != current) {
        docker::remove_image(&state.docker, image).await;
    }
    Ok(())
}

async fn built_images(db: &PgPool, service_id: Uuid, pr: Option<i32>) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar!(
        r#"SELECT DISTINCT image_ref AS "image!" FROM deployments
           WHERE service_id = $1 AND image_ref LIKE 'jd-%'
             AND ($2::int IS NULL OR pr_number = $2)"#,
        service_id,
        pr
    )
    .fetch_all(db)
    .await?)
}

/// Removes containers, built images and volumes, then the service itself.
pub async fn remove_service(state: &SharedState, service_id: Uuid) -> Result<()> {
    let service = match services::get_service(&state.db, service_id).await {
        Ok(s) => s,
        Err(Error::NotFound) => return Ok(()),
        Err(e) => return Err(e),
    };
    let id = service.id.to_string();
    for container in docker::list(&state.docker, &[(LABEL_SERVICE, &id)]).await? {
        if let Some(cid) = container.id {
            docker::remove(&state.docker, &cid).await?;
        }
    }
    for image in built_images(&state.db, service.id, None).await? {
        docker::remove_image(&state.docker, &image).await;
    }
    for volume in services::volumes(&state.db, service.id).await? {
        docker::remove_volume(&state.docker, &volume.name).await?;
    }
    sqlx::query!("DELETE FROM services WHERE id = $1", service.id)
        .execute(&state.db)
        .await?;
    services::audit(&state.db, "service.deleted", &service.name, None).await;
    Ok(())
}

pub async fn remove_project(state: &SharedState, project_id: Uuid) -> Result<()> {
    let project = match services::get_project(&state.db, project_id).await {
        Ok(p) => p,
        Err(Error::NotFound) => return Ok(()),
        Err(e) => return Err(e),
    };
    for service in services::list_services(&state.db, project.id).await? {
        remove_service(state, service.id).await?;
    }
    docker::remove_network(&state.docker, &names::project_network(project.id)).await?;
    sqlx::query!("DELETE FROM projects WHERE id = $1", project.id)
        .execute(&state.db)
        .await?;
    services::audit(&state.db, "project.deleted", &project.name, None).await;
    Ok(())
}

pub async fn remove_preview(state: &SharedState, service_id: Uuid, pr: i32) -> Result<()> {
    let id = service_id.to_string();
    let pr_label = pr.to_string();
    let labels = [(LABEL_SERVICE, id.as_str()), (LABEL_PR, pr_label.as_str())];
    for container in docker::list(&state.docker, &labels).await? {
        if let Some(cid) = container.id {
            docker::remove(&state.docker, &cid).await?;
        }
    }
    for image in built_images(&state.db, service_id, Some(pr)).await? {
        docker::remove_image(&state.docker, &image).await;
    }
    sqlx::query!(
        "UPDATE deployments SET status = 'removed', finished_at = COALESCE(finished_at, now())
         WHERE service_id = $1 AND pr_number = $2 AND status IN ('running', 'queued')",
        service_id,
        pr
    )
    .execute(&state.db)
    .await?;
    Ok(())
}
