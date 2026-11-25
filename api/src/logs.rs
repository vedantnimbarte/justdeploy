//! Kept logs. A follower per production or preview workload copies every line
//! its copies print into Postgres, kept `JD_LOG_DAYS` days and searchable by
//! text, level and service. JSON lines keep their fields, and a line that names
//! a trace links to the request that printed it.

use crate::{
    docker::{self, LABEL_DEPLOYMENT, LABEL_KIND, LABEL_SERVICE},
    error::Result,
    state::SharedState,
};
use bollard::query_parameters::LogsOptionsBuilder;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Longer lines are cut, so one runaway line can't fill the table.
const MAX_LINE: usize = 8 * 1024;
const BATCH: usize = 500;

pub const DEBUG: i16 = 0;
pub const INFO: i16 = 1;
pub const WARN: i16 = 2;
pub const ERROR: i16 = 3;

fn followers() -> &'static Mutex<HashMap<String, JoinHandle<()>>> {
    static FOLLOWERS: OnceLock<Mutex<HashMap<String, JoinHandle<()>>>> = OnceLock::new();
    FOLLOWERS.get_or_init(Default::default)
}

/// Every 30 seconds: a follower for each production and preview workload,
/// none for gone ones.
pub async fn collect(state: SharedState) -> Result<()> {
    let workloads: Vec<_> = docker::workloads(&state.docker, &[])
        .await?
        .into_iter()
        .filter(|w| matches!(w.label(LABEL_KIND), Some("production" | "preview")))
        .collect();
    let mut followers = followers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    followers.retain(|name, handle| {
        let wanted = workloads.iter().any(|w| &w.name == name);
        if !wanted {
            handle.abort();
        }
        wanted && !handle.is_finished()
    });
    for workload in workloads {
        let id = |label| workload.label(label).and_then(|s| s.parse::<Uuid>().ok());
        let (Some(service), Some(deployment)) = (id(LABEL_SERVICE), id(LABEL_DEPLOYMENT)) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = followers.entry(workload.name) {
            let task = tokio::spawn(follow(
                state.clone(),
                slot.key().clone(),
                service,
                deployment,
            ));
            slot.insert(task);
        }
    }
    Ok(())
}

/// Copies one workload's output until it ends. After a restart of justdeploy it
/// picks up where the stored lines stop.
async fn follow(state: SharedState, workload: String, service: Uuid, deployment: Uuid) {
    // Lines stored before deployments were recorded count as this one's.
    let since = sqlx::query_scalar!(
        "SELECT max(ts) FROM log_lines
         WHERE service_id = $1 AND (deployment_id = $2 OR deployment_id IS NULL)",
        service,
        deployment
    )
    .fetch_one(&state.db)
    .await
    .ok()
    .flatten();
    let options = LogsOptionsBuilder::new()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .since(since.map_or(0, |t| t.timestamp()) as i32)
        .build();
    let mut stream = state.docker.service_logs(&workload, Some(options));
    let mut splitter = docker::LineSplitter::default();
    let mut batch: Vec<(DateTime<Utc>, String)> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            chunk = stream.next() => match chunk {
                Some(Ok(chunk)) => {
                    for line in splitter.push(&docker::output_bytes(&chunk)) {
                        if let Some((ts, text)) = parse(&line) {
                            // `since` is in whole seconds: skip what's already stored.
                            if since.is_none_or(|s| ts > s) {
                                batch.push((ts, text));
                            }
                        }
                    }
                    if batch.len() >= BATCH {
                        store(&state, service, deployment, &mut batch).await;
                    }
                }
                _ => break,
            },
            _ = tick.tick() => store(&state, service, deployment, &mut batch).await,
        }
    }
    store(&state, service, deployment, &mut batch).await;
}

/// `2026-11-18T10:32:46.123456789Z text` into its time and text.
fn parse(line: &str) -> Option<(DateTime<Utc>, String)> {
    let (stamp, text) = line.split_once(' ').unwrap_or((line, ""));
    let ts = DateTime::parse_from_rfc3339(stamp)
        .ok()?
        .with_timezone(&Utc);
    let mut text = text.to_string();
    if text.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some((ts, text))
}

/// A line's level, the trace it names, and its fields when it's a JSON object.
/// Plain text is info unless it starts with a level (`ERROR`, `[warn]`,
/// `error:`) or carries `level=…`.
pub fn classify(text: &str) -> (i16, Option<String>, Option<Value>) {
    let text = text.trim();
    if text.starts_with('{') {
        if let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(text) {
            let level = [
                "level",
                "severity",
                "lvl",
                "log.level",
                "levelname",
                "severity_text",
            ]
            .iter()
            .find_map(|key| match fields.get(*key)? {
                // pino and bunyan: 10 trace, 20 debug, 30 info, 40 warn, 50 error, 60 fatal
                Value::Number(n) => n.as_i64().map(|n| match n {
                    ..=20 => DEBUG,
                    21..=30 => INFO,
                    31..=40 => WARN,
                    _ => ERROR,
                }),
                Value::String(s) => word_level(s),
                _ => None,
            })
            .unwrap_or(INFO);
            let trace = ["trace_id", "traceId", "trace.id", "otelTraceID", "traceid"]
                .iter()
                .find_map(|key| trace_id(fields.get(*key)?.as_str()?));
            return (level, trace, Some(Value::Object(fields)));
        }
    }
    let mut level = None;
    let mut trace = None;
    for (i, token) in text.split_whitespace().take(40).enumerate() {
        if let Some((key, value)) = token.split_once('=') {
            let value = value.trim_matches('"');
            match key {
                "level" | "lvl" | "severity" => level = level.or_else(|| word_level(value)),
                "trace_id" | "traceId" | "trace.id" => trace = trace.or_else(|| trace_id(value)),
                _ => {}
            }
        } else if i < 4 && level.is_none() {
            // `ERROR`, `[warn]`, `error:`: a level word set apart, near the start.
            let marked = token.starts_with('[')
                || token.ends_with(':')
                || token.chars().all(|c| !c.is_lowercase());
            let word = token.trim_matches(|c: char| !c.is_alphanumeric());
            if marked {
                level = word_level(word);
            }
        }
    }
    (level.unwrap_or(INFO), trace, None)
}

fn word_level(word: &str) -> Option<i16> {
    match word.to_ascii_lowercase().as_str() {
        "trace" | "debug" | "verbose" | "silly" => Some(DEBUG),
        "info" | "information" | "notice" => Some(INFO),
        "warn" | "warning" => Some(WARN),
        "error" | "err" | "fatal" | "critical" | "crit" | "panic" | "alert" | "emergency" => {
            Some(ERROR)
        }
        _ => None,
    }
}

/// A W3C trace id: 32 hex digits, not all zero.
fn trace_id(text: &str) -> Option<String> {
    (text.len() == 32
        && text.bytes().all(|b| b.is_ascii_hexdigit())
        && text.bytes().any(|b| b != b'0'))
    .then(|| text.to_ascii_lowercase())
}

async fn store(
    state: &SharedState,
    service: Uuid,
    deployment: Uuid,
    batch: &mut Vec<(DateTime<Utc>, String)>,
) {
    if batch.is_empty() {
        return;
    }
    let (mut times, mut lines, mut levels, mut traces, mut fields) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (ts, line) in std::mem::take(batch) {
        let (level, trace, json) = classify(&line);
        times.push(ts);
        lines.push(line);
        levels.push(level);
        traces.push(trace);
        fields.push(json);
    }
    let result = sqlx::query!(
        "INSERT INTO log_lines (service_id, deployment_id, ts, line, level, trace_id, fields)
         SELECT $1, $2, * FROM UNNEST($3::timestamptz[], $4::text[], $5::int2[], $6::text[], $7::jsonb[])",
        service,
        deployment,
        &times,
        &lines,
        &levels,
        &traces as &[Option<String>],
        &fields as &[Option<Value>]
    )
    .execute(&state.db)
    .await;
    if let Err(error) = result {
        tracing::warn!(%service, %error, "log lines not stored");
    }
}

#[derive(Serialize)]
pub struct Line {
    id: i64,
    ts: DateTime<Utc>,
    line: String,
    level: i16,
    trace_id: Option<String>,
    fields: Option<Value>,
    service_id: Uuid,
    service: String,
}

/// What to search a project's kept lines for.
pub struct Search<'a> {
    pub project: Uuid,
    pub service: Option<Uuid>,
    /// `production` or `preview`.
    pub env: Option<&'a str>,
    pub level: Option<i16>,
    pub query: &'a str,
    /// The window: from (inclusive) to (exclusive).
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// Older than this line, when paging back.
    pub before: Option<i64>,
}

/// The newest 200 matching lines (text in any case).
pub async fn search(state: &SharedState, s: Search<'_>) -> Result<Vec<Line>> {
    let pattern = format!("%{}%", escape_like(s.query.trim()));
    Ok(sqlx::query_as!(
        Line,
        r#"SELECT l.id, l.ts, l.line, l.level, l.trace_id, l.fields,
                  l.service_id AS "service_id!", sv.name AS service
           FROM log_lines l
           JOIN services sv ON sv.id = l.service_id
           LEFT JOIN deployments d ON d.id = l.deployment_id
           WHERE sv.project_id = $1
             AND ($2::uuid IS NULL OR l.service_id = $2)
             AND ($3::text IS NULL OR ($3 = 'preview') = (d.pr_number IS NOT NULL))
             AND ($4::int2 IS NULL OR l.level = $4)
             AND l.ts >= $5 AND l.ts < $6
             AND l.line ILIKE $7
             AND ($8::bigint IS NULL OR l.id < $8)
           ORDER BY l.id DESC LIMIT 200"#,
        s.project,
        s.service,
        s.env,
        s.level,
        s.from,
        s.to,
        pattern,
        s.before
    )
    .fetch_all(&state.db)
    .await?)
}

/// Lines that name a trace, oldest first.
pub async fn for_trace(state: &SharedState, trace: &str) -> Result<Vec<Line>> {
    Ok(sqlx::query_as!(
        Line,
        r#"SELECT l.id, l.ts, l.line, l.level, l.trace_id, l.fields,
                  l.service_id AS "service_id!", sv.name AS service
           FROM log_lines l JOIN services sv ON sv.id = l.service_id
           WHERE l.trace_id = $1
           ORDER BY l.ts, l.id LIMIT 500"#,
        trace
    )
    .fetch_all(&state.db)
    .await?)
}

/// Matches the text literally: `%` and `_` in a search mean themselves.
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub async fn prune(state: &SharedState) -> Result<()> {
    sqlx::query!(
        "DELETE FROM log_lines WHERE ts < now() - make_interval(days => $1)",
        state.config.log_days
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_into_time_and_text() {
        let (ts, text) = parse("2026-11-18T10:32:46.123456789Z GET / 200").unwrap();
        assert_eq!(ts.timestamp(), 1_794_997_966);
        assert_eq!(text, "GET / 200");
        assert!(parse("no timestamp here").is_none());
        let (_, long) = parse(&format!("2026-11-18T10:32:46Z {}", "é".repeat(MAX_LINE))).unwrap();
        assert!(long.len() <= MAX_LINE);
    }

    #[test]
    fn search_text_is_literal() {
        assert_eq!(escape_like("100%_done\\"), "100\\%\\_done\\\\");
    }

    const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    #[test]
    fn json_lines_keep_their_level_trace_and_fields() {
        let (level, trace, fields) = classify(&format!(
            r#"{{"level":50,"msg":"boom","trace_id":"{}"}}"#,
            TRACE.to_uppercase()
        ));
        assert_eq!((level, trace.as_deref()), (ERROR, Some(TRACE)));
        assert_eq!(fields.unwrap()["msg"], "boom");
        assert_eq!(classify(r#"{"level":30}"#).0, INFO);
        assert_eq!(classify(r#"{"level":20}"#).0, DEBUG);
        assert_eq!(classify(r#"{"severity":"WARNING"}"#).0, WARN);
        assert_eq!(classify(r#"{"levelname":"CRITICAL"}"#).0, ERROR);
        assert_eq!(classify(r#"{"msg":"no level"}"#).0, INFO);
        // Not a trace id: wrong length, or all zeros.
        assert_eq!(classify(r#"{"traceId":"abc"}"#).1, None);
        assert_eq!(
            classify(&format!(r#"{{"traceId":"{}"}}"#, "0".repeat(32))).1,
            None
        );
    }

    #[test]
    fn plain_lines_get_a_level_only_when_one_is_marked() {
        assert_eq!(classify("ERROR: Exception in ASGI application").0, ERROR);
        assert_eq!(
            classify("INFO:     127.0.0.1:5000 - \"GET / HTTP/1.1\" 200").0,
            INFO
        );
        assert_eq!(classify("[warn] disk almost full").0, WARN);
        assert_eq!(classify("2026-11-25 12:00:01 WARN cache cold").0, WARN);
        assert_eq!(classify("error: could not connect").0, ERROR);
        let (level, trace, fields) = classify(&format!(
            "time=now level=error msg=\"boom\" trace_id={TRACE}"
        ));
        assert_eq!(
            (level, trace.as_deref(), fields),
            (ERROR, Some(TRACE), None)
        );
        // "error" in passing isn't a level.
        assert_eq!(classify("GET /error 200").0, INFO);
        assert_eq!(classify("starting the error reporter").0, INFO);
        assert_eq!(classify("").0, INFO);
    }
}
