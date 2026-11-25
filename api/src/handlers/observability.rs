//! Requests, logs, traces and the errors overview of a project, optionally
//! narrowed to one service.

use crate::{
    auth::Viewer,
    error::{bad_request, Result},
    logs, services,
    state::SharedState,
    traces,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct Filters {
    service: Option<Uuid>,
    /// `production` or `preview`.
    env: Option<String>,
    /// Requests: `2xx` to `5xx`.
    status: Option<String>,
    method: Option<String>,
    /// Logs: `debug`, `info`, `warn` or `error`.
    level: Option<String>,
    #[serde(default)]
    q: String,
    /// The last this many hours, unless `from` is given.
    hours: Option<i32>,
    /// A fixed window instead: from (inclusive) to (exclusive, default now).
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    before: Option<i64>,
}

impl Filters {
    fn env(&self) -> Result<Option<&str>> {
        match self.env.as_deref() {
            None | Some("") => Ok(None),
            Some(env @ ("production" | "preview")) => Ok(Some(env)),
            Some(_) => Err(bad_request("env must be production or preview")),
        }
    }

    fn method(&self) -> Option<&str> {
        self.method.as_deref().filter(|m| !m.is_empty())
    }

    /// The time window searched, at most a year long.
    fn window(&self, default_hours: i32) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
        let to = self.to.unwrap_or_else(Utc::now);
        let hours = self.hours.unwrap_or(default_hours).clamp(1, 24 * 365);
        let from = self.from.unwrap_or(to - Duration::hours(hours.into()));
        if from >= to {
            return Err(bad_request("from must be before to"));
        }
        Ok((from.max(to - Duration::days(365)), to))
    }

    fn requests(&self, project: Uuid) -> Result<traces::RequestSearch<'_>> {
        let class = match self.status.as_deref() {
            None | Some("") => None,
            Some(s @ ("2xx" | "3xx" | "4xx" | "5xx")) => s[..1].parse().ok(),
            Some(_) => return Err(bad_request("status must be 2xx, 3xx, 4xx or 5xx")),
        };
        let (from, to) = self.window(1)?;
        Ok(traces::RequestSearch {
            project,
            service: self.service,
            env: self.env()?,
            class,
            method: self.method(),
            query: &self.q,
            from,
            to,
            before: self.before,
        })
    }
}

pub async fn requests(
    _: Viewer,
    State(state): State<SharedState>,
    Path(project): Path<Uuid>,
    Query(f): Query<Filters>,
) -> Result<Json<Vec<traces::Request>>> {
    services::get_project(&state.db, project).await?;
    Ok(Json(traces::requests(&state, f.requests(project)?).await?))
}

/// Requests over the window by status class, for the bar strip above the list.
pub async fn volume(
    _: Viewer,
    State(state): State<SharedState>,
    Path(project): Path<Uuid>,
    Query(f): Query<Filters>,
) -> Result<Json<traces::Volume>> {
    services::get_project(&state.db, project).await?;
    Ok(Json(traces::volume(&state, f.requests(project)?).await?))
}

pub async fn logs(
    _: Viewer,
    State(state): State<SharedState>,
    Path(project): Path<Uuid>,
    Query(f): Query<Filters>,
) -> Result<Json<Vec<logs::Line>>> {
    services::get_project(&state.db, project).await?;
    let level = match f.level.as_deref() {
        None | Some("") => None,
        Some("debug") => Some(logs::DEBUG),
        Some("info") => Some(logs::INFO),
        Some("warn") => Some(logs::WARN),
        Some("error") => Some(logs::ERROR),
        Some(_) => return Err(bad_request("level must be debug, info, warn or error")),
    };
    let (from, to) = f.window(24)?;
    Ok(Json(
        logs::search(
            &state,
            logs::Search {
                project,
                service: f.service,
                env: f.env()?,
                level,
                query: &f.q,
                from,
                to,
                before: f.before,
            },
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct OverviewQuery {
    service: Option<Uuid>,
    hours: Option<i32>,
}

pub async fn overview(
    _: Viewer,
    State(state): State<SharedState>,
    Path(project): Path<Uuid>,
    Query(q): Query<OverviewQuery>,
) -> Result<Json<traces::Overview>> {
    services::get_project(&state.db, project).await?;
    Ok(Json(
        traces::overview(&state, project, q.service, q.hours.unwrap_or(24)).await?,
    ))
}

#[derive(Serialize)]
pub struct Trace {
    spans: Vec<traces::Span>,
    logs: Vec<logs::Line>,
}

/// Every span of a trace, and the lines logged with its id.
pub async fn trace(
    _: Viewer,
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Trace>> {
    let id = id.to_ascii_lowercase();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad_request("a trace id is 32 hex digits"));
    }
    Ok(Json(Trace {
        spans: traces::trace(&state, &id).await?,
        logs: logs::for_trace(&state, &id).await?,
    }))
}
