//! Projects, services and their configuration: env vars, domains, volumes.

use crate::{
    backups,
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

/// Deleting a base project deletes its environments too.
pub async fn delete_project(state: &SharedState, id: Uuid) -> Result<()> {
    let project = get_project(&state.db, id).await?;
    let mut doomed = vec![project.id];
    if project.base_id.is_none() {
        doomed.extend(
            environments(&state.db, &project)
                .await?
                .into_iter()
                .filter(|p| p.id != project.id)
                .map(|p| p.id),
        );
    }
    for id in doomed {
        sqlx::query!(
            "UPDATE services SET desired_state = 'deleting' WHERE project_id = $1",
            id
        )
        .execute(&state.db)
        .await?;
        queue::enqueue(&state.db, Job::RemoveProject { project_id: id }).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- environments

/// The base project and every environment made from it, production first.
pub async fn environments(db: &PgPool, project: &Project) -> Result<Vec<Project>> {
    let root = project.base_id.unwrap_or(project.id);
    Ok(sqlx::query_as!(
        Project,
        "SELECT * FROM projects WHERE id = $1 OR base_id = $1
         ORDER BY base_id IS NOT NULL, environment",
        root
    )
    .fetch_all(db)
    .await?)
}

/// A new environment copied from `from`: its services with their settings and
/// variables, the shared variables, and fresh empty volumes. Not copied: data,
/// domains, host ports, previews and backup schedules. App services can track
/// another branch. Everything then deploys.
pub async fn create_environment(
    state: &SharedState,
    from: &Project,
    environment: &str,
    branch: Option<&str>,
) -> Result<Project> {
    names::validate(environment, "environment")?;
    let root = get_project(&state.db, from.base_id.unwrap_or(from.id)).await?;
    if environment == root.environment {
        return Err(Error::Conflict(format!("{environment} already exists")));
    }
    let name = format!("{}-{environment}", root.name);
    names::validate(&name, "project")?;

    let mut tx = state.db.begin().await?;
    let project = sqlx::query_as!(
        Project,
        "INSERT INTO projects (id, name, environment, base_id) VALUES ($1, $2, $3, $4) RETURNING *",
        Uuid::new_v4(),
        name,
        environment,
        root.id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => Error::Conflict(format!("{environment} already exists")),
        other => other,
    })?;
    sqlx::query!(
        "INSERT INTO shared_env_vars (project_id, key, value_enc)
         SELECT $2, key, value_enc FROM shared_env_vars WHERE project_id = $1",
        from.id,
        project.id
    )
    .execute(&mut *tx)
    .await?;
    let mut created = Vec::new();
    for source in list_services(&state.db, from.id).await? {
        created.push(clone_service(&mut tx, &source, &project, branch).await?);
    }
    tx.commit().await?;
    for service in &created {
        deploy(state, service, "create").await?;
    }
    Ok(project)
}

/// Copies one service into another environment, with its variables and new
/// empty volumes.
async fn clone_service(
    tx: &mut sqlx::PgConnection,
    source: &Service,
    project: &Project,
    branch: Option<&str>,
) -> Result<Service> {
    if names::generated_host(&source.name, &project.name, "x").len() > 63 + 2 {
        return Err(bad_request(format!(
            "{} and {} together are too long for a hostname; use a shorter environment name",
            source.name, project.name
        )));
    }
    let branch = match (source.kind.as_str(), branch.map(str::trim)) {
        ("app", Some(b)) if !b.is_empty() => Some(b.to_string()),
        _ => source.branch.clone(),
    };
    let service = sqlx::query_as!(
        Service,
        r#"
        INSERT INTO services (id, project_id, name, kind, repo_url, branch, root_dir, git_token_enc,
            build_type, dockerfile_path, image, engine, port, public, health_path, cpu_millis,
            memory_mb, webhook_secret, pre_deploy_command, cron_schedule, replicas, autoscale_min,
            autoscale_max, autoscale_cpu, node_label)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
            $19, $20, $21, $22, $23, $24, $25)
        RETURNING *
        "#,
        Uuid::new_v4(),
        project.id,
        source.name,
        source.kind,
        source.repo_url,
        branch,
        source.root_dir,
        source.git_token_enc,
        source.build_type,
        source.dockerfile_path,
        source.image,
        source.engine,
        source.port,
        source.public,
        source.health_path,
        source.cpu_millis,
        source.memory_mb,
        random_string(32),
        source.pre_deploy_command,
        source.cron_schedule,
        source.replicas,
        source.autoscale_min,
        source.autoscale_max,
        source.autoscale_cpu,
        source.node_label,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO env_vars (service_id, key, value_enc)
         SELECT $2, key, value_enc FROM env_vars WHERE service_id = $1",
        source.id,
        service.id
    )
    .execute(&mut *tx)
    .await?;
    let paths = sqlx::query_scalar!(
        "SELECT mount_path FROM volumes WHERE service_id = $1",
        source.id
    )
    .fetch_all(&mut *tx)
    .await?;
    for path in paths {
        add_volume(tx, service.id, &path).await?;
    }
    Ok(service)
}

#[derive(serde::Serialize, Default)]
pub struct SyncReport {
    /// Services whose settings or variables changed.
    updated: Vec<String>,
    /// Services that only existed in the source and were copied over.
    added: Vec<String>,
}

/// Brings `target` in line with `source`: settings copied onto services of the
/// same name, missing services added, missing variables added. Kept as they
/// are: the target's branches, variable values, domains, host ports, previews
/// and any service only it has. Whatever changed redeploys.
pub async fn sync_environment(
    state: &SharedState,
    target: &Project,
    source: &Project,
) -> Result<SyncReport> {
    if target.id == source.id
        || target.base_id.unwrap_or(target.id) != source.base_id.unwrap_or(source.id)
    {
        return Err(bad_request(
            "pick another environment of the same project to copy from",
        ));
    }
    let existing: HashMap<String, Service> = list_services(&state.db, target.id)
        .await?
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    let mut report = SyncReport::default();
    let mut rebuild = Vec::new();
    let mut rerun = Vec::new();
    let mut fresh = Vec::new();
    let mut tx = state.db.begin().await?;
    for s in list_services(&state.db, source.id).await? {
        let Some(t) = existing.get(&s.name) else {
            fresh.push(clone_service(&mut tx, &s, target, None).await?);
            report.added.push(s.name.clone());
            continue;
        };
        let source_changed = t.repo_url != s.repo_url
            || t.root_dir != s.root_dir
            || t.build_type != s.build_type
            || t.dockerfile_path != s.dockerfile_path
            || t.image != s.image
            || t.git_token_enc != s.git_token_enc;
        let changed = source_changed
            || t.port != s.port
            || t.public != s.public
            || t.health_path != s.health_path
            || t.cpu_millis != s.cpu_millis
            || t.memory_mb != s.memory_mb
            || t.pre_deploy_command != s.pre_deploy_command
            || t.cron_schedule != s.cron_schedule;
        if changed {
            sqlx::query!(
                "UPDATE services SET repo_url = $2, root_dir = $3, build_type = $4,
                     dockerfile_path = $5, image = $6, git_token_enc = $7, port = $8, public = $9,
                     health_path = $10, cpu_millis = $11, memory_mb = $12,
                     pre_deploy_command = $13, cron_schedule = $14, updated_at = now()
                 WHERE id = $1",
                t.id,
                s.repo_url,
                s.root_dir,
                s.build_type,
                s.dockerfile_path,
                s.image,
                s.git_token_enc,
                s.port,
                s.public,
                s.health_path,
                s.cpu_millis,
                s.memory_mb,
                s.pre_deploy_command,
                s.cron_schedule
            )
            .execute(&mut *tx)
            .await?;
        }
        let added = sqlx::query!(
            "INSERT INTO env_vars (service_id, key, value_enc)
             SELECT $2, key, value_enc FROM env_vars WHERE service_id = $1
             ON CONFLICT DO NOTHING",
            s.id,
            t.id
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed || added > 0 {
            report.updated.push(s.name.clone());
            if source_changed {
                rebuild.push(t.id);
            } else {
                rerun.push(t.id);
            }
        }
    }
    let shared_added = sqlx::query!(
        "INSERT INTO shared_env_vars (project_id, key, value_enc)
         SELECT $2, key, value_enc FROM shared_env_vars WHERE project_id = $1
         ON CONFLICT DO NOTHING",
        source.id,
        target.id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;

    if shared_added > 0 {
        for t in existing.values() {
            if !rebuild.contains(&t.id) && !rerun.contains(&t.id) {
                rerun.push(t.id);
            }
        }
    }
    for service in &fresh {
        deploy(state, service, "settings").await?;
    }
    for id in rebuild {
        deploy(state, &get_service(&state.db, id).await?, "settings").await?;
    }
    for id in rerun {
        rerelease(state, &get_service(&state.db, id).await?, "settings").await?;
    }
    Ok(report)
}

#[derive(serde::Serialize)]
pub struct Edge {
    from: Uuid,
    to: Uuid,
    /// The variables that make the reference; names only.
    keys: Vec<String>,
}

/// Which services point at which, through `${{service.KEY}}` references.
pub async fn project_graph(state: &SharedState, project_id: Uuid) -> Result<Vec<Edge>> {
    let services = list_services(&state.db, project_id).await?;
    let ids: HashMap<&str, Uuid> = services.iter().map(|s| (s.name.as_str(), s.id)).collect();
    let shared: BTreeMap<String, String> =
        shared_env(state, project_id).await?.into_iter().collect();
    let mut edges: BTreeMap<(Uuid, Uuid), Vec<String>> = BTreeMap::new();
    for service in &services {
        let mut vars = shared.clone();
        vars.extend(env(state, service.id).await?);
        for (key, value) in &vars {
            for (target, _) in references(value) {
                if let Some(&to) = ids.get(target.as_str()).filter(|to| **to != service.id) {
                    let keys = edges.entry((service.id, to)).or_default();
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
            }
        }
    }
    Ok(edges
        .into_iter()
        .map(|((from, to), keys)| Edge { from, to, keys })
        .collect())
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
    pub pre_deploy_command: Option<String>,
    pub cron_schedule: Option<String>,
}

/// Empty and blank strings mean "not set".
fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A schedule makes a service a cron job: nothing stays up, so nothing can be routed to it.
fn validate_schedule(
    kind: &str,
    schedule: Option<&str>,
    public: bool,
    public_port: Option<i32>,
    preview: bool,
) -> Result<()> {
    let Some(expr) = schedule else {
        return Ok(());
    };
    if kind == "database" {
        return Err(bad_request("databases can't run on a schedule"));
    }
    if public || public_port.is_some() {
        return Err(bad_request(
            "a scheduled service takes no traffic; turn off Public and the host port",
        ));
    }
    if preview {
        return Err(bad_request(
            "turn off preview deploys before putting the service on a schedule",
        ));
    }
    backups::parse_cron(expr).map(|_| ())
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
    let cron_schedule = optional_text(new.cron_schedule.clone());
    validate_schedule(&new.kind, cron_schedule.as_deref(), public, None, false)?;

    let mut tx = state.db.begin().await?;
    let service = sqlx::query_as!(
        Service,
        r#"
        INSERT INTO services (id, project_id, name, kind, repo_url, branch, root_dir, git_token_enc,
            build_type, dockerfile_path, image, engine, port, public, health_path, cpu_millis,
            memory_mb, webhook_secret, pre_deploy_command, cron_schedule)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
            $19, $20)
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
        optional_text(new.pre_deploy_command.clone()),
        cron_schedule,
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
    /// Some("") clears it.
    pub pre_deploy_command: Option<String>,
    /// Some("") clears it, and the service stays up again.
    pub cron_schedule: Option<String>,
    /// Copies to run; with autoscaling, where it starts.
    pub replicas: Option<i32>,
    /// Both set: autoscaling on, between them. Both null: off.
    pub autoscale_min: Option<Option<i32>>,
    pub autoscale_max: Option<Option<i32>>,
    pub autoscale_cpu: Option<i32>,
    /// The server (Swarm node id) to pin it to; null lets Swarm choose.
    pub node_id: Option<Option<String>>,
    /// `key=value`, or null for any server.
    pub node_label: Option<Option<String>>,
}

pub struct Scaling {
    pub replicas: i32,
    pub min: Option<i32>,
    pub max: Option<i32>,
    pub cpu: i32,
}

/// One copy for anything with a volume, a host port or a schedule; between 1
/// and 50 otherwise, with autoscaling bounds in order.
fn validate_scaling(single: bool, scaling: &Scaling) -> Result<()> {
    let autoscale = scaling.min.is_some() || scaling.max.is_some();
    if single && (scaling.replicas != 1 || autoscale) {
        return Err(bad_request(
            "a service with a volume, a host port or a schedule runs exactly one copy",
        ));
    }
    if !(1..=50).contains(&scaling.replicas) {
        return Err(bad_request("copies must be between 1 and 50"));
    }
    match (scaling.min, scaling.max) {
        (None, None) => {}
        (Some(min), Some(max)) if 1 <= min && min <= max && max <= 50 => {}
        (Some(_), Some(_)) => {
            return Err(bad_request(
                "autoscaling needs 1 ≤ fewest ≤ most ≤ 50 copies",
            ))
        }
        _ => return Err(bad_request("autoscaling needs both a fewest and a most")),
    }
    if !(10..=100).contains(&scaling.cpu) {
        return Err(bad_request(
            "the CPU target must be between 10 and 100 percent",
        ));
    }
    Ok(())
}

fn validate_node_label(label: Option<&str>) -> Result<()> {
    if let Some(label) = label {
        let ok = label
            .split_once('=')
            .is_some_and(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty());
        if !ok {
            return Err(bad_request(
                "write the server label as key=value, like region=eu",
            ));
        }
    }
    Ok(())
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
    let cron_schedule = match update.cron_schedule {
        Some(v) => optional_text(Some(v)),
        None => current.cron_schedule.clone(),
    };
    validate_schedule(
        &current.kind,
        cron_schedule.as_deref(),
        public,
        public_port,
        preview,
    )?;
    let pre_deploy_command = match update.pre_deploy_command {
        Some(v) => optional_text(Some(v)),
        None => current.pre_deploy_command.clone(),
    };
    let volumes = volumes(&state.db, id).await?;
    let single = current.kind == "database"
        || !volumes.is_empty()
        || public_port.is_some()
        || cron_schedule.is_some();
    let mut scaling = Scaling {
        replicas: update.replicas.unwrap_or(current.replicas),
        min: update.autoscale_min.unwrap_or(current.autoscale_min),
        max: update.autoscale_max.unwrap_or(current.autoscale_max),
        cpu: update.autoscale_cpu.unwrap_or(current.autoscale_cpu),
    };
    if single && update.replicas.is_none() {
        // Turning on a host port or a schedule brings it back to one copy.
        scaling.replicas = 1;
        scaling.min = None;
        scaling.max = None;
    }
    validate_scaling(single, &scaling)?;
    if let (Some(min), Some(max)) = (scaling.min, scaling.max) {
        scaling.replicas = scaling.replicas.clamp(min, max);
    }
    let node_id = update
        .node_id
        .map(|n| n.filter(|n| !n.is_empty()))
        .unwrap_or(current.node_id.clone());
    if !volumes.is_empty() && current.node_id.is_some() && node_id != current.node_id {
        return Err(bad_request(
            "its data lives on its server, and justdeploy can't move volumes between servers",
        ));
    }
    let node_label = update
        .node_label
        .map(optional_text)
        .unwrap_or(current.node_label.clone());
    validate_node_label(node_label.as_deref())?;
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
            pre_deploy_command = $16, cron_schedule = $17, replicas = $18, autoscale_min = $19,
            autoscale_max = $20, autoscale_cpu = $21, node_id = $22, node_label = $23,
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
        pre_deploy_command,
        cron_schedule,
        scaling.replicas,
        scaling.min,
        scaling.max,
        scaling.cpu,
        node_id,
        node_label,
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
            service_id: service.id,
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

/// A project's shared variables: every service in it gets them.
pub async fn shared_env(state: &SharedState, project_id: Uuid) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query!(
        "SELECT key, value_enc FROM shared_env_vars WHERE project_id = $1 ORDER BY key",
        project_id
    )
    .fetch_all(&state.db)
    .await?;
    rows.into_iter()
        .map(|r| Ok((r.key, state.cipher.decrypt(&r.value_enc)?)))
        .collect()
}

/// Replaces every shared variable. Returns whether anything changed.
pub async fn replace_shared_env(
    state: &SharedState,
    project_id: Uuid,
    vars: BTreeMap<String, String>,
) -> Result<bool> {
    validate_env(&vars)?;
    let before: BTreeMap<String, String> =
        shared_env(state, project_id).await?.into_iter().collect();
    if before == vars {
        return Ok(false);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!(
        "DELETE FROM shared_env_vars WHERE project_id = $1",
        project_id
    )
    .execute(&mut *tx)
    .await?;
    for (key, value) in &vars {
        sqlx::query!(
            "INSERT INTO shared_env_vars (project_id, key, value_enc) VALUES ($1, $2, $3)",
            project_id,
            key,
            state.cipher.encrypt(value)
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// What a container (and a build) gets: the project's shared variables, then
/// the service's own, which win, with `${{service.KEY}}` references to sibling
/// services filled in. A reference can name any variable of the sibling, or
/// HOST, PORT, and for a database URL.
///
/// For a pull request's preview, references to a database point at that
/// pull request's own copy of it.
pub async fn runtime_env(
    state: &SharedState,
    service: &Service,
    pr: Option<i32>,
) -> Result<Vec<(String, String)>> {
    let shared: BTreeMap<String, String> = shared_env(state, service.project_id)
        .await?
        .into_iter()
        .collect();
    let mut own = shared.clone();
    own.extend(env(state, service.id).await?);

    let wanted: std::collections::HashSet<String> = own
        .values()
        .flat_map(|v| references(v))
        .map(|(name, _)| name)
        .collect();
    let mut siblings = HashMap::new();
    for sibling in list_services(&state.db, service.project_id).await? {
        if !wanted.contains(&sibling.name) {
            continue;
        }
        let mut vars = shared.clone();
        vars.extend(env(state, sibling.id).await?);
        let host = match pr {
            Some(pr) if is_copied_for_previews(&sibling) => {
                names::preview_database(&sibling.name, pr)
            }
            _ => sibling.name.clone(),
        };
        if let Some(port) = sibling.port {
            vars.insert("PORT".into(), port.to_string());
        }
        if let Some(url) = connection_url_at(state, &sibling, &host).await? {
            vars.insert("URL".into(), url);
        }
        vars.insert("HOST".into(), host);
        siblings.insert(sibling.name.clone(), vars);
    }
    own.into_iter()
        .map(|(key, value)| {
            substitute(&value, &siblings)
                .map(|v| (key.clone(), v))
                .map_err(|e| bad_request(format!("{key}: {e}")))
        })
        .collect()
}

/// Every `${{service.KEY}}` in a value, as (service, key).
fn references(value: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else { break };
        if let Some((service, key)) = after[..end].trim().split_once('.') {
            found.push((service.trim().to_string(), key.trim().to_string()));
        }
        rest = &after[end + 2..];
    }
    found
}

fn substitute(
    value: &str,
    siblings: &HashMap<String, BTreeMap<String, String>>,
) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| "a ${{ reference is never closed with }}".to_string())?;
        let inner = after[..end].trim();
        let (service, key) = inner
            .split_once('.')
            .ok_or_else(|| format!("${{{{{inner}}}}} should look like ${{{{service.KEY}}}}"))?;
        let vars = siblings.get(service.trim()).ok_or_else(|| {
            format!("${{{{{inner}}}}} refers to {service}, which is not in this project")
        })?;
        let resolved = vars
            .get(key.trim())
            .ok_or_else(|| format!("${{{{{inner}}}}}: {service} has no variable {key}"))?;
        out.push_str(resolved);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// For a database, how sibling services connect to it.
pub async fn connection_url(state: &SharedState, service: &Service) -> Result<Option<String>> {
    connection_url_at(state, service, &service.name).await
}

async fn connection_url_at(
    state: &SharedState,
    service: &Service,
    host: &str,
) -> Result<Option<String>> {
    let Some(engine) = service.engine.as_deref() else {
        return Ok(None);
    };
    let env: HashMap<String, String> = env(state, service.id).await?.into_iter().collect();
    Ok(engines::connection_url(engine, host, &env))
}

/// Databases that get a copy per pull request: deployed and meant to be running.
pub fn is_copied_for_previews(service: &Service) -> bool {
    service.kind == "database"
        && service.current_deployment_id.is_some()
        && service.desired_state == "running"
}

/// Whether the service reaches a database of its project through a reference,
/// so a preview of it talks to its own copy rather than production's.
pub async fn references_database(state: &SharedState, service: &Service) -> Result<bool> {
    let databases: Vec<String> = list_services(&state.db, service.project_id)
        .await?
        .into_iter()
        .filter(is_copied_for_previews)
        .map(|s| s.name)
        .collect();
    let mut vars = shared_env(state, service.project_id).await?;
    vars.extend(env(state, service.id).await?);
    Ok(vars
        .iter()
        .flat_map(|(_, v)| references(v))
        .any(|(target, _)| databases.contains(&target)))
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

/// One line in the activity log. `actor` is an email; None is justdeploy itself.
pub async fn audit(
    db: &PgPool,
    actor: Option<&str>,
    action: &str,
    target: &str,
    detail: Option<&str>,
) {
    let result = sqlx::query!(
        "INSERT INTO audit_log (actor, action, target, detail) VALUES ($1, $2, $3, $4)",
        actor,
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
mod reference_tests {
    use super::*;

    #[test]
    fn references_resolve_from_siblings_and_report_what_is_missing() {
        let siblings = HashMap::from([(
            "db".to_string(),
            BTreeMap::from([
                (
                    "URL".to_string(),
                    "postgresql://u:p@db:5432/app".to_string(),
                ),
                ("HOST".to_string(), "db".to_string()),
            ]),
        )]);
        assert_eq!(
            substitute("${{db.URL}}?sslmode=disable", &siblings).unwrap(),
            "postgresql://u:p@db:5432/app?sslmode=disable"
        );
        assert_eq!(
            substitute("${{ db.HOST }}:5432", &siblings).unwrap(),
            "db:5432"
        );
        assert_eq!(
            substitute("plain ${value}", &siblings).unwrap(),
            "plain ${value}"
        );
        assert!(substitute("${{cache.URL}}", &siblings)
            .unwrap_err()
            .contains("not in this project"));
        assert!(substitute("${{db.NOPE}}", &siblings)
            .unwrap_err()
            .contains("no variable NOPE"));
        assert!(substitute("${{db.URL", &siblings).is_err());
        assert_eq!(
            references("a ${{db.URL}} b ${{api.PORT}}"),
            vec![("db".into(), "URL".into()), ("api".into(), "PORT".into())]
        );
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

#[cfg(test)]
mod scaling_tests {
    use super::*;

    fn scaling(replicas: i32, min: Option<i32>, max: Option<i32>) -> Scaling {
        Scaling {
            replicas,
            min,
            max,
            cpu: 70,
        }
    }

    #[test]
    fn copies_and_autoscaling_bounds_are_checked() {
        assert!(validate_scaling(false, &scaling(3, None, None)).is_ok());
        assert!(validate_scaling(false, &scaling(2, Some(1), Some(5))).is_ok());
        assert!(validate_scaling(false, &scaling(0, None, None)).is_err());
        assert!(validate_scaling(false, &scaling(2, Some(5), Some(1))).is_err());
        assert!(validate_scaling(false, &scaling(2, Some(1), None)).is_err());
        assert!(
            validate_scaling(true, &scaling(2, None, None)).is_err(),
            "volumes run one copy"
        );
        assert!(validate_scaling(true, &scaling(1, None, None)).is_ok());
        assert!(validate_node_label(Some("region=eu")).is_ok());
        assert!(validate_node_label(Some("region")).is_err());
    }
}
