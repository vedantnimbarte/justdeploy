//! Requests and traces. Traefik and apps send OpenTelemetry spans (OTLP over
//! HTTP, protobuf) to an internal port; Traefik's span for each request is the
//! request log, and app spans nest under it by trace id.

use crate::{docker, error::Result, names, state::SharedState};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{any_value::Value as Any, AnyValue, KeyValue},
};
use prost::Message;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

/// The OTLP/HTTP port, reached over `jd-mesh` and never published on the host.
pub const PORT: u16 = 4318;

/// Attributes kept per span, as JSON; more is cut.
const MAX_ATTRS: usize = 16 * 1024;

/// The OTLP receiver, served on its own port that is never published.
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/v1/traces", post(ingest))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(state)
}

async fn ingest(State(state): State<SharedState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let gzip = headers
        .get("content-encoding")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));
    let body = if gzip {
        let mut out = Vec::new();
        let mut decoder = async_compression::tokio::bufread::GzipDecoder::new(&body[..]);
        if decoder.read_to_end(&mut out).await.is_err() {
            return StatusCode::BAD_REQUEST;
        }
        Bytes::from(out)
    } else {
        body
    };
    let Ok(request) = ExportTraceServiceRequest::decode(body) else {
        return StatusCode::BAD_REQUEST;
    };
    let mut found = rows(&request, &routes::get());
    // A service deployed in the last half minute isn't in the routes yet.
    if found.iter().any(|r| r.deployment.is_none()) && routes::stale() {
        if let Err(error) = refresh_routes(state.clone()).await {
            tracing::warn!(%error, "routes not refreshed");
        }
        found = rows(&request, &routes::get());
    }
    if let Err(error) = store(&state, found).await {
        tracing::warn!(%error, "spans not stored");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::OK
}

/// Which deployment answers each hostname, as Traefik decides it: from the
/// router labels on every routed workload, the newest (highest priority) winning.
pub async fn refresh_routes(state: SharedState) -> Result<()> {
    let workloads = docker::workloads(&state.docker, &[]).await?;
    routes::set(hosts(&workloads));
    Ok(())
}

fn hosts(workloads: &[docker::Workload]) -> HashMap<String, Uuid> {
    let mut best: HashMap<String, (i64, Uuid)> = HashMap::new();
    for workload in workloads {
        let Some(deployment) = workload
            .label(docker::LABEL_DEPLOYMENT)
            .and_then(|d| d.parse::<Uuid>().ok())
        else {
            continue;
        };
        let router = names::router(deployment);
        let Some(rule) = workload.label(&format!("traefik.http.routers.{router}.rule")) else {
            continue;
        };
        let priority = workload
            .label(&format!("traefik.http.routers.{router}.priority"))
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        for host in rule.split("||").filter_map(|part| {
            part.trim()
                .strip_prefix("Host(`")?
                .strip_suffix("`)")
                .map(str::to_ascii_lowercase)
        }) {
            let entry = best.entry(host).or_insert((priority, deployment));
            if priority > entry.0 {
                *entry = (priority, deployment);
            }
        }
    }
    best.into_iter().map(|(host, (_, d))| (host, d)).collect()
}

mod routes {
    use std::{
        collections::HashMap,
        sync::{Arc, OnceLock, RwLock},
        time::{Duration, Instant},
    };
    use uuid::Uuid;

    type Routes = (Instant, Arc<HashMap<String, Uuid>>);

    fn slot() -> &'static RwLock<Option<Routes>> {
        static ROUTES: OnceLock<RwLock<Option<Routes>>> = OnceLock::new();
        ROUTES.get_or_init(Default::default)
    }

    pub fn get() -> Arc<HashMap<String, Uuid>> {
        slot()
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|(_, r)| r.clone())
            .unwrap_or_default()
    }

    /// Not refreshed in the last 10 seconds, so a miss may refresh it.
    pub fn stale() -> bool {
        slot()
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(10))
    }

    pub fn set(routes: HashMap<String, Uuid>) {
        *slot().write().unwrap_or_else(|p| p.into_inner()) =
            Some((Instant::now(), Arc::new(routes)));
    }
}

/// One span as stored. Its deployment comes from the app's resource
/// attributes, or for Traefik's spans from the requested host.
#[derive(Debug, PartialEq)]
pub struct Row {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    deployment: Option<Uuid>,
    source: &'static str,
    name: String,
    kind: i16,
    start_ts: DateTime<Utc>,
    duration_us: i64,
    status: i16,
    http_method: Option<String>,
    http_host: Option<String>,
    http_path: Option<String>,
    http_status: Option<i32>,
    client_ip: Option<String>,
    attrs: Value,
}

/// OpenTelemetry's SERVER span kind.
const SERVER: i32 = 2;

/// The spans worth keeping: every app span, and from Traefik only its entry
/// span per request (not its proxy span, nor its own calls to Docker).
fn rows(request: &ExportTraceServiceRequest, routes: &HashMap<String, Uuid>) -> Vec<Row> {
    let mut rows = Vec::new();
    for resource_spans in &request.resource_spans {
        let resource = resource_spans
            .resource
            .as_ref()
            .map(|r| r.attributes.as_slice())
            .unwrap_or_default();
        let edge = text(resource, &["service.name"]).as_deref() == Some("traefik");
        let app_deployment = text(resource, &["jd.deployment"]).and_then(|d| d.parse().ok());
        for span in resource_spans.scope_spans.iter().flat_map(|s| &s.spans) {
            let a = span.attributes.as_slice();
            if edge && (span.kind != SERVER || text(a, &["entry_point"]).is_none()) {
                continue;
            }
            let http_host = text(a, &["server.address", "http.host", "net.host.name"]);
            let deployment = if edge {
                http_host
                    .as_ref()
                    .and_then(|h| routes.get(&h.to_ascii_lowercase()).copied())
            } else {
                app_deployment
            };
            let start = span.start_time_unix_nano;
            let mut attrs: Map<String, Value> = a
                .iter()
                .map(|kv| (kv.key.clone(), json(kv.value.as_ref())))
                .collect();
            if !span.events.is_empty() {
                let events = span
                    .events
                    .iter()
                    .map(|e| {
                        let fields: Map<String, Value> = e
                            .attributes
                            .iter()
                            .map(|kv| (kv.key.clone(), json(kv.value.as_ref())))
                            .collect();
                        serde_json::json!({
                            "name": e.name,
                            "offset_us": e.time_unix_nano.saturating_sub(start) / 1000,
                            "attributes": fields,
                        })
                    })
                    .collect();
                attrs.insert("events".into(), Value::Array(events));
            }
            if let Some(message) = span
                .status
                .as_ref()
                .map(|s| &s.message)
                .filter(|m| !m.is_empty())
            {
                attrs.insert("status.message".into(), Value::String(message.clone()));
            }
            let mut attrs = Value::Object(attrs);
            if attrs.to_string().len() > MAX_ATTRS {
                attrs = serde_json::json!({ "truncated": true });
            }
            rows.push(Row {
                trace_id: hex::encode(&span.trace_id),
                span_id: hex::encode(&span.span_id),
                parent_span_id: (!span.parent_span_id.is_empty())
                    .then(|| hex::encode(&span.parent_span_id)),
                deployment,
                source: if edge { "edge" } else { "app" },
                name: span.name.clone(),
                kind: span.kind as i16,
                start_ts: DateTime::from_timestamp_nanos(start as i64),
                duration_us: (span.end_time_unix_nano.saturating_sub(start) / 1000) as i64,
                status: span.status.as_ref().map_or(0, |s| s.code as i16),
                http_method: text(a, &["http.request.method", "http.method"]),
                http_host,
                http_path: text(a, &["url.path", "http.target", "http.route"]),
                http_status: int(a, &["http.response.status_code", "http.status_code"])
                    .map(|s| s as i32),
                client_ip: text(a, &["client.address", "http.client_ip", "net.peer.ip"]),
                attrs,
            });
        }
    }
    rows
}

/// Stores rows whose deployment is known, with its service; others (the
/// dashboard, status pages, unknown senders) are dropped.
async fn store(state: &SharedState, rows: Vec<Row>) -> Result<()> {
    let deployments: Vec<Uuid> = rows.iter().filter_map(|r| r.deployment).collect();
    if deployments.is_empty() {
        return Ok(());
    }
    let services: HashMap<Uuid, Uuid> = sqlx::query!(
        "SELECT id, service_id FROM deployments WHERE id = ANY($1)",
        &deployments
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|r| (r.id, r.service_id))
    .collect();
    let rows: Vec<(Row, Uuid)> = rows
        .into_iter()
        .filter_map(|r| {
            let service = *services.get(&r.deployment?)?;
            Some((r, service))
        })
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    let mut c = Columns::default();
    for (r, service) in rows {
        c.trace_id.push(r.trace_id);
        c.span_id.push(r.span_id);
        c.parent.push(r.parent_span_id);
        c.deployment.push(r.deployment);
        c.service.push(service);
        c.source.push(r.source.to_string());
        c.name.push(r.name);
        c.kind.push(r.kind);
        c.start.push(r.start_ts);
        c.duration.push(r.duration_us);
        c.status.push(r.status);
        c.method.push(r.http_method);
        c.host.push(r.http_host);
        c.path.push(r.http_path);
        c.http_status.push(r.http_status);
        c.client.push(r.client_ip);
        c.attrs.push(r.attrs);
    }
    sqlx::query!(
        r#"INSERT INTO spans (trace_id, span_id, parent_span_id, deployment_id, service_id,
                              source, name, kind, start_ts, duration_us, status,
                              http_method, http_host, http_path, http_status, client_ip, attrs)
           SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::uuid[], $5::uuid[],
                                $6::text[], $7::text[], $8::int2[], $9::timestamptz[],
                                $10::int8[], $11::int2[], $12::text[], $13::text[],
                                $14::text[], $15::int4[], $16::text[], $17::jsonb[])"#,
        &c.trace_id,
        &c.span_id,
        &c.parent as &[Option<String>],
        &c.deployment as &[Option<Uuid>],
        &c.service,
        &c.source,
        &c.name,
        &c.kind,
        &c.start,
        &c.duration,
        &c.status,
        &c.method as &[Option<String>],
        &c.host as &[Option<String>],
        &c.path as &[Option<String>],
        &c.http_status as &[Option<i32>],
        &c.client as &[Option<String>],
        &c.attrs
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

#[derive(Default)]
struct Columns {
    trace_id: Vec<String>,
    span_id: Vec<String>,
    parent: Vec<Option<String>>,
    deployment: Vec<Option<Uuid>>,
    service: Vec<Uuid>,
    source: Vec<String>,
    name: Vec<String>,
    kind: Vec<i16>,
    start: Vec<DateTime<Utc>>,
    duration: Vec<i64>,
    status: Vec<i16>,
    method: Vec<Option<String>>,
    host: Vec<Option<String>>,
    path: Vec<Option<String>>,
    http_status: Vec<Option<i32>>,
    client: Vec<Option<String>>,
    attrs: Vec<Value>,
}

/// The first of `keys` present, as text.
fn text(attrs: &[KeyValue], keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = attrs.iter().find(|kv| kv.key == *key)?.value.as_ref()?;
        match value.value.as_ref()? {
            Any::StringValue(s) => Some(s.clone()),
            Any::IntValue(i) => Some(i.to_string()),
            _ => None,
        }
    })
}

fn int(attrs: &[KeyValue], keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = attrs.iter().find(|kv| kv.key == *key)?.value.as_ref()?;
        match value.value.as_ref()? {
            Any::IntValue(i) => Some(*i),
            Any::StringValue(s) => s.parse().ok(),
            _ => None,
        }
    })
}

fn json(value: Option<&AnyValue>) -> Value {
    match value.and_then(|v| v.value.as_ref()) {
        Some(Any::StringValue(s)) => Value::String(s.clone()),
        Some(Any::BoolValue(b)) => Value::Bool(*b),
        Some(Any::IntValue(i)) => Value::from(*i),
        Some(Any::DoubleValue(d)) => Value::from(*d),
        Some(Any::ArrayValue(a)) => Value::Array(a.values.iter().map(|v| json(Some(v))).collect()),
        Some(Any::KvlistValue(l)) => Value::Object(
            l.values
                .iter()
                .map(|kv| (kv.key.clone(), json(kv.value.as_ref())))
                .collect(),
        ),
        Some(Any::BytesValue(b)) => Value::String(hex::encode(b)),
        _ => Value::Null,
    }
}

pub async fn prune(state: &SharedState) -> Result<()> {
    sqlx::query!(
        "DELETE FROM spans WHERE start_ts < now() - make_interval(days => $1)",
        state.config.log_days
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// One request: Traefik's span for it.
#[derive(Serialize)]
pub struct Request {
    id: i64,
    trace_id: String,
    ts: DateTime<Utc>,
    duration_us: i64,
    method: Option<String>,
    host: Option<String>,
    path: Option<String>,
    status: Option<i32>,
    client_ip: Option<String>,
    service_id: Uuid,
    service: String,
    /// The pull request a preview served it, if one did.
    pr: Option<i32>,
}

/// What to search a project's requests for.
pub struct RequestSearch<'a> {
    pub project: Uuid,
    pub service: Option<Uuid>,
    /// `production` or `preview`.
    pub env: Option<&'a str>,
    /// The status class: 2 for 2xx up to 5 for 5xx.
    pub class: Option<i32>,
    pub method: Option<&'a str>,
    /// Text in the path or host.
    pub query: &'a str,
    /// The window: from (inclusive) to (exclusive).
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub before: Option<i64>,
}

/// The newest 200 matching requests; `before` is the last one of the previous page.
pub async fn requests(state: &SharedState, s: RequestSearch<'_>) -> Result<Vec<Request>> {
    let pattern = format!("%{}%", crate::logs::escape_like(s.query.trim()));
    Ok(sqlx::query_as!(
        Request,
        r#"SELECT sp.id, sp.trace_id, sp.start_ts AS ts, sp.duration_us,
                  sp.http_method AS method, sp.http_host AS host, sp.http_path AS path,
                  sp.http_status AS status, sp.client_ip,
                  sp.service_id AS "service_id!", sv.name AS service, d.pr_number AS "pr?"
           FROM spans sp
           JOIN services sv ON sv.id = sp.service_id
           LEFT JOIN deployments d ON d.id = sp.deployment_id
           WHERE sp.source = 'edge' AND sv.project_id = $1
             AND ($2::uuid IS NULL OR sp.service_id = $2)
             AND ($3::text IS NULL OR ($3 = 'preview') = (d.pr_number IS NOT NULL))
             AND ($4::int IS NULL OR sp.http_status / 100 = $4)
             AND ($5::text IS NULL OR sp.http_method = $5)
             AND ($6 = '%%' OR sp.http_path ILIKE $6 OR sp.http_host ILIKE $6)
             AND sp.start_ts >= $7 AND sp.start_ts < $8
             -- By when they came in, not when Traefik sent them: it sends in
             -- batches, and a slow request only once it ends.
             AND ($9::bigint IS NULL
                  OR (sp.start_ts, sp.id) < (SELECT start_ts, id FROM spans WHERE id = $9))
           ORDER BY sp.start_ts DESC, sp.id DESC LIMIT 200"#,
        s.project,
        s.service,
        s.env,
        s.class,
        s.method,
        pattern,
        s.from,
        s.to,
        s.before
    )
    .fetch_all(&state.db)
    .await?)
}

#[derive(Serialize)]
pub struct VolumeBucket {
    t: DateTime<Utc>,
    /// Requests by status class: below 300, 3xx, 4xx, 5xx.
    ok: i64,
    redirect: i64,
    client: i64,
    server: i64,
}

#[derive(Serialize)]
pub struct Volume {
    step_secs: i64,
    buckets: Vec<VolumeBucket>,
}

/// A round step that cuts `span_secs` into at most 60 bars.
fn volume_step(span_secs: i64) -> i64 {
    const STEPS: [i64; 17] = [
        1, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400,
    ];
    STEPS
        .into_iter()
        .find(|step| span_secs / step <= 60)
        .unwrap_or(86400)
}

/// How many requests came in over the window, step by step and by status
/// class, with the same filters as the list (all classes, to compare them).
pub async fn volume(state: &SharedState, s: RequestSearch<'_>) -> Result<Volume> {
    let pattern = format!("%{}%", crate::logs::escape_like(s.query.trim()));
    let step = volume_step((s.to - s.from).num_seconds());
    let buckets = sqlx::query_as!(
        VolumeBucket,
        r#"WITH b AS (
             SELECT generate_series(
               date_bin(make_interval(secs => $8), $6::timestamptz, TIMESTAMPTZ '2000-01-01'),
               $7::timestamptz - interval '1 microsecond', make_interval(secs => $8)) AS t
           ), r AS (
             SELECT date_bin(make_interval(secs => $8), sp.start_ts, TIMESTAMPTZ '2000-01-01') AS t,
                    count(*) FILTER (WHERE sp.http_status IS NULL OR sp.http_status < 300) AS ok,
                    count(*) FILTER (WHERE sp.http_status BETWEEN 300 AND 399) AS redirect,
                    count(*) FILTER (WHERE sp.http_status BETWEEN 400 AND 499) AS client,
                    count(*) FILTER (WHERE sp.http_status >= 500) AS server
             FROM spans sp
             JOIN services sv ON sv.id = sp.service_id
             LEFT JOIN deployments d ON d.id = sp.deployment_id
             WHERE sp.source = 'edge' AND sv.project_id = $1
               AND ($2::uuid IS NULL OR sp.service_id = $2)
               AND ($3::text IS NULL OR ($3 = 'preview') = (d.pr_number IS NOT NULL))
               AND ($4::text IS NULL OR sp.http_method = $4)
               AND ($5 = '%%' OR sp.http_path ILIKE $5 OR sp.http_host ILIKE $5)
               AND sp.start_ts >= $6 AND sp.start_ts < $7
             GROUP BY 1
           )
           SELECT b.t AS "t!", coalesce(r.ok, 0) AS "ok!", coalesce(r.redirect, 0) AS "redirect!",
                  coalesce(r.client, 0) AS "client!", coalesce(r.server, 0) AS "server!"
           FROM b LEFT JOIN r USING (t)
           ORDER BY b.t"#,
        s.project,
        s.service,
        s.env,
        s.method,
        pattern,
        s.from,
        s.to,
        step as f64
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Volume {
        step_secs: step,
        buckets,
    })
}

/// A span of a trace, for the waterfall.
#[derive(Serialize)]
pub struct Span {
    span_id: String,
    parent_span_id: Option<String>,
    source: String,
    name: String,
    kind: i16,
    start_ts: DateTime<Utc>,
    duration_us: i64,
    status: i16,
    http_status: Option<i32>,
    service: Option<String>,
    attrs: Value,
}

/// Every span of a trace, earliest first.
pub async fn trace(state: &SharedState, trace: &str) -> Result<Vec<Span>> {
    Ok(sqlx::query_as!(
        Span,
        r#"SELECT sp.span_id, sp.parent_span_id, sp.source, sp.name, sp.kind, sp.start_ts,
                  sp.duration_us, sp.status, sp.http_status, sv.name AS "service?", sp.attrs
           FROM spans sp LEFT JOIN services sv ON sv.id = sp.service_id
           WHERE sp.trace_id = $1
           ORDER BY sp.start_ts, sp.id LIMIT 2000"#,
        trace
    )
    .fetch_all(&state.db)
    .await?)
}

#[derive(Serialize)]
pub struct Bucket {
    t: DateTime<Utc>,
    requests: i64,
    /// Requests answered with a 5xx.
    errors: i64,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
    /// Lines logged at error level.
    error_logs: i64,
}

#[derive(Serialize)]
pub struct FailingRoute {
    method: Option<String>,
    path: Option<String>,
    service: String,
    errors: i64,
    last: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ErrorMessage {
    message: String,
    service: String,
    count: i64,
    last: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct Overview {
    /// Minutes per bucket.
    step: i32,
    buckets: Vec<Bucket>,
    failing: Vec<FailingRoute>,
    errors: Vec<ErrorMessage>,
}

/// Minutes per chart step for a range: about a hundred steps or fewer.
fn step(hours: i32) -> i32 {
    match hours {
        ..=1 => 1,
        2..=24 => 15,
        _ => hours * 60 / 84,
    }
}

/// Requests, 5xx answers, latency and error lines over the last `hours`, with
/// the routes and messages that fail most.
pub async fn overview(
    state: &SharedState,
    project: Uuid,
    service: Option<Uuid>,
    hours: i32,
) -> Result<Overview> {
    let hours = hours.clamp(1, 24 * 30);
    let step = step(hours);
    let buckets = sqlx::query_as!(
        Bucket,
        r#"WITH b AS (
             SELECT generate_series(
               date_bin(make_interval(mins => $3), now() - make_interval(hours => $2), TIMESTAMPTZ '2000-01-01'),
               now(), make_interval(mins => $3)) AS t
           ), r AS (
             SELECT date_bin(make_interval(mins => $3), sp.start_ts, TIMESTAMPTZ '2000-01-01') AS t,
                    count(*) AS n,
                    count(*) FILTER (WHERE sp.http_status >= 500) AS e,
                    percentile_cont(0.5) WITHIN GROUP (ORDER BY sp.duration_us) AS p50,
                    percentile_cont(0.95) WITHIN GROUP (ORDER BY sp.duration_us) AS p95
             FROM spans sp JOIN services sv ON sv.id = sp.service_id
             WHERE sp.source = 'edge' AND sv.project_id = $1
               AND ($4::uuid IS NULL OR sp.service_id = $4)
               AND sp.start_ts > now() - make_interval(hours => $2)
             GROUP BY 1
           ), l AS (
             SELECT date_bin(make_interval(mins => $3), l.ts, TIMESTAMPTZ '2000-01-01') AS t, count(*) AS n
             FROM log_lines l JOIN services sv ON sv.id = l.service_id
             WHERE l.level = 3 AND sv.project_id = $1
               AND ($4::uuid IS NULL OR l.service_id = $4)
               AND l.ts > now() - make_interval(hours => $2)
             GROUP BY 1
           )
           SELECT b.t AS "t!", coalesce(r.n, 0) AS "requests!", coalesce(r.e, 0) AS "errors!",
                  r.p50 / 1000.0 AS p50_ms, r.p95 / 1000.0 AS p95_ms,
                  coalesce(l.n, 0) AS "error_logs!"
           FROM b LEFT JOIN r USING (t) LEFT JOIN l USING (t)
           ORDER BY b.t"#,
        project,
        hours,
        step,
        service
    )
    .fetch_all(&state.db)
    .await?;
    let failing = sqlx::query_as!(
        FailingRoute,
        r#"SELECT sp.http_method AS method, sp.http_path AS path, sv.name AS service,
                  count(*) AS "errors!", max(sp.start_ts) AS "last!"
           FROM spans sp JOIN services sv ON sv.id = sp.service_id
           WHERE sp.source = 'edge' AND sp.http_status >= 500 AND sv.project_id = $1
             AND ($2::uuid IS NULL OR sp.service_id = $2)
             AND sp.start_ts > now() - make_interval(hours => $3)
           GROUP BY 1, 2, 3 ORDER BY 4 DESC LIMIT 10"#,
        project,
        service,
        hours
    )
    .fetch_all(&state.db)
    .await?;
    // ponytail: groups by the exact message, so lines with ids or times in
    // their text don't group. Normalise digits out if that's noisy.
    let errors = sqlx::query_as!(
        ErrorMessage,
        r#"SELECT left(coalesce(l.fields->>'msg', l.fields->>'message', l.line), 300) AS "message!",
                  sv.name AS service, count(*) AS "count!", max(l.ts) AS "last!"
           FROM log_lines l JOIN services sv ON sv.id = l.service_id
           WHERE l.level = 3 AND sv.project_id = $1
             AND ($2::uuid IS NULL OR l.service_id = $2)
             AND l.ts > now() - make_interval(hours => $3)
           GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 10"#,
        project,
        service,
        hours
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Overview {
        step,
        buckets,
        failing,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        resource::v1::Resource,
        trace::v1::{ResourceSpans, ScopeSpans, Span as Proto, Status},
    };

    fn kv(key: &str, value: Any) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        }
    }

    fn s(text: &str) -> Any {
        Any::StringValue(text.into())
    }

    fn export(resource: Vec<KeyValue>, spans: Vec<Proto>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: resource,
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    fn span(kind: i32, parent: &[u8], attributes: Vec<KeyValue>) -> Proto {
        Proto {
            trace_id: vec![0xab; 16],
            span_id: vec![0x01; 8],
            parent_span_id: parent.to_vec(),
            name: "GET".into(),
            kind,
            start_time_unix_nano: 1_794_997_966_000_000_000,
            end_time_unix_nano: 1_794_997_966_250_000_000,
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn traefik_keeps_only_its_entry_span_attributed_by_host() {
        let deployment = Uuid::new_v4();
        let routes = HashMap::from([("api-shop.example.com".to_string(), deployment)]);
        let request = export(
            vec![kv("service.name", s("traefik"))],
            vec![
                // The request as it entered.
                span(
                    SERVER,
                    &[],
                    vec![
                        kv("entry_point", s("web")),
                        kv("http.request.method", s("POST")),
                        kv("url.path", s("/orders")),
                        kv("server.address", s("API-shop.example.com")),
                        kv("client.address", s("203.0.113.9")),
                        kv("http.response.status_code", Any::IntValue(502)),
                    ],
                ),
                // Its proxy span, and Traefik polling Docker: both dropped.
                span(3, &[0x01; 8], vec![kv("server.address", s("10.0.0.5"))]),
                span(
                    3,
                    &[],
                    vec![kv("server.address", s("/var/run/docker.sock"))],
                ),
            ],
        );
        let rows = rows(&request, &routes);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.source, "edge");
        assert_eq!(r.deployment, Some(deployment));
        assert_eq!(r.trace_id, "ab".repeat(16));
        assert_eq!(r.parent_span_id, None);
        assert_eq!(r.duration_us, 250_000);
        assert_eq!(r.http_method.as_deref(), Some("POST"));
        assert_eq!(r.http_path.as_deref(), Some("/orders"));
        assert_eq!(r.http_status, Some(502));
        assert_eq!(r.client_ip.as_deref(), Some("203.0.113.9"));

        // A host no deployment serves stays unattributed (and isn't stored).
        assert_eq!(super::rows(&request, &HashMap::new())[0].deployment, None);
    }

    #[test]
    fn app_spans_come_from_their_resource_and_keep_events_and_errors() {
        let deployment = Uuid::new_v4();
        let mut db = span(3, &[0x02; 8], vec![kv("db.system", s("postgresql"))]);
        db.status = Some(Status {
            code: 2,
            message: "connection refused".into(),
        });
        db.events = vec![opentelemetry_proto::tonic::trace::v1::span::Event {
            name: "exception".into(),
            time_unix_nano: db.start_time_unix_nano + 1_000,
            ..Default::default()
        }];
        let request = export(
            vec![
                kv("service.name", s("api")),
                kv("jd.deployment", s(&deployment.to_string())),
            ],
            vec![db],
        );
        let rows = rows(&request, &HashMap::new());
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            (r.source, r.deployment, r.status),
            ("app", Some(deployment), 2)
        );
        assert_eq!(r.parent_span_id, Some("02".repeat(8)));
        assert_eq!(r.attrs["db.system"], "postgresql");
        assert_eq!(r.attrs["status.message"], "connection refused");
        assert_eq!(r.attrs["events"][0]["name"], "exception");
        assert_eq!(r.attrs["events"][0]["offset_us"], 1);
    }

    #[test]
    fn exports_decode_from_protobuf() {
        let request = export(
            vec![kv("service.name", s("api"))],
            vec![span(2, &[], vec![])],
        );
        let decoded = ExportTraceServiceRequest::decode(Bytes::from(request.encode_to_vec()));
        assert_eq!(decoded.unwrap(), request);
    }

    #[test]
    fn each_host_goes_to_its_newest_deployment() {
        let (old, new, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let workload = |deployment: Uuid, rule: &str, priority: &str| {
            let router = names::router(deployment);
            docker::Workload {
                name: deployment.to_string(),
                labels: HashMap::from([
                    (docker::LABEL_DEPLOYMENT.to_string(), deployment.to_string()),
                    (
                        format!("traefik.http.routers.{router}.rule"),
                        rule.to_string(),
                    ),
                    (
                        format!("traefik.http.routers.{router}.priority"),
                        priority.to_string(),
                    ),
                ]),
                running: 1,
                desired: 1,
            }
        };
        let hosts = hosts(&[
            workload(
                new,
                "Host(`api.example.com`) || Host(`Shop.Example.com`)",
                "200",
            ),
            workload(old, "Host(`api.example.com`)", "100"),
            workload(other, "Host(`pr-4-api.example.com`)", "150"),
        ]);
        assert_eq!(hosts["api.example.com"], new);
        assert_eq!(hosts["shop.example.com"], new);
        assert_eq!(hosts["pr-4-api.example.com"], other);
        assert_eq!(hosts.len(), 3);
    }

    #[test]
    fn volume_bars_are_round_and_at_most_sixty() {
        assert_eq!(volume_step(3600), 60);
        assert_eq!(volume_step(60), 1);
        assert_eq!(volume_step(24 * 3600), 1800);
        assert_eq!(volume_step(7 * 24 * 3600), 10800);
        assert_eq!(volume_step(365 * 24 * 3600), 86400);
    }

    #[test]
    fn chart_steps_stay_near_a_hundred() {
        assert_eq!(step(1), 1);
        assert_eq!(step(24), 15);
        assert_eq!(24 * 7 * 60 / step(24 * 7), 84);
    }
}
