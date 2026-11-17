//! Settings, templates, backups and the webhook endpoints.

use crate::{
    auth::{self, Admin},
    backups::{self, DestinationConfig, Schedule},
    engines,
    error::{bad_request, Result},
    services,
    state::SharedState,
    templates::{self, DeployTemplate, Template},
    webhooks,
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ---------------------------------------------------------------- settings

#[derive(Serialize)]
pub struct Settings {
    base_domain: String,
    tls: bool,
    acme_email: Option<String>,
    public_url: String,
    version: &'static str,
    engines: Vec<Value>,
}

pub async fn get_settings(_: Admin, State(state): State<SharedState>) -> Result<Json<Settings>> {
    Ok(Json(Settings {
        base_domain: services::base_domain(&state).await?,
        tls: state.config.tls(),
        acme_email: state.config.acme_email.clone(),
        public_url: state.config.public_url.clone(),
        version: env!("CARGO_PKG_VERSION"),
        engines: engines::ENGINES
            .iter()
            .map(|e| json!({ "id": e.id, "label": e.label, "image": e.image }))
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct SettingsUpdate {
    base_domain: String,
}

/// Changing the base domain re-releases every public service onto the new hostnames.
pub async fn put_settings(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<SettingsUpdate>,
) -> Result<StatusCode> {
    let domain = body.base_domain.trim().to_lowercase();
    if domain == services::base_domain(&state).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    services::set_base_domain(&state.db, &domain).await?;
    for project in services::list_projects(&state.db).await? {
        for service in services::list_services(&state.db, project.id).await? {
            if service.public {
                services::rerelease(&state, &service, "settings").await?;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current: String,
    new: String,
}

pub async fn change_password(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<PasswordChange>,
) -> Result<StatusCode> {
    if !auth::check_password(&state, &body.current).await? {
        return Err(bad_request("the current password is wrong"));
    }
    auth::validate_password(&body.new)?;
    auth::set_password(&state.db, &body.new).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- templates

pub async fn list_templates(_: Admin) -> Json<Vec<Template>> {
    Json(templates::catalog())
}

pub async fn deploy_template(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<DeployTemplate>,
) -> Result<(StatusCode, Json<Value>)> {
    let project = templates::deploy(&state, &id, body).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "project_id": project.id })),
    ))
}

// ---------------------------------------------------------------- backups

pub async fn list_destinations(
    _: Admin,
    State(state): State<SharedState>,
) -> Result<Json<Vec<backups::Destination>>> {
    Ok(Json(backups::list_destinations(&state).await?))
}

#[derive(Deserialize)]
pub struct NewDestination {
    name: String,
    #[serde(flatten)]
    config: DestinationConfig,
}

pub async fn create_destination(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<NewDestination>,
) -> Result<(StatusCode, Json<Value>)> {
    let id = backups::create_destination(&state, &body.name, body.config).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn delete_destination(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    backups::delete_destination(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_backups(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<backups::Backup>>> {
    Ok(Json(backups::list(&state, id).await?))
}

#[derive(Deserialize)]
pub struct BackupNow {
    destination_id: Uuid,
}

pub async fn backup_now(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<BackupNow>,
) -> Result<(StatusCode, Json<Value>)> {
    let service = services::get_service(&state.db, id).await?;
    let backup = backups::start_backup(&state, &service, body.destination_id, "manual").await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": backup }))))
}

pub async fn restore(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    backups::start_restore(&state, id).await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn get_schedule(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<Schedule>>> {
    Ok(Json(backups::get_schedule(&state, id).await?))
}

pub async fn put_schedule(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Option<Schedule>>,
) -> Result<StatusCode> {
    let service = services::get_service(&state.db, id).await?;
    backups::set_schedule(&state, &service, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- webhooks (no session)

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|e| bad_request(format!("unexpected payload: {e}")))
}

pub async fn github(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>> {
    let service = services::get_service(&state.db, id).await?;
    if !webhooks::verify_github(
        &service.webhook_secret,
        header(&headers, "x-hub-signature-256"),
        &body,
    ) {
        return Err(webhooks::forbidden());
    }
    let message = match header(&headers, "x-github-event") {
        "ping" => "pong".to_string(),
        "push" => webhooks::on_push(&state, &service, parse(&body)?).await?,
        "pull_request" => webhooks::on_pull_request(&state, &service, parse(&body)?).await?,
        other => format!("ignored: {other} events are not used"),
    };
    Ok(Json(json!({ "message": message })))
}

pub async fn gitlab(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>> {
    let service = services::get_service(&state.db, id).await?;
    if !webhooks::verify_gitlab(&service.webhook_secret, header(&headers, "x-gitlab-token")) {
        return Err(webhooks::forbidden());
    }
    let message = match header(&headers, "x-gitlab-event") {
        "Push Hook" => webhooks::on_push(&state, &service, parse(&body)?).await?,
        other => format!("ignored: {other} events are not used"),
    };
    Ok(Json(json!({ "message": message })))
}
