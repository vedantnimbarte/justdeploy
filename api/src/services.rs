//! Projects, services and their configuration: env vars, domains, volumes.

use crate::{
    crypto::random_string,
    engines,
    error::{bad_request, Error, Result},
    models::{Deployment, Domain, Project, Service, Volume},
    names,
    queue::{self, Job},
    state::SharedState,
};
use serde::Deserialize;
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

// ---------------------------------------------------------------- settings

pub async fn base_domain(state: &SharedState) -> Result<String> {
    let stored = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'base_domain'")
        .fetch_optional(&state.db)
        .await?;
    Ok(stored.unwrap_or_else(|| state.config.base_domain.clone()))
}

pub async fn set_base_domain(db: &PgPool, domain: &str) -> Result<()> {
    if domain != "localhost" {
        names::validate_hostname(domain)?;
    }
    sqlx::query!(
        "INSERT INTO settings (key, value) VALUES ('base_domain', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        domain
    )
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- projects

pub async fn list_projects(db: &PgPool) -> Result<Vec<Project>> {
    Ok(
        sqlx::query_as!(Project, "SELECT * FROM projects ORDER BY name")
            .fetch_all(db)
            .await?,
    )
}

pub async fn get_project(db: &PgPool, id: Uuid) -> Result<Project> {
    Ok(
        sqlx::query_as!(Project, "SELECT * FROM projects WHERE id = $1", id)
            .fetch_one(db)
            .await?,
    )
}

pub async fn create_project(db: &PgPool, name: &str) -> Result<Project> {
    names::validate(name, "project")?;
    Ok(sqlx::query_as!(
        Project,
        "INSERT INTO projects (id, name) VALUES ($1, $2) RETURNING *",
        Uuid::new_v4(),
        name
    )
    .fetch_one(db)
    .await?)
}

pub async fn delete_project(state: &SharedState, id: Uuid) -> Result<()> {
    get_project(&state.db, id).await?;
    sqlx::query!(
        "UPDATE services SET desired_state = 'deleting' WHERE project_id = $1",
        id
    )
    .execute(&state.db)
    .await?;
    queue::enqueue(&state.db, Job::RemoveProject { project_id: id }).await
}

// ---------------------------------------------------------------- services

pub async fn list_services(db: &PgPool, project_id: Uuid) -> Result<Vec<Service>> {
    Ok(sqlx::query_as!(
        Service,
        "SELECT * FROM services WHERE project_id = $1 ORDER BY created_at",
        project_id
    )
    .fetch_all(db)
    .await?)
}

pub async fn get_service(db: &PgPool, id: Uuid) -> Result<Service> {
    Ok(
        sqlx::query_as!(Service, "SELECT * FROM services WHERE id = $1", id)
            .fetch_one(db)
            .await?,
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct NewService {
    pub name: String,
    pub kind: String,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    #[serde(default)]
    pub root_dir: String,
    pub git_token: Option<String>,
    pub build_type: Option<String>,
    pub dockerfile_path: Option<String>,
    pub image: Option<String>,
    pub engine: Option<String>,
    pub port: Option<i32>,
    #[serde(default)]
    pub public: bool,
    pub health_path: Option<String>,
    pub cpu_millis: Option<i32>,
    pub memory_mb: Option<i32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub volumes: Vec<String>,
}

/// Creates a service and queues its first deployment.
pub async fn create_service(
    state: &SharedState,
    project_id: Uuid,
    new: NewService,
) -> Result<Service> {
    let project = get_project(&state.db, project_id).await?;
    names::validate(&new.name, "service")?;
    if names::generated_host(&new.name, &project.name, "x").len() > 63 + 2 {
        return Err(bad_request(
            "service and project names together are too long for a hostname",
        ));
    }

    let mut env = new.env.clone();
    let mut volumes = new.volumes.clone();
    let mut image = new.image.clone().filter(|i| !i.trim().is_empty());
    let mut port = new.port;
    let mut public = new.public;

    match new.kind.as_str() {
        "app" => {
            let repo = new.repo_url.as_deref().unwrap_or_default();
            if !(repo.starts_with("https://") || repo.starts_with("http://")) {
                return Err(bad_request("repository URL must start with https://"));
            }
        }
        "image" => {
            if image.is_none() {
                return Err(bad_request(
                    "an image is required, for example nginx:alpine",
                ));
            }
        }
        "database" => {
            let engine = new
                .engine
                .as_deref()
                .and_then(engines::get)
                .ok_or_else(|| bad_request("unknown database engine"))?;
            image = image.or_else(|| Some(engine.image.to_string()));
            port = Some(engine.port);
            public = false;
            volumes = vec![engine.data_path.to_string()];
            for (k, v) in engines::initial_env(engine.id) {
                env.entry(k).or_insert(v);
            }
        }
        _ => return Err(bad_request("kind must be app, image or database")),
    }
    if public && port.is_none() {
        return Err(bad_request("a public service needs a port"));
    }
    validate_limits(new.cpu_millis, new.memory_mb)?;
    validate_env(&env)?;

    let mut tx = state.db.begin().await?;
    let service = sqlx::query_as!(
        Service,
        r#"
        INSERT INTO services (id, project_id, name, kind, repo_url, branch, root_dir, git_token_enc,
            build_type, dockerfile_path, image, engine, port, public, health_path, cpu_millis,
            memory_mb, webhook_secret)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
        RETURNING *
        "#,
        Uuid::new_v4(),
        project_id,
        new.name,
        new.kind,
        new.repo_url.as_deref().map(str::trim),
        new.branch
            .clone()
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "main".into()),
        new.root_dir.trim().trim_matches('/'),
        new.git_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| state.cipher.encrypt(t)),
        new.build_type.clone().unwrap_or_else(|| "auto".into()),
        new.dockerfile_path
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "Dockerfile".into()),
        image,
        new.engine,
        port,
        public,
        new.health_path.filter(|p| !p.is_empty()),
        new.cpu_millis,
        new.memory_mb,
        random_string(32),
    )
    .fetch_one(&mut *tx)
    .await?;

    for (key, value) in &env {
        sqlx::query!(
            "INSERT INTO env_vars (service_id, key, value_enc) VALUES ($1, $2, $3)",
            service.id,
            key,
            state.cipher.encrypt(value)
        )
        .execute(&mut *tx)
        .await?;
    }
    for path in &volumes {
        add_volume(&mut tx, service.id, path).await?;
    }
    tx.commit().await?;

    deploy(state, &service, "create").await?;
    Ok(service)
}

async fn add_volume(tx: &mut sqlx::PgConnection, service_id: Uuid, path: &str) -> Result<()> {
    if !path.starts_with('/') {
        return Err(bad_request(
            "volume mount paths must be absolute, like /data",
        ));
    }
    sqlx::query!(
        "INSERT INTO volumes (id, service_id, name, mount_path) VALUES ($1, $2, $3, $4)",
        Uuid::new_v4(),
        service_id,
        names::volume(service_id, path),
        path
    )
    .execute(tx)
    .await?;
    Ok(())
}

fn validate_limits(cpu_millis: Option<i32>, memory_mb: Option<i32>) -> Result<()> {
    if cpu_millis.is_some_and(|c| !(50..=64_000).contains(&c)) {
        return Err(bad_request(
            "CPU limit must be between 50 and 64000 millicores",
        ));
    }
    if memory_mb.is_some_and(|m| !(16..=262_144).contains(&m)) {
        return Err(bad_request("memory limit must be between 16 MB and 256 GB"));
    }
    Ok(())
}

fn validate_env(env: &BTreeMap<String, String>) -> Result<()> {
    for key in env.keys() {
        let ok = !key.is_empty()
            && !key.starts_with(|c: char| c.is_ascii_digit())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return Err(bad_request(format!("{key:?} is not a valid variable name")));
        }
        if key.starts_with("JD_") {
            return Err(bad_request("names starting with JD_ are reserved"));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ServiceUpdate {
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub root_dir: Option<String>,
    /// Some("") clears the token.
    pub git_token: Option<String>,
    pub build_type: Option<String>,
    pub dockerfile_path: Option<String>,
    pub image: Option<String>,
    pub port: Option<Option<i32>>,
    pub public: Option<bool>,
    pub health_path: Option<String>,
    pub cpu_millis: Option<Option<i32>>,
    pub memory_mb: Option<Option<i32>>,
    pub public_port: Option<Option<i32>>,
    pub preview_enabled: Option<bool>,
}

/// Saves settings, then redeploys: a source change rebuilds, anything else
/// re-releases the current image.
pub async fn update_service(
    state: &SharedState,
    id: Uuid,
    update: ServiceUpdate,
) -> Result<Service> {
    let current = get_service(&state.db, id).await?;
    let source_changed = update
        .repo_url
        .as_ref()
        .is_some_and(|v| Some(v) != current.repo_url.as_ref())
        || update
            .branch
            .as_ref()
            .is_some_and(|v| Some(v) != current.branch.as_ref())
        || update
            .root_dir
            .as_ref()
            .is_some_and(|v| v != &current.root_dir)
        || update
            .build_type
            .as_ref()
            .is_some_and(|v| v != &current.build_type)
        || update
            .dockerfile_path
            .as_ref()
            .is_some_and(|v| v != &current.dockerfile_path)
        || update
            .image
            .as_ref()
            .is_some_and(|v| Some(v) != current.image.as_ref())
        || update.git_token.is_some();

    let port = update.port.unwrap_or(current.port);
    let public = update.public.unwrap_or(current.public);
    let cpu = update.cpu_millis.unwrap_or(current.cpu_millis);
    let memory = update.memory_mb.unwrap_or(current.memory_mb);
    let public_port = update.public_port.unwrap_or(current.public_port);
    if public && port.is_none() {
        return Err(bad_request("a public service needs a port"));
    }
    validate_limits(cpu, memory)?;
    if public_port.is_some_and(|p| !(1024..=65535).contains(&p)) {
        return Err(bad_request("public port must be between 1024 and 65535"));
    }
    if current.kind == "database" && public {
        return Err(bad_request(
            "databases are not served over HTTP; use a public port instead",
        ));
    }
    let preview = update.preview_enabled.unwrap_or(current.preview_enabled);
    if preview && current.kind != "app" {
        return Err(bad_request(
            "only services built from Git can have preview deploys",
        ));
    }
    let git_token_enc = match update.git_token.as_deref() {
        None => current.git_token_enc.clone(),
        Some("") => None,
        Some(token) => Some(state.cipher.encrypt(token)),
    };

    let service = sqlx::query_as!(
        Service,
        r#"
        UPDATE services SET
            repo_url = $2, branch = $3, root_dir = $4, git_token_enc = $5, build_type = $6,
            dockerfile_path = $7, image = $8, port = $9, public = $10, health_path = $11,
            cpu_millis = $12, memory_mb = $13, public_port = $14, preview_enabled = $15,
            updated_at = now()
        WHERE id = $1
        RETURNING *
        "#,
        id,
        update.repo_url.or(current.repo_url),
        update.branch.or(current.branch),
        update
            .root_dir
            .map(|r| r.trim().trim_matches('/').to_string())
            .unwrap_or(current.root_dir),
        git_token_enc,
        update.build_type.unwrap_or(current.build_type),
        update.dockerfile_path.unwrap_or(current.dockerfile_path),
        update.image.or(current.image),
        port,
        public,
        match update.health_path {
            Some(p) if p.is_empty() => None,
            Some(p) => Some(p),
            None => current.health_path,
        },
        cpu,
        memory,
        public_port,
        preview,
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => {
            Error::Conflict("that public port is already used by another service".into())
        }
        other => other,
    })?;

    if source_changed {
        deploy(state, &service, "settings").await?;
    } else {
        rerelease(state, &service, "settings").await?;
    }
    Ok(service)
}

pub async fn delete_service(state: &SharedState, id: Uuid) -> Result<()> {
    sqlx::query!(
        "UPDATE services SET desired_state = 'deleting' WHERE id = $1",
        id
    )
    .execute(&state.db)
    .await?;
    queue::enqueue(&state.db, Job::RemoveService { service_id: id }).await
}

// ---------------------------------------------------------------- deployments

pub struct NewDeployment {
    pub kind: &'static str,
    pub trigger: &'static str,
    pub pr_number: Option<i32>,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    /// Set to skip the build and run this image.
    pub image_ref: Option<String>,
}

pub async fn create_deployment(
    state: &SharedState,
    service: &Service,
    new: NewDeployment,
) -> Result<Deployment> {
    if service.desired_state == "deleting" {
        return Err(Error::Conflict("this service is being deleted".into()));
    }
    let deployment = sqlx::query_as!(
        Deployment,
        r#"
        INSERT INTO deployments (id, service_id, kind, pr_number, repo_url, branch, commit_sha,
            commit_message, image_ref, trigger)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING *
        "#,
        Uuid::new_v4(),
        service.id,
        new.kind,
        new.pr_number,
        new.repo_url.or_else(|| service.repo_url.clone()),
        new.branch.or_else(|| service.branch.clone()),
        new.commit_sha,
        new.commit_message,
        new.image_ref,
        new.trigger,
    )
    .fetch_one(&state.db)
    .await?;
    queue::enqueue(
        &state.db,
        Job::Deploy {
            deployment_id: deployment.id,
        },
    )
    .await?;
    Ok(deployment)
}

/// A fresh build from the branch head (apps) or a fresh pull (images, databases).
pub async fn deploy(
    state: &SharedState,
    service: &Service,
    trigger: &'static str,
) -> Result<Deployment> {
    set_desired(&state.db, service.id, "running").await?;
    create_deployment(
        state,
        service,
        NewDeployment {
            kind: "production",
            trigger,
            pr_number: None,
            repo_url: None,
            branch: None,
            commit_sha: None,
            commit_message: None,
            image_ref: None,
        },
    )
    .await
}

/// Runs the current image again with the latest settings, env and domains.
/// With nothing deployed yet it falls back to a full deploy.
pub async fn rerelease(
    state: &SharedState,
    service: &Service,
    trigger: &'static str,
) -> Result<Option<Deployment>> {
    if service.desired_state != "running" {
        return Ok(None);
    }
    // A deploy already on its way reads settings, env and domains when it
    // releases, so it carries this change too. Queuing another release of the
    // current image here would undo it.
    let in_flight = sqlx::query_as!(
        Deployment,
        "SELECT * FROM deployments
         WHERE service_id = $1 AND kind = 'production' AND status IN ('queued', 'building', 'deploying')
         ORDER BY created_at DESC LIMIT 1",
        service.id
    )
    .fetch_optional(&state.db)
    .await?;
    if in_flight.is_some() {
        return Ok(in_flight);
    }
    let current = match service.current_deployment_id {
        Some(id) => get_deployment(&state.db, id).await.ok(),
        None => None,
    };
    let Some(current) = current else {
        return Ok(Some(deploy(state, service, trigger).await?));
    };
    Ok(Some(
        create_deployment(
            state,
            service,
            NewDeployment {
                kind: "production",
                trigger,
                pr_number: None,
                repo_url: current.repo_url,
                branch: current.branch,
                commit_sha: current.commit_sha,
                commit_message: current.commit_message,
                image_ref: current.image_ref,
            },
        )
        .await?,
    ))
}

pub async fn rollback(state: &SharedState, deployment_id: Uuid) -> Result<Deployment> {
    let target = get_deployment(&state.db, deployment_id).await?;
    if target.kind != "production" || target.image_ref.is_none() {
        return Err(bad_request(
            "only a production deployment that built successfully can be rolled back to",
        ));
    }
    let service = get_service(&state.db, target.service_id).await?;
    set_desired(&state.db, service.id, "running").await?;
    create_deployment(
        state,
        &service,
        NewDeployment {
            kind: "production",
            trigger: "rollback",
            pr_number: None,
            repo_url: target.repo_url,
            branch: target.branch,
            commit_sha: target.commit_sha,
            commit_message: target.commit_message,
            image_ref: target.image_ref,
        },
    )
    .await
}

pub async fn get_deployment(db: &PgPool, id: Uuid) -> Result<Deployment> {
    Ok(
        sqlx::query_as!(Deployment, "SELECT * FROM deployments WHERE id = $1", id)
            .fetch_one(db)
            .await?,
    )
}

pub async fn list_deployments(db: &PgPool, service_id: Uuid) -> Result<Vec<Deployment>> {
    Ok(sqlx::query_as!(
        Deployment,
        "SELECT * FROM deployments WHERE service_id = $1 ORDER BY created_at DESC LIMIT 50",
        service_id
    )
    .fetch_all(db)
    .await?)
}

pub async fn set_desired(db: &PgPool, id: Uuid, desired: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE services SET desired_state = $2 WHERE id = $1 AND desired_state <> 'deleting'",
        id,
        desired
    )
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- env vars

pub async fn env(state: &SharedState, service_id: Uuid) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query!(
        "SELECT key, value_enc FROM env_vars WHERE service_id = $1 ORDER BY key",
        service_id
    )
    .fetch_all(&state.db)
    .await?;
    rows.into_iter()
        .map(|r| Ok((r.key, state.cipher.decrypt(&r.value_enc)?)))
        .collect()
}

/// Replaces every variable. Returns whether anything changed.
pub async fn replace_env(
    state: &SharedState,
    service_id: Uuid,
    vars: BTreeMap<String, String>,
) -> Result<bool> {
    validate_env(&vars)?;
    let before: BTreeMap<String, String> = env(state, service_id).await?.into_iter().collect();
    if before == vars {
        return Ok(false);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM env_vars WHERE service_id = $1", service_id)
        .execute(&mut *tx)
        .await?;
    for (key, value) in &vars {
        sqlx::query!(
            "INSERT INTO env_vars (service_id, key, value_enc) VALUES ($1, $2, $3)",
            service_id,
            key,
            state.cipher.encrypt(value)
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// For a database, how sibling services connect to it.
pub async fn connection_url(state: &SharedState, service: &Service) -> Result<Option<String>> {
    let Some(engine) = service.engine.as_deref() else {
        return Ok(None);
    };
    let env: HashMap<String, String> = env(state, service.id).await?.into_iter().collect();
    Ok(engines::connection_url(engine, &service.name, &env))
}

// ---------------------------------------------------------------- domains and volumes

pub async fn domains(db: &PgPool, service_id: Uuid) -> Result<Vec<Domain>> {
    Ok(sqlx::query_as!(
        Domain,
        "SELECT * FROM domains WHERE service_id = $1 ORDER BY created_at",
        service_id
    )
    .fetch_all(db)
    .await?)
}

pub async fn add_domain(db: &PgPool, service: &Service, hostname: &str) -> Result<Domain> {
    let hostname = hostname.trim().to_lowercase();
    names::validate_hostname(&hostname)?;
    if !service.public || service.port.is_none() {
        return Err(bad_request(
            "make the service public with a port before adding a domain",
        ));
    }
    sqlx::query_as!(
        Domain,
        "INSERT INTO domains (id, service_id, hostname) VALUES ($1, $2, $3) RETURNING *",
        Uuid::new_v4(),
        service.id,
        hostname
    )
    .fetch_one(db)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => Error::Conflict(format!("{hostname} is already in use")),
        other => other,
    })
}

pub async fn remove_domain(db: &PgPool, service_id: Uuid, domain_id: Uuid) -> Result<()> {
    let removed = sqlx::query!(
        "DELETE FROM domains WHERE id = $1 AND service_id = $2",
        domain_id,
        service_id
    )
    .execute(db)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Every hostname a deployment answers on: the generated one plus custom domains,
/// or just the preview hostname.
pub async fn hosts(
    state: &SharedState,
    service: &Service,
    project: &Project,
    pr: Option<i32>,
) -> Result<Vec<String>> {
    if !service.public || service.port.is_none() {
        return Ok(vec![]);
    }
    let base = base_domain(state).await?;
    if let Some(pr) = pr {
        return Ok(vec![names::preview_host(
            pr,
            &service.name,
            &project.name,
            &base,
        )]);
    }
    let mut hosts = vec![names::generated_host(&service.name, &project.name, &base)];
    hosts.extend(
        domains(&state.db, service.id)
            .await?
            .into_iter()
            .map(|d| d.hostname),
    );
    Ok(hosts)
}

pub async fn volumes(db: &PgPool, service_id: Uuid) -> Result<Vec<Volume>> {
    Ok(sqlx::query_as!(
        Volume,
        "SELECT * FROM volumes WHERE service_id = $1 ORDER BY mount_path",
        service_id
    )
    .fetch_all(db)
    .await?)
}

pub async fn audit(db: &PgPool, action: &str, target: &str, detail: Option<&str>) {
    let result = sqlx::query!(
        "INSERT INTO audit_log (action, target, detail) VALUES ($1, $2, $3)",
        action,
        target,
        detail
    )
    .execute(db)
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, "audit log write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_are_checked() {
        let ok: BTreeMap<_, _> = [("DATABASE_URL".to_string(), "x".to_string())].into();
        assert!(validate_env(&ok).is_ok());
        for bad in ["1A", "A-B", "", "JD_SECRET", "A B"] {
            let env: BTreeMap<_, _> = [(bad.to_string(), "x".to_string())].into();
            assert!(validate_env(&env).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn limits_are_bounded() {
        assert!(validate_limits(Some(500), Some(512)).is_ok());
        assert!(validate_limits(None, None).is_ok());
        assert!(validate_limits(Some(1), None).is_err());
        assert!(validate_limits(None, Some(1)).is_err());
    }
}
