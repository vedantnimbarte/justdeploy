//! Build, release and removal.
//!
//! Release is zero-downtime when it can be: the new container starts beside the
//! old one under the same Traefik router, and the old one is removed only once
//! the new one answers. A service with a volume or a host port can't run twice,
//! so it is stopped first and started again if the new one fails.
//!
//! A pre-deploy command runs from the new image before any of that, and a
//! scheduled service stops there: its image is ready, and runs start from it.

use crate::{
    backups,
    docker::{self, spec, LABEL_DEPLOYMENT, LABEL_KIND, LABEL_PR, LABEL_PROJECT, LABEL_SERVICE},
    error::{Error, Result},
    github,
    models::{Deployment, Project, Service},
    names, notify, services,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PRE_DEPLOY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const KEEP_IMAGES: i64 = 5;
/// Traefik batches Docker events for about two seconds before it reloads.
const PROXY_SETTLE: Duration = Duration::from_secs(3);

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
    github::report(state, &service, &deployment, "pending", "Deploying".into());
    let outcome = if service.desired_state == "deleting" {
        Err(anyhow!("the service is being deleted"))
    } else {
        pipeline(state, &service, &deployment, &log).await
    };
    if let Err(error) = &outcome {
        log.line(format!("==> Deployment failed: {error:#}"));
    }
    log.finish().await;
    // The build recorded the commit it checked out; statuses and messages use it.
    let deployment = services::get_deployment(&state.db, deployment.id)
        .await
        .unwrap_or(deployment);

    let project = services::get_project(&state.db, service.project_id)
        .await
        .map(|p| p.name)
        .unwrap_or_default();
    let what = match deployment.pr_number {
        Some(pr) => format!("the preview of {} for PR #{pr} in {project}", service.name),
        None => format!("{} in {project}", service.name),
    };
    let path = format!("/services/{}", service.id);
    if let Err(error) = outcome {
        sqlx::query!(
            "UPDATE deployments SET status = 'failed', error = $2, finished_at = now() WHERE id = $1",
            deployment.id,
            format!("{error:#}")
        )
        .execute(&state.db)
        .await?;
        github::report(
            state,
            &service,
            &deployment,
            "failure",
            format!("{error:#}"),
        );
        notify::send(
            state,
            notify::Event {
                kind: "deploy.failed",
                text: format!("Deploying {what} failed: {error:#}"),
                path,
            },
        );
    } else {
        let commit = deployment
            .commit_message
            .as_deref()
            .and_then(|m| m.lines().next())
            .map(|m| format!(" ({m})"))
            .unwrap_or_default();
        github::report(state, &service, &deployment, "success", "Deployed".into());
        notify::send(
            state,
            notify::Event {
                kind: "deploy.succeeded",
                text: format!("Deployed {what}{commit}."),
                path,
            },
        );
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

    // Settings may have changed during a long build; release with the latest.
    let service = services::get_service(&state.db, service.id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    release(state, &service, &project, deployment, &image, log).await
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
    } else if let Some(token) = github::repo_token(state, &repo).await {
        log.line("==> Cloning through the GitHub App");
        env.push(format!("JD_GIT_TOKEN={token}"));
    }
    let vars = services::runtime_env(state, service, deployment.pr_number)
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
    if let Some(pr) = deployment.pr_number {
        preview_databases(state, project, &network, pr, log).await?;
    }
    let env = services::runtime_env(state, service, deployment.pr_number)
        .await
        .map_err(err)?;
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

    if let Some(command) = &service.pre_deploy_command {
        // A preview runs it only against its own database copies; with a
        // fixed database address it would change production's.
        let isolated = deployment.pr_number.is_none()
            || services::references_database(state, service)
                .await
                .map_err(err)?;
        if !isolated {
            log.line(
                "==> Skipping the pre-deploy command: this preview reaches its database by a fixed \
                 address, which is production's. Use a reference like ${{db.URL}} to give previews their own copy",
            );
        } else {
            log.line(format!("==> Running the pre-deploy command: {command}"));
            let body = spec::one_off(image, Some(command), &env, service, &network);
            let name = format!("jd-predeploy-{}", &deployment.id.simple().to_string()[..12]);
            let code = docker::run_to_completion(
                &state.docker,
                &name,
                body,
                PRE_DEPLOY_TIMEOUT,
                log.sink(),
            )
            .await?;
            if code != 0 {
                bail!("the pre-deploy command exited with code {code}, so nothing was released");
            }
        }
    }

    if let Some(schedule) = &service.cron_schedule {
        // Nothing stays up: runs start from this image on the schedule. A
        // container left from before the service was scheduled goes.
        for id in &old {
            docker::remove(&state.docker, id).await?;
        }
        mark_released(state, service, deployment, image).await?;
        log.line(format!(
            "==> Ready. Runs on the schedule `{schedule}` (UTC)"
        ));
        return Ok(());
    }

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

    // Hand traffic over before removing anything: give Traefik time to add the
    // new container, then stop the old one gracefully. Once it stops listening,
    // Traefik's retry middleware sends refused requests to the new container.
    if spec.routed() && !exclusive && !old.is_empty() {
        tokio::time::sleep(PROXY_SETTLE).await;
    }
    for id in &old {
        docker::stop(&state.docker, id).await?;
    }
    if spec.routed() && !exclusive && !old.is_empty() {
        tokio::time::sleep(PROXY_SETTLE).await;
    }
    for id in &old {
        docker::remove(&state.docker, id).await?;
    }

    mark_released(state, service, deployment, image).await?;

    let scheme = if state.config.tls() { "https" } else { "http" };
    for host in &hosts {
        log.line(format!("==> Live at {scheme}://{host}"));
    }
    if hosts.is_empty() {
        log.line("==> Running");
    }
    Ok(())
}

/// Records `deployment` as the live one and supersedes the one before.
async fn mark_released(
    state: &SharedState,
    service: &Service,
    deployment: &Deployment,
    image: &str,
) -> anyhow::Result<()> {
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

    let path = service.health_path.clone().unwrap_or_default();
    log.line(match path.as_str() {
        "" => format!("==> Waiting for port {port}"),
        p => format!("==> Waiting for GET {p} on port {port}"),
    });
    let check = format!("jd-check-{}", &deployment.id.simple().to_string()[..12]);
    if !probe(state, network, container, port, &path, &check, log).await? {
        bail!("the service did not become healthy; is it listening on 0.0.0.0:{port}?");
    }
    Ok(())
}

/// Whether `host:port` on `network` accepts connections (and answers 2xx/3xx
/// on `path`, when given) within two minutes.
async fn probe(
    state: &SharedState,
    network: &str,
    host: &str,
    port: i32,
    path: &str,
    helper_name: &str,
    log: &DeployLog,
) -> anyhow::Result<bool> {
    ensure_builder(state, log).await?;
    let env = vec![
        format!("JD_HOST={host}"),
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
    let code = docker::run_to_completion(
        &state.docker,
        helper_name,
        body,
        Duration::from_secs(180),
        log.sink(),
    )
    .await?;
    Ok(code == 0)
}

/// Gives a pull request its own copy of each database in the project, the
/// first time it deploys: same image and credentials, fresh volumes, filled
/// from the database's newest backup. Previews reach them through references.
async fn preview_databases(
    state: &SharedState,
    project: &Project,
    network: &str,
    pr: i32,
    log: &DeployLog,
) -> anyhow::Result<()> {
    let err = |e: Error| anyhow!("{e}");
    let databases = services::list_services(&state.db, project.id)
        .await
        .map_err(err)?
        .into_iter()
        .filter(services::is_copied_for_previews);
    for db in databases {
        let alias = names::preview_database(&db.name, pr);
        let (id, pr_label) = (db.id.to_string(), pr.to_string());
        let labels = [(LABEL_SERVICE, id.as_str()), (LABEL_PR, pr_label.as_str())];
        if let Some(existing) = docker::list(&state.docker, &labels).await?.first() {
            if !docker::is_running(existing) {
                if let Some(cid) = &existing.id {
                    docker::start(&state.docker, cid).await?;
                }
            }
            continue;
        }
        let image = db
            .image
            .clone()
            .ok_or_else(|| anyhow!("{} has no image", db.name))?;
        log.line(format!(
            "==> Creating {alias}, this pull request's own copy of {}",
            db.name
        ));
        let mut mounts = Vec::new();
        for volume in services::volumes(&state.db, db.id).await.map_err(err)? {
            let name = format!("{}-pr{pr}", volume.name);
            docker::ensure_volume(&state.docker, &name).await?;
            mounts.push((name, volume.mount_path));
        }
        let env = services::runtime_env(state, &db, None).await.map_err(err)?;
        let name = format!("jd-{}-{alias}", project.name);
        let body = spec::preview_database(&db, &image, &env, &mounts, network, &alias, pr);
        let container = docker::create_and_start(&state.docker, &name, body).await?;
        if let Some(port) = db.port {
            let check = format!("jd-check-{alias}-{}", &project.id.simple().to_string()[..8]);
            if !probe(state, network, &name, port, "", &check, log).await? {
                bail!("{alias} did not start");
            }
        }
        if db.engine.as_deref() == Some("redis") {
            log.line(format!(
                "{alias} starts empty: Redis copies aren't filled from backups"
            ));
            continue;
        }
        match backups::restore_latest(state, &db, &container).await {
            Ok(true) => log.line(format!("Filled {alias} from {}'s latest backup", db.name)),
            Ok(false) => log.line(format!(
                "{} has no backup yet, so {alias} starts empty",
                db.name
            )),
            Err(e) => log.line(format!(
                "Restoring {}'s latest backup failed ({e:#}); {alias} starts empty",
                db.name
            )),
        }
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
    services::audit(&state.db, None, "finished deleting", &service.name, None).await;
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
    services::audit(&state.db, None, "finished deleting", &project.name, None).await;
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
    // The pull request's database copies, with their volumes.
    if let Ok(service) = services::get_service(&state.db, service_id).await {
        let project = service.project_id.to_string();
        let labels = [
            (LABEL_PROJECT, project.as_str()),
            (LABEL_PR, pr_label.as_str()),
            (LABEL_KIND, spec::PREVIEW_DATABASE),
        ];
        for container in docker::list(&state.docker, &labels).await? {
            if let Some(cid) = container.id {
                docker::remove(&state.docker, &cid).await?;
            }
        }
        for db in services::list_services(&state.db, service.project_id).await? {
            if db.kind == "database" {
                for volume in services::volumes(&state.db, db.id).await? {
                    docker::remove_volume(&state.docker, &format!("{}-pr{pr}", volume.name))
                        .await?;
                }
            }
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
