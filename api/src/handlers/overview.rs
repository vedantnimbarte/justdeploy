//! The system overview on the home page: the server, all apps together, platform
//! activity, Docker's disk usage, and anything that needs attention.

use super::projects::{copies, latest_deployment, status};
use crate::{
    auth::Viewer,
    cluster,
    error::Result,
    host::{self, DockerDisk},
    models::Service,
    state::SharedState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use bytes_fmt::human;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Points per chart, whatever the range; longer ranges are averaged into buckets.
const POINTS: i32 = 240;
const DISK_WARN: f64 = 0.85;
const DISK_CRITICAL: f64 = 0.95;
const MEMORY_WARN: f64 = 0.90;

#[derive(Serialize)]
pub struct Server {
    sampled_at: DateTime<Utc>,
    cpu_percent: f64,
    cpu_cores: i32,
    memory_used: i64,
    memory_total: i64,
    disk_used: i64,
    disk_total: i64,
    net_rx_rate: f64,
    net_tx_rate: f64,
    uptime_seconds: i64,
}

#[derive(Serialize)]
pub struct Point {
    ts: DateTime<Utc>,
    /// Whole server, percent of all cores.
    cpu_percent: f64,
    /// All deployed services together, on the same scale.
    apps_cpu_percent: Option<f64>,
    memory_used: i64,
    apps_memory: Option<i64>,
    /// None for the first point, which has no previous counter to diff against.
    net_rx_rate: Option<f64>,
    net_tx_rate: Option<f64>,
}

#[derive(Serialize)]
pub struct TopService {
    id: Uuid,
    name: String,
    project: String,
    cpu_percent: f64,
    memory_bytes: i64,
}

#[derive(Serialize, Default)]
pub struct Platform {
    projects: i64,
    services: i64,
    running: i64,
    deploying: i64,
    crashed: i64,
    failed: i64,
    stopped: i64,
    deploys_today: i64,
    deploys_failed_today: i64,
    /// Successful builds over the last 7 days, 0-100; none when nothing finished.
    build_success_rate: Option<f64>,
    last_backup: Option<LastBackup>,
    failed_backups_24h: i64,
}

#[derive(Serialize)]
pub struct LastBackup {
    status: String,
    created_at: DateTime<Utc>,
    service: String,
}

#[derive(Serialize)]
pub struct Warning {
    /// critical | warning
    level: &'static str,
    text: String,
    link: Option<String>,
}

#[derive(Serialize)]
pub struct Overview {
    server: Option<Server>,
    series: Vec<Point>,
    top_services: Vec<TopService>,
    platform: Platform,
    docker_disk: Option<DockerDisk>,
    warnings: Vec<Warning>,
}

#[derive(Deserialize)]
pub struct Range {
    minutes: Option<i32>,
}

pub async fn overview(
    _: Viewer,
    State(state): State<SharedState>,
    Query(range): Query<Range>,
) -> Result<Json<Overview>> {
    let minutes = range.minutes.unwrap_or(60).clamp(5, 24 * 60);
    let server = server(&state).await?;
    let cores = server.as_ref().map_or(1, |s| s.cpu_cores.max(1));
    let series = series(&state, minutes, cores).await?;
    let top_services = top_services(&state, cores).await?;
    let (platform, crashed) = platform(&state).await?;
    let docker_disk = host::docker_disk();

    let mut warnings = Vec::new();
    if let Some(s) = &server {
        let disk = s.disk_used as f64 / s.disk_total.max(1) as f64;
        if disk >= DISK_WARN {
            warnings.push(Warning {
                level: if disk >= DISK_CRITICAL { "critical" } else { "warning" },
                text: format!(
                    "The disk is {:.0}% full, {} left. Old images and build cache are usually the first to clear.",
                    disk * 100.0,
                    human((s.disk_total - s.disk_used).max(0))
                ),
                link: None,
            });
        }
        let memory = s.memory_used as f64 / s.memory_total.max(1) as f64;
        if memory >= MEMORY_WARN {
            warnings.push(Warning {
                level: "warning",
                text: format!(
                    "Memory is {:.0}% used. Set memory limits on the heaviest services.",
                    memory * 100.0
                ),
                link: None,
            });
        }
    }
    for (id, service, project) in crashed {
        warnings.push(Warning {
            level: "critical",
            text: format!("{service} in {project} has crashed."),
            link: Some(format!("/services/{id}?tab=logs")),
        });
    }
    let failed_backups = sqlx::query!(
        r#"SELECT DISTINCT ON (b.service_id) s.id AS "service_id!", s.name AS "service!", p.name AS "project!",
                  b.error
           FROM backups b JOIN services s ON s.id = b.service_id JOIN projects p ON p.id = s.project_id
           WHERE b.status = 'failed' AND b.created_at > now() - interval '24 hours'
           ORDER BY b.service_id, b.created_at DESC"#
    )
    .fetch_all(&state.db)
    .await?;
    for b in failed_backups {
        warnings.push(Warning {
            level: "critical",
            text: format!(
                "A backup of {} in {} failed{}",
                b.service,
                b.project,
                b.error
                    .map(|e| format!(": {e}"))
                    .unwrap_or_else(|| ".".into())
            ),
            link: Some(format!("/services/{}?tab=backups", b.service_id)),
        });
    }

    Ok(Json(Overview {
        server,
        series,
        top_services,
        platform,
        docker_disk,
        warnings,
    }))
}

async fn server(state: &SharedState) -> Result<Option<Server>> {
    // The server justdeploy runs on; the Servers page shows the others.
    let rows = sqlx::query!(
        "SELECT * FROM host_samples WHERE node_id = $1 ORDER BY ts DESC LIMIT 2",
        cluster::self_node()
    )
    .fetch_all(&state.db)
    .await?;
    let Some(last) = rows.first() else {
        return Ok(None);
    };
    let (rx_rate, tx_rate) = match rows.get(1) {
        Some(prev) => {
            let secs = (last.ts - prev.ts).num_milliseconds().max(1) as f64 / 1000.0;
            (
                (last.net_rx_bytes - prev.net_rx_bytes).max(0) as f64 / secs,
                (last.net_tx_bytes - prev.net_tx_bytes).max(0) as f64 / secs,
            )
        }
        None => (0.0, 0.0),
    };
    Ok(Some(Server {
        sampled_at: last.ts,
        cpu_percent: last.cpu_percent,
        cpu_cores: last.cpu_cores,
        memory_used: last.memory_used,
        memory_total: last.memory_total,
        disk_used: last.disk_used,
        disk_total: last.disk_total,
        net_rx_rate: rx_rate,
        net_tx_rate: tx_rate,
        uptime_seconds: last.uptime_seconds,
    }))
}

/// Server and apps-combined samples, averaged into at most POINTS buckets.
async fn series(state: &SharedState, minutes: i32, cores: i32) -> Result<Vec<Point>> {
    let bucket = ((minutes * 60) / POINTS).max(30);
    let rows = sqlx::query!(
        r#"
        WITH h AS (
            SELECT date_bin(make_interval(secs => $2), ts, TIMESTAMPTZ '2000-01-01') AS b,
                   avg(cpu_percent) AS cpu, avg(memory_used)::bigint AS mem,
                   max(net_rx_bytes) AS rx, max(net_tx_bytes) AS tx
            FROM host_samples WHERE ts > now() - make_interval(mins => $1) AND node_id = $3
            GROUP BY 1
        ), per_sample AS (
            SELECT ts, sum(cpu_percent) AS cpu, sum(memory_bytes) AS mem
            FROM metric_samples WHERE ts > now() - make_interval(mins => $1)
            GROUP BY ts
        ), a AS (
            SELECT date_bin(make_interval(secs => $2), ts, TIMESTAMPTZ '2000-01-01') AS b,
                   avg(cpu) AS cpu, avg(mem)::bigint AS mem
            FROM per_sample GROUP BY 1
        )
        SELECT h.b AS "ts!", h.cpu AS "cpu!", h.mem AS "mem!", h.rx AS "rx!", h.tx AS "tx!",
               a.cpu AS apps_cpu, a.mem AS apps_mem
        FROM h LEFT JOIN a ON a.b = h.b
        ORDER BY h.b
        "#,
        minutes,
        bucket as f64,
        cluster::self_node()
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    let mut previous: Option<(DateTime<Utc>, i64, i64)> = None;
    for r in rows {
        let (rx_rate, tx_rate) = match previous {
            // A reboot resets the counters; show zero rather than a negative spike.
            Some((ts, rx, tx)) if r.rx >= rx && r.tx >= tx => {
                let secs = (r.ts - ts).num_milliseconds().max(1) as f64 / 1000.0;
                (
                    Some((r.rx - rx) as f64 / secs),
                    Some((r.tx - tx) as f64 / secs),
                )
            }
            Some(_) => (Some(0.0), Some(0.0)),
            None => (None, None),
        };
        previous = Some((r.ts, r.rx, r.tx));
        out.push(Point {
            ts: r.ts,
            cpu_percent: r.cpu,
            // Docker reports 100% per core; the server line is percent of all cores.
            apps_cpu_percent: r.apps_cpu.map(|c| c / cores as f64),
            memory_used: r.mem,
            apps_memory: r.apps_mem,
            net_rx_rate: rx_rate,
            net_tx_rate: tx_rate,
        });
    }
    Ok(out)
}

async fn top_services(state: &SharedState, cores: i32) -> Result<Vec<TopService>> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (m.service_id) m.service_id, s.name, p.name AS project,
               m.cpu_percent, m.memory_bytes
        FROM metric_samples m
        JOIN services s ON s.id = m.service_id
        JOIN projects p ON p.id = s.project_id
        WHERE m.ts > now() - interval '2 minutes'
        ORDER BY m.service_id, m.ts DESC
        "#
    )
    .fetch_all(&state.db)
    .await?;
    let mut top: Vec<TopService> = rows
        .into_iter()
        .map(|r| TopService {
            id: r.service_id,
            name: r.name,
            project: r.project,
            cpu_percent: r.cpu_percent / cores as f64,
            memory_bytes: r.memory_bytes,
        })
        .collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.memory_bytes));
    top.truncate(5);
    Ok(top)
}

/// Platform counts, and the crashed services as (id, service, project).
async fn platform(state: &SharedState) -> Result<(Platform, Vec<(Uuid, String, String)>)> {
    let mut p = Platform {
        projects: sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM projects"#)
            .fetch_one(&state.db)
            .await?,
        ..Default::default()
    };

    let services = sqlx::query_as!(Service, "SELECT * FROM services")
        .fetch_all(&state.db)
        .await?;
    let projects: std::collections::HashMap<Uuid, String> =
        sqlx::query!("SELECT id, name FROM projects")
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect();
    let copies = copies(state).await?;
    let mut crashed = Vec::new();
    for service in services {
        let mine = copies.get(&service.id).copied().unwrap_or_default();
        let latest = latest_deployment(state, service.id).await?;
        p.services += 1;
        match status(&service, latest.as_ref(), mine).as_str() {
            "running" | "scheduled" => p.running += 1,
            "deploying" => p.deploying += 1,
            "crashed" => {
                p.crashed += 1;
                let project = projects
                    .get(&service.project_id)
                    .cloned()
                    .unwrap_or_default();
                crashed.push((service.id, service.name.clone(), project));
            }
            "failed" => p.failed += 1,
            "stopped" => p.stopped += 1,
            _ => {}
        }
    }

    let today = sqlx::query!(
        r#"SELECT count(*) AS "total!", count(*) FILTER (WHERE status = 'failed') AS "failed!"
           FROM deployments WHERE created_at >= date_trunc('day', now())"#
    )
    .fetch_one(&state.db)
    .await?;
    p.deploys_today = today.total;
    p.deploys_failed_today = today.failed;

    let week = sqlx::query!(
        r#"SELECT count(*) FILTER (WHERE status IN ('running', 'superseded', 'removed')) AS "ok!",
                  count(*) FILTER (WHERE status = 'failed') AS "failed!"
           FROM deployments
           WHERE created_at > now() - interval '7 days' AND finished_at IS NOT NULL"#
    )
    .fetch_one(&state.db)
    .await?;
    let finished = week.ok + week.failed;
    p.build_success_rate = (finished > 0).then(|| week.ok as f64 / finished as f64 * 100.0);

    p.last_backup = sqlx::query!(
        r#"SELECT b.status, b.created_at, s.name AS service
           FROM backups b JOIN services s ON s.id = b.service_id
           ORDER BY b.created_at DESC LIMIT 1"#
    )
    .fetch_optional(&state.db)
    .await?
    .map(|r| LastBackup {
        status: r.status,
        created_at: r.created_at,
        service: r.service,
    });
    p.failed_backups_24h = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM backups WHERE status = 'failed' AND created_at > now() - interval '24 hours'"#
    )
    .fetch_one(&state.db)
    .await?;

    Ok((p, crashed))
}

mod bytes_fmt {
    pub fn human(bytes: i64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"];
        let mut value = bytes as f64;
        let mut unit = 0;
        while value >= 1024.0 && unit < units.len() - 1 {
            value /= 1024.0;
            unit += 1;
        }
        format!("{value:.1} {}", units[unit])
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn formats_sizes() {
            assert_eq!(super::human(512), "512.0 B");
            assert_eq!(super::human(5 * 1024 * 1024 * 1024), "5.0 GB");
        }
    }
}
