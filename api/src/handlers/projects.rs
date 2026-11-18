use crate::{
    auth::Admin,
    docker::{self, LABEL_KIND, LABEL_PROJECT, LABEL_SERVICE},
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
use bollard::models::{ContainerSummary, ContainerSummaryStateEnum};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Serialize)]
pub struct ServiceView {
    #[serde(flatten)]
    service: Service,
    /// deploying | running | scheduled | crashed | stopped | failed | idle | deleting
    status: String,
    url: Option<String>,
    connection_url: Option<String>,
    has_git_token: bool,
    latest_deployment: Option<Deployment>,
}

#[derive(Serialize)]
pub struct ProjectView {
    #[serde(flatten)]
    project: Project,
    services: Vec<ServiceView>,
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

pub(super) fn status(
    service: &Service,
    latest: Option<&Deployment>,
    containers: &[&ContainerSummary],
) -> String {
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
    let running = containers
        .iter()
        .any(|c| matches!(c.state, Some(ContainerSummaryStateEnum::RUNNING)));
    if running { "running" } else { "crashed" }.into()
}

pub async fn view_service(
    state: &SharedState,
    project: &Project,
    service: Service,
    containers: &[ContainerSummary],
    base: &str,
) -> Result<ServiceView> {
    let latest = latest_deployment(state, service.id).await?;
    let id = service.id.to_string();
    let mine: Vec<&ContainerSummary> = containers
        .iter()
        .filter(|c| docker::label(c, LABEL_SERVICE) == Some(id.as_str()))
        .filter(|c| docker::label(c, LABEL_KIND) == Some("production"))
        .collect();
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
        status: status(&service, latest.as_ref(), &mine),
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
    containers: &[ContainerSummary],
    base: &str,
) -> Result<ProjectView> {
    let services = services::list_services(&state.db, project.id).await?;
    let views = futures::future::try_join_all(
        services
            .into_iter()
            .map(|service| view_service(state, &project, service, containers, base)),
    )
    .await?;
    Ok(ProjectView {
        project,
        services: views,
    })
}

/// One Docker listing and one settings read for the whole page.
pub async fn list(_: Admin, State(state): State<SharedState>) -> Result<Json<Vec<ProjectView>>> {
    let containers = docker::list(&state.docker, &[]).await?;
    let base = services::base_domain(&state).await?;
    let projects = services::list_projects(&state.db).await?;
    let views = futures::future::try_join_all(
        projects
            .into_iter()
            .map(|project| project_view(&state, project, &containers, &base)),
    )
    .await?;
    Ok(Json(views))
}

#[derive(Deserialize)]
pub struct NewProject {
    name: String,
}

pub async fn create(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<NewProject>,
) -> Result<(StatusCode, Json<Project>)> {
    let project = services::create_project(&state.db, body.name.trim()).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn get(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectView>> {
    let project = services::get_project(&state.db, id).await?;
    let pid = project.id.to_string();
    let containers = docker::list(&state.docker, &[(LABEL_PROJECT, &pid)]).await?;
    let base = services::base_domain(&state).await?;
    Ok(Json(
        project_view(&state, project, &containers, &base).await?,
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

// ---------------------------------------------------------------- services

pub async fn create_service(
    _: Admin,
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
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ServiceDetail>> {
    let service = services::get_service(&state.db, id).await?;
    let project = services::get_project(&state.db, service.project_id).await?;
    let pid = project.id.to_string();
    let containers = docker::list(&state.docker, &[(LABEL_PROJECT, &pid)]).await?;
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
            secret: service.webhook_secret.clone(),
        },
        volumes: services::volumes(&state.db, id).await?,
        domains: services::domains(&state.db, id).await?,
        generated_host,
        view: view_service(&state, &project, service, &containers, &base).await?,
        project,
    }))
}

pub async fn update_service(
    _: Admin,
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
    _: Admin,
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
            for container in docker::list(
                &state.docker,
                &[(LABEL_SERVICE, &sid), (LABEL_KIND, "production")],
            )
            .await?
            {
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
    _: Admin,
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
    _: Admin,
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
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Deployment>>> {
    Ok(Json(services::list_deployments(&state.db, id).await?))
}

pub async fn deployment(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Deployment>> {
    Ok(Json(services::get_deployment(&state.db, id).await?))
}

pub async fn rollback(
    _: Admin,
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
    _: Admin,
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
    _: Admin,
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

#[derive(Deserialize)]
pub struct NewDomain {
    hostname: String,
}

pub async fn add_domain(
    _: Admin,
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
    _: Admin,
    State(state): State<SharedState>,
    Path((id, domain_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let service = services::get_service(&state.db, id).await?;
    services::remove_domain(&state.db, id, domain_id).await?;
    services::rerelease(&state, &service, "domains").await?;
    Ok(StatusCode::NO_CONTENT)
}
