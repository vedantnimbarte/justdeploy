//! Kept logs. A follower per production workload copies every line its copies
//! print into Postgres, kept `JD_LOG_DAYS` days and searchable by text.

use crate::{
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::Result,
    state::SharedState,
};
use bollard::query_parameters::LogsOptionsBuilder;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Serialize;
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

fn followers() -> &'static Mutex<HashMap<String, JoinHandle<()>>> {
    static FOLLOWERS: OnceLock<Mutex<HashMap<String, JoinHandle<()>>>> = OnceLock::new();
    FOLLOWERS.get_or_init(Default::default)
}

/// Every 30 seconds: a follower for each production workload, none for gone ones.
pub async fn collect(state: SharedState) -> Result<()> {
    let workloads = docker::workloads(&state.docker, &[(LABEL_KIND, "production")]).await?;
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
        let Some(service) = workload
            .label(LABEL_SERVICE)
            .and_then(|s| s.parse::<Uuid>().ok())
        else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = followers.entry(workload.name) {
            let task = tokio::spawn(follow(state.clone(), slot.key().clone(), service));
            slot.insert(task);
        }
    }
    Ok(())
}

/// Copies one workload's output until it ends. After a restart of justdeploy it
/// picks up where the stored lines stop.
async fn follow(state: SharedState, workload: String, service: Uuid) {
    let since = sqlx::query_scalar!(
        "SELECT max(ts) FROM log_lines WHERE service_id = $1",
        service
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
                        store(&state, service, &mut batch).await;
                    }
                }
                _ => break,
            },
            _ = tick.tick() => store(&state, service, &mut batch).await,
        }
    }
    store(&state, service, &mut batch).await;
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

async fn store(state: &SharedState, service: Uuid, batch: &mut Vec<(DateTime<Utc>, String)>) {
    if batch.is_empty() {
        return;
    }
    let (times, lines): (Vec<DateTime<Utc>>, Vec<String>) =
        std::mem::take(batch).into_iter().unzip();
    let result = sqlx::query!(
        "INSERT INTO log_lines (service_id, ts, line)
         SELECT $1, * FROM UNNEST($2::timestamptz[], $3::text[])",
        service,
        &times,
        &lines
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
}

/// The newest 200 lines containing `query` (any case) in the last `hours`,
/// older than `before` when paging back.
pub async fn search(
    state: &SharedState,
    service: Uuid,
    query: &str,
    hours: i32,
    before: Option<i64>,
) -> Result<Vec<Line>> {
    let pattern = format!("%{}%", escape_like(query.trim()));
    Ok(sqlx::query_as!(
        Line,
        r#"SELECT id, ts, line FROM log_lines
           WHERE service_id = $1 AND ts > now() - make_interval(hours => $2)
             AND line ILIKE $3 AND ($4::bigint IS NULL OR id < $4)
           ORDER BY id DESC LIMIT 200"#,
        service,
        hours.clamp(1, 24 * 365),
        pattern,
        before
    )
    .fetch_all(&state.db)
    .await?)
}

/// Matches the text literally: `%` and `_` in a search mean themselves.
fn escape_like(text: &str) -> String {
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
}
