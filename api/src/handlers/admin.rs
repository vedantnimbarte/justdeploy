//! Settings, templates, backups and the webhook endpoints.

use crate::{
    auth::{Admin, Member, Viewer},
    backups::{self, DestinationConfig, Schedule},
    engines,
    error::{bad_request, Result},
    github, names, notify, services,
    state::SharedState,
    templates::{self, DeployTemplate, Template},
    webhooks,
};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::IpAddr, time::Duration};
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

pub async fn get_settings(_: Viewer, State(state): State<SharedState>) -> Result<Json<Settings>> {
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

// ---------------------------------------------------------------- GitHub App

pub async fn github_overview(
    _: Viewer,
    State(state): State<SharedState>,
) -> Result<Json<github::Overview>> {
    Ok(Json(github::overview(&state).await?))
}

#[derive(Deserialize)]
pub struct ManifestRequest {
    /// Create the app under this organization instead of a personal account.
    org: Option<String>,
}

pub async fn github_manifest(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<ManifestRequest>,
) -> Result<Json<github::ManifestForm>> {
    Ok(Json(github::manifest(&state, body.org.as_deref()).await?))
}

#[derive(Deserialize)]
pub struct Conversion {
    code: String,
    state: String,
}

pub async fn github_create(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<Conversion>,
) -> Result<StatusCode> {
    github::finish_creation(&state, &body.code, &body.state).await?;
    Ok(StatusCode::CREATED)
}

pub async fn github_forget(_: Admin, State(state): State<SharedState>) -> Result<StatusCode> {
    github::forget(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn github_repos(
    _: Member,
    State(state): State<SharedState>,
) -> Result<Json<Vec<github::Repo>>> {
    Ok(Json(github::repos(&state).await?))
}

pub async fn github_app_webhook(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>> {
    Ok(Json(github::webhook(&state, &headers, &body).await?))
}

// ---------------------------------------------------------------- notifications

pub async fn list_channels(
    _: Viewer,
    State(state): State<SharedState>,
) -> Result<Json<Vec<notify::Channel>>> {
    Ok(Json(notify::list(&state).await?))
}

pub async fn create_channel(
    _: Admin,
    State(state): State<SharedState>,
    Json(body): Json<notify::NewChannel>,
) -> Result<(StatusCode, Json<Value>)> {
    let id = notify::create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn delete_channel(
    _: Admin,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    notify::delete(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- dns check

#[derive(Deserialize)]
pub struct DnsQuery {
    host: String,
}

#[derive(Serialize)]
pub struct DnsCheck {
    /// This server's addresses, IPv4 and public ones first.
    expected: Vec<IpAddr>,
    found: Vec<IpAddr>,
    /// "connected", "missing" (no record), "elsewhere" (another server) or
    /// "unknown" (this server's own address could not be worked out).
    status: &'static str,
}

/// Whether a hostname resolves to this server. A wildcard (`*.example.com`) is
/// checked through a made-up label under it.
pub async fn dns_check(
    _: Viewer,
    State(state): State<SharedState>,
    Query(q): Query<DnsQuery>,
) -> Result<Json<DnsCheck>> {
    let host = q.host.trim().to_lowercase();
    let wildcard = host.strip_prefix("*.");
    let probe = wildcard.map_or(host.clone(), |base| format!("jd-dns-check.{base}"));
    names::validate_hostname(&probe)?;

    // This server is wherever the dashboard URL points and, for custom domains,
    // wherever the generated hostnames already resolve.
    let url = &state.config.public_url;
    let authority = url.split("://").last().unwrap_or(url);
    let mut expected = resolve(authority.split('/').next().unwrap_or_default()).await;
    let base = services::base_domain(&state).await?;
    if wildcard.is_none() && base != "localhost" {
        expected.extend(resolve(&format!("jd-dns-check.{base}")).await);
    }
    expected.retain(|ip| !ip.is_loopback());
    expected.sort_by_key(|ip| {
        (
            !ip.is_ipv4(),
            matches!(ip, IpAddr::V4(v4) if v4.is_private()),
            *ip,
        )
    });
    expected.dedup();

    let found = resolve(&probe).await;
    let status = if found.is_empty() {
        "missing"
    } else if expected.is_empty() {
        "unknown"
    } else if found.iter().any(|ip| expected.contains(ip)) {
        "connected"
    } else {
        "elsewhere"
    };
    Ok(Json(DnsCheck {
        expected,
        found,
        status,
    }))
}

/// Addresses for `host` or `host:port`; empty when it does not resolve in 5s.
async fn resolve(host: &str) -> Vec<IpAddr> {
    let has_port = host
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok());
    let target = if has_port {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    match tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(target)).await {
        Ok(Ok(addrs)) => addrs.map(|a| a.ip()).collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------- templates

pub async fn list_templates(_: Viewer) -> Json<Vec<Template>> {
    Json(templates::catalog())
}

pub async fn deploy_template(
    _: Member,
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
    _: Viewer,
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
    _: Viewer,
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
    _: Member,
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
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<Schedule>>> {
    Ok(Json(backups::get_schedule(&state, id).await?))
}

pub async fn put_schedule(
    _: Member,
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

#[cfg(test)]
mod tests {
    use super::resolve;

    #[tokio::test]
    async fn resolves_hosts_with_or_without_a_port() {
        let has = |ips: Vec<std::net::IpAddr>, ip: &str| ips.iter().any(|a| a.to_string() == ip);
        assert!(has(resolve("127.0.0.1").await, "127.0.0.1"));
        assert!(has(resolve("127.0.0.1:8080").await, "127.0.0.1"));
        assert!(has(resolve("[::1]").await, "::1"));
        assert!(resolve("nothing-here.invalid").await.is_empty());
    }
}
