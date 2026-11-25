use crate::{
    auth::{Admin, Can, Member, Role, Viewer},
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::{bad_request, Result},
    models::{Deployment, Domain, Project, Run, Service, Volume},
    names, runs,
    services::{self, NewService, ServiceUpdate},
    state::SharedState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

#[derive(Serialize)]
pub struct ServiceView {
    #[serde(flatten)]
    service: Service,
    /// deploying | running | scheduled | crashed | stopped | failed | idle | deleting
    status: String,
    /// Production copies up, and how many should be.
    copies_running: u64,
    copies_desired: u64,
    url: Option<String>,
    connection_url: Option<String>,
    has_git_token: bool,
    latest_deployment: Option<Deployment>,
}

impl ServiceView {
    /// Drops what a viewer may not see.
    fn for_role(mut self, role: Role) -> Self {
        if role < Role::Member {
            self.connection_url = None;
        }
        self
    }
}

#[derive(Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    project: Project,
    services: Vec<ServiceView>,
    /// Every environment of this project, production first.
    environments: Vec<EnvironmentRef>,
}

#[derive(Serialize)]
struct EnvironmentRef {
    id: Uuid,
    environment: String,
}

pub(super) async fn latest_deployment(
    state: &SharedState,
    service_id: Uuid,
) -> Result<Option<Deployment>> {
    Ok(sqlx::query_as!(
        Deployment,
        "SELECT * FROM deployments WHERE service_id = $1 AND kind = 'production'
         ORDER BY created_at DESC LIMIT 1",
        service_id
    )
    .fetch_optional(&state.db)
    .await?)
}

/// How many production copies of a service run, and should.
#[derive(Default, Clone, Copy)]
pub(super) struct Copies {
    pub running: u64,
    pub desired: u64,
}

/// Copies of every service, from one look at the Swarm.
pub(super) async fn copies(state: &SharedState) -> Result<HashMap<Uuid, Copies>> {
    let mut out: HashMap<Uuid, Copies> = HashMap::new();
    for w in docker::workloads(&state.docker, &[(LABEL_KIND, "production")]).await? {
        if let Some(id) = w.label(LABEL_SERVICE).and_then(|s| s.parse().ok()) {
            let c = out.entry(id).or_default();
            // A server that went down still counts its copies as running until
            // Swarm gives up on them; never show more than should run.
            c.running += w.running.min(w.desired);
            c.desired += w.desired;
        }
    }
    // Plain containers from before the cluster, until they move onto it.
    for c in docker::list(&state.docker, &[(LABEL_KIND, "production")]).await? {
        if let Some(id) = docker::label(&c, LABEL_SERVICE).and_then(|s| s.parse().ok()) {
            let e = out.entry(id).or_default();
            e.desired += 1;
            e.running += docker::is_running(&c) as u64;
        }
    }
    Ok(out)
}

pub(super) fn status(service: &Service, latest: Option<&Deployment>, copies: Copies) -> String {
    if service.desired_state == "deleting" {
        return "deleting".into();
    }
    if latest.is_some_and(|d| matches!(d.status.as_str(), "queued" | "building" | "deploying")) {
        return "deploying".into();
    }
    if service.desired_state == "stopped" {
        return "stopped".into();
    }
    if service.current_deployment_id.is_none() {
        return if latest.is_some_and(|d| d.status == "failed") {
            "failed"
        } else {
            "idle"
        }
        .into();
    }
    if service.cron_schedule.is_some() {
        return "scheduled".into();
    }
    if copies.running > 0 {
        "running"
    } else {
        "crashed"
    }
    .into()
}

pub async fn view_service(
    state: &SharedState,
    project: &Project,
    service: Service,
    copies: &HashMap<Uuid, Copies>,
    base: &str,
) -> Result<ServiceView> {
    let latest = latest_deployment(state, service.id).await?;
    let mine = copies.get(&service.id).copied().unwrap_or_default();
    let url = if service.public && service.port.is_some() {
        let scheme = if state.config.tls() { "https" } else { "http" };
        Some(format!(
            "{scheme}://{}",
            names::generated_host(&service.name, &project.name, base)
        ))
    } else {
        None
    };
    Ok(ServiceView {
        status: status(&service, latest.as_ref(), mine),
        copies_running: mine.running,
        copies_desired: mine.desired,
        url,
        connection_url: services::connection_url(state, &service).await?,
        has_git_token: service.git_token_enc.is_some(),
        latest_deployment: latest,
        service,
    })
}

async fn project_view(
    state: &SharedState,
    project: Project,
    copies: &HashMap<Uuid, Copies>,
    base: &str,
    role: Role,
) -> Result<ProjectView> {
    let services = services::list_services(&state.db, project.id).await?;
    let views = futures::future::try_join_all(
        services
            .into_iter()
            .map(|service| view_service(state, &project, service, copies, base)),
    )
    .await?;
    let environments = services::environments(&state.db, &project)
        .await?
        .into_iter()
        .map(|p| EnvironmentRef {
            id: p.id,
            environment: p.environment,
        })
        .collect();
    Ok(ProjectView {
        project,
        services: views.into_iter().map(|v| v.for_role(role)).collect(),
        environments,
    })
}

/// One Docker listing and one settings read for the whole page. Lists base
/// projects; their environments come with each.
pub async fn list(
    Can(user): Viewer,
    State(state): State<SharedState>,
) -> Result<Json<Vec<ProjectView>>> {
    let copies = copies(&state).await?;
    let base = services::base_domain(&state).await?;
    let projects = services::list_projects(&state.db).await?;
    let views = futures::future::try_join_all(
        projects
            .into_iter()
            .filter(|p| p.base_id.is_none())
            .map(|project| project_view(&state, project, &copies, &base, user.role)),
    )
    .await?;
    Ok(Json(views))
}

#[derive(Deserialize)]
pub struct NewProject {
    name: String,
}

pub async fn create(
    _: Member,
    State(state): State<SharedState>,
    Json(body): Json<NewProject>,
) -> Result<(StatusCode, Json<Project>)> {
    let project = services::create_project(&state.db, body.name.trim()).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn get(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectView>> {
    let project = services::get_project(&state.db, id).await?;
    let copies = copies(&state).await?;
    let base = services::base_domain(&state).await?;
    Ok(Json(
        project_view(&state, project, &copies, &base, user.role).await?,
    ))
}

pub async fn delete(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    services::delete_project(&state, id).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
pub struct NewEnvironment {
    environment: String,
    /// The branch app services track in it; empty keeps each one's own.
    branch: Option<String>,
}

pub async fn create_environment(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<NewEnvironment>,
) -> Result<(StatusCode, Json<Project>)> {
    let from = services::get_project(&state.db, id).await?;
    let project = services::create_environment(
        &state,
        &from,
        body.environment.trim(),
        body.branch.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(project)))
}

#[derive(Deserialize)]
pub struct Sync {
    /// The environment to copy settings from.
    from: Uuid,
}

pub async fn sync_environment(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Sync>,
) -> Result<Json<services::SyncReport>> {
    let target = services::get_project(&state.db, id).await?;
    let source = services::get_project(&state.db, body.from).await?;
    Ok(Json(
        services::sync_environment(&state, &target, &source).await?,
    ))
}

/// Which services reference which: variable names only, so viewers may see it.
pub async fn graph(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<services::Edge>>> {
    services::get_project(&state.db, id).await?;
    Ok(Json(services::project_graph(&state, id).await?))
}

#[derive(Deserialize)]
pub struct UsageQuery {
    /// day | week | quarter
    range: Option<String>,
}

pub async fn usage(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<UsageQuery>,
) -> Result<Json<Vec<crate::usage::Point>>> {
    services::get_project(&state.db, id).await?;
    let range = query.range.as_deref().unwrap_or("day");
    Ok(Json(crate::usage::series(&state, id, range).await?))
}

pub async fn uptime(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::uptime::ServiceUptime>>> {
    Ok(Json(crate::uptime::summary(&state, id, false).await?))
}

#[derive(Deserialize)]
pub struct StatusPageSettings {
    enabled: bool,
    domain: Option<String>,
    #[serde(default)]
    services: Vec<Uuid>,
}

pub async fn configure_status_page(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusPageSettings>,
) -> Result<StatusCode> {
    services::get_project(&state.db, id).await?;
    crate::uptime::configure(&state, id, body.enabled, body.domain, &body.services).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A published status page. No sign-in: it's public.
pub async fn public_status(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> Result<Json<crate::uptime::StatusPage>> {
    Ok(Json(crate::uptime::status_page(&state, &name).await?))
}

pub async fn pitr_status(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::pitr::Status>> {
    let service = services::get_service(&state.db, id).await?;
    Ok(Json(crate::pitr::status(&state, &service).await?))
}

#[derive(Deserialize)]
pub struct PitrSettings {
    /// An S3 destination to archive to; null turns it off.
    destination_id: Option<Uuid>,
}

pub async fn configure_pitr(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PitrSettings>,
) -> Result<StatusCode> {
    let service = services::get_service(&state.db, id).await?;
    crate::pitr::configure(&state, &service, body.destination_id).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
pub struct PitrRestore {
    /// The moment to restore, any time zone.
    target: chrono::DateTime<chrono::Utc>,
}

/// A new database with this one's data.
pub async fn copy_database(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Service>)> {
    let source = services::get_service(&state.db, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::data::create_copy(&state, &source).await?),
    ))
}

pub async fn list_copies(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::data::Copy>>> {
    Ok(Json(crate::data::list(&state, id).await?))
}

#[derive(Deserialize)]
pub struct ReplaceData {
    /// The database whose data replaces this one's.
    from: Uuid,
}

/// Replaces this database's data with another's. Admins, like restores.
pub async fn replace_data(
    _: crate::auth::Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplaceData>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let target = services::get_service(&state.db, id).await?;
    let source = services::get_service(&state.db, body.from).await?;
    if source.project_id != target.project_id {
        return Err(crate::error::bad_request(
            "pick a database from the same project",
        ));
    }
    let copy = crate::data::replace(&state, &target, &source).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "id": copy })),
    ))
}

pub async fn list_replicas(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::data::Replica>>> {
    let primary = services::get_service(&state.db, id).await?;
    Ok(Json(crate::data::replicas(&state, &primary).await?))
}

pub async fn create_replica(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Service>)> {
    let primary = services::get_service(&state.db, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::data::create_replica(&state, &primary).await?),
    ))
}

pub async fn pitr_restore(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PitrRestore>,
) -> Result<(StatusCode, Json<Service>)> {
    let source = services::get_service(&state.db, id).await?;
    let restored = crate::pitr::restore(&state, &source, body.target).await?;
    Ok((StatusCode::ACCEPTED, Json(restored)))
}

// ---------------------------------------------------------------- services

pub async fn create_service(
    _: Member,
    State(state): State<SharedState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<NewService>,
) -> Result<(StatusCode, Json<Service>)> {
    let service = services::create_service(&state, project_id, body).await?;
    Ok((StatusCode::CREATED, Json(service)))
}

#[derive(Serialize)]
pub struct ServiceDetail {
    #[serde(flatten)]
    view: ServiceView,
    project: Project,
    volumes: Vec<Volume>,
    domains: Vec<Domain>,
    generated_host: Option<String>,
    webhooks: Webhooks,
}

#[derive(Serialize)]
struct Webhooks {
    github: String,
    gitlab: String,
    secret: String,
}

pub async fn get_service(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ServiceDetail>> {
    let service = services::get_service(&state.db, id).await?;
    let project = services::get_project(&state.db, service.project_id).await?;
    let copies = copies(&state).await?;
    let base = services::base_domain(&state).await?;
    let generated_host = (service.public && service.port.is_some())
        .then(|| names::generated_host(&service.name, &project.name, &base));
    let webhook = |provider: &str| {
        format!(
            "{}/api/v1/webhooks/{provider}/{}",
            state.config.public_url, service.id
        )
    };
    Ok(Json(ServiceDetail {
        webhooks: Webhooks {
            github: webhook("github"),
            gitlab: webhook("gitlab"),
            secret: if user.role >= Role::Member {
                service.webhook_secret.clone()
            } else {
                String::new()
            },
        },
        volumes: services::volumes(&state.db, id).await?,
        domains: services::domains(&state.db, id).await?,
        generated_host,
        view: view_service(&state, &project, service, &copies, &base)
            .await?
            .for_role(user.role),
        project,
    }))
}

pub async fn update_service(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ServiceUpdate>,
) -> Result<Json<Service>> {
    Ok(Json(services::update_service(&state, id, body).await?))
}

pub async fn delete_service(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    services::get_service(&state.db, id).await?;
    services::delete_service(&state, id).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
pub struct Action {
    action: String,
}

/// deploy (fresh build or pull) | restart (same image) | stop | start
pub async fn service_action(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Action>,
) -> Result<StatusCode> {
    let service = services::get_service(&state.db, id).await?;
    match body.action.as_str() {
        "deploy" => {
            services::deploy(&state, &service, "manual").await?;
        }
        "restart" | "start" => {
            services::set_desired(&state.db, id, "running").await?;
            let service = services::get_service(&state.db, id).await?;
            services::rerelease(&state, &service, "manual").await?;
        }
        "stop" => {
            services::set_desired(&state.db, id, "stopped").await?;
            let sid = id.to_string();
            let labels = [(LABEL_SERVICE, sid.as_str()), (LABEL_KIND, "production")];
            for workload in docker::workloads(&state.docker, &labels).await? {
                docker::scale(&state.docker, &workload.name, 0).await?;
            }
            for container in docker::list(&state.docker, &labels).await? {
                if let Some(cid) = container.id {
                    docker::stop(&state.docker, &cid).await?;
                }
            }
        }
        _ => return Err(bad_request("action must be deploy, restart, start or stop")),
    }
    Ok(StatusCode::ACCEPTED)
}

// ---------------------------------------------------------------- runs

pub async fn list_runs(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<runs::Runs>> {
    let service = services::get_service(&state.db, id).await?;
    Ok(Json(runs::list(&state, &service).await?))
}

#[derive(Deserialize)]
pub struct NewRun {
    /// Empty runs the image's own command.
    command: Option<String>,
}

pub async fn start_run(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<NewRun>,
) -> Result<(StatusCode, Json<Run>)> {
    let service = services::get_service(&state.db, id).await?;
    let run = runs::start(&state, &service, body.command, "manual").await?;
    Ok((StatusCode::ACCEPTED, Json(run)))
}

// ---------------------------------------------------------------- deployments

pub async fn deployments(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Deployment>>> {
    Ok(Json(services::list_deployments(&state.db, id).await?))
}

pub async fn deployment(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Deployment>> {
    Ok(Json(services::get_deployment(&state.db, id).await?))
}

pub async fn rollback(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Deployment>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(services::rollback(&state, id).await?),
    ))
}

// ---------------------------------------------------------------- env and domains

#[derive(Serialize, Deserialize)]
pub struct EnvVar {
    key: String,
    value: String,
}

pub async fn get_env(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EnvVar>>> {
    services::get_service(&state.db, id).await?;
    let vars = services::env(&state, id).await?;
    Ok(Json(
        vars.into_iter()
            .map(|(key, value)| EnvVar { key, value })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct EnvSaved {
    redeploying: bool,
}

/// Replaces all variables and re-releases the current image when they changed.
pub async fn put_env(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(vars): Json<Vec<EnvVar>>,
) -> Result<Json<EnvSaved>> {
    let service = services::get_service(&state.db, id).await?;
    let mut map = BTreeMap::new();
    for var in vars {
        let key = var.key.trim().to_string();
        if map.insert(key.clone(), var.value).is_some() {
            return Err(bad_request(format!("{key} appears twice")));
        }
    }
    let changed = services::replace_env(&state, id, map).await?;
    let redeploying = changed
        && services::rerelease(&state, &service, "env")
            .await?
            .is_some();
    Ok(Json(EnvSaved { redeploying }))
}

/// The project's shared variables. Members only: they are secrets.
pub async fn get_shared_env(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EnvVar>>> {
    services::get_project(&state.db, id).await?;
    let vars = services::shared_env(&state, id).await?;
    Ok(Json(
        vars.into_iter()
            .map(|(key, value)| EnvVar { key, value })
            .collect(),
    ))
}

/// Replaces the shared variables and re-releases every running service in the
/// project when they changed.
pub async fn put_shared_env(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(vars): Json<Vec<EnvVar>>,
) -> Result<Json<EnvSaved>> {
    services::get_project(&state.db, id).await?;
    let mut map = BTreeMap::new();
    for var in vars {
        let key = var.key.trim().to_string();
        if map.insert(key.clone(), var.value).is_some() {
            return Err(bad_request(format!("{key} appears twice")));
        }
    }
    let mut redeploying = false;
    if services::replace_shared_env(&state, id, map).await? {
        for service in services::list_services(&state.db, id).await? {
            redeploying |= services::rerelease(&state, &service, "env")
                .await?
                .is_some();
        }
    }
    Ok(Json(EnvSaved { redeploying }))
}

#[derive(Deserialize)]
pub struct NewDomain {
    hostname: String,
}

pub async fn add_domain(
    _: Member,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<NewDomain>,
) -> Result<(StatusCode, Json<Domain>)> {
    let service = services::get_service(&state.db, id).await?;
    let domain = services::add_domain(&state.db, &service, &body.hostname).await?;
    services::rerelease(&state, &service, "domains").await?;
    Ok((StatusCode::CREATED, Json(domain)))
}

pub async fn remove_domain(
    _: Member,
    State(state): State<SharedState>,
    Path((id, domain_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let service = services::get_service(&state.db, id).await?;
    services::remove_domain(&state.db, id, domain_id).await?;
    services::rerelease(&state, &service, "domains").await?;
    Ok(StatusCode::NO_CONTENT)
}
