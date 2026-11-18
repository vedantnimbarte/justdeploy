//! CPU, memory and network per service, sampled from Docker every 30 seconds and
//! kept for 24 hours.

use crate::{
    cluster,
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::Result,
    state::SharedState,
};
use bollard::{models::ContainerStatsResponse, query_parameters::StatsOptionsBuilder};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default, Clone, Copy)]
struct Totals {
    cpu: f64,
    memory: u64,
    rx: u64,
    tx: u64,
}

/// Every copy on every server, added up per service.
pub async fn sample(state: SharedState) -> Result<()> {
    let mut containers = Vec::new();
    for node in docker::ready_nodes(&state.docker).await? {
        let listed = match cluster::node_docker(&state, &node) {
            Ok(docker) => docker::list_all(&docker, &[(LABEL_KIND, "production")])
                .await
                .map(|list| (docker, list)),
            Err(e) => Err(e),
        };
        let (docker, list) = match listed {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(%node, %error, "no metrics from this server");
                continue;
            }
        };
        for c in list.into_iter().filter(docker::is_running) {
            let service = docker::label(&c, LABEL_SERVICE).and_then(|s| s.parse::<Uuid>().ok());
            if let (Some(service), Some(id)) = (service, c.id) {
                containers.push((docker.clone(), service, id));
            }
        }
    }

    // Without `one_shot`, Docker waits a second to fill in the previous CPU reading,
    // which is what the percentage is computed from. Run them side by side.
    let stats = futures::future::join_all(containers.iter().map(|(docker, service, id)| {
        let docker = docker.clone();
        async move {
            let options = StatsOptionsBuilder::new().stream(false).build();
            let stats = docker.stats(id, Some(options)).next().await;
            (*service, stats.and_then(|s| s.ok()))
        }
    }))
    .await;

    let mut by_service: HashMap<Uuid, Totals> = HashMap::new();
    for (service, stats) in stats {
        if let Some(stats) = stats {
            let t = by_service.entry(service).or_default();
            let s = totals(&stats);
            t.cpu += s.cpu;
            t.memory += s.memory;
            t.rx += s.rx;
            t.tx += s.tx;
        }
    }

    let now = Utc::now();
    for (service, t) in by_service {
        sqlx::query!(
            "INSERT INTO metric_samples (service_id, ts, cpu_percent, memory_bytes, net_rx_bytes, net_tx_bytes)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            service,
            now,
            t.cpu,
            t.memory as i64,
            t.rx as i64,
            t.tx as i64
        )
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

fn totals(stats: &ContainerStatsResponse) -> Totals {
    let cpu = stats.cpu_stats.clone().unwrap_or_default();
    let pre = stats.precpu_stats.clone().unwrap_or_default();
    let used = |c: &bollard::models::ContainerCpuStats| {
        c.cpu_usage
            .as_ref()
            .and_then(|u| u.total_usage)
            .unwrap_or(0)
    };
    let cpu_delta = used(&cpu).saturating_sub(used(&pre)) as f64;
    let system_delta = cpu
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(pre.system_cpu_usage.unwrap_or(0)) as f64;
    let cpus = cpu.online_cpus.unwrap_or(1).max(1) as f64;
    let cpu_percent = if system_delta > 0.0 {
        cpu_delta / system_delta * cpus * 100.0
    } else {
        0.0
    };

    // Page cache is reclaimable; leaving it in overstates what an app uses.
    let memory = stats.memory_stats.as_ref().map_or(0, |m| {
        let cache = m
            .stats
            .as_ref()
            .and_then(|s| {
                s.get("inactive_file")
                    .or_else(|| s.get("total_inactive_file"))
                    .copied()
            })
            .unwrap_or(0);
        m.usage.unwrap_or(0).saturating_sub(cache)
    });

    let (rx, tx) = stats.networks.as_ref().map_or((0, 0), |nets| {
        nets.values().fold((0, 0), |(rx, tx), n| {
            (rx + n.rx_bytes.unwrap_or(0), tx + n.tx_bytes.unwrap_or(0))
        })
    });

    Totals {
        cpu: cpu_percent,
        memory,
        rx,
        tx,
    }
}

#[derive(Serialize)]
pub struct Sample {
    pub ts: DateTime<Utc>,
    pub cpu_percent: f64,
    pub memory_bytes: i64,
    /// Bytes per second since the previous sample.
    pub net_rx_rate: f64,
    pub net_tx_rate: f64,
}

pub async fn series(db: &PgPool, service_id: Uuid, minutes: i32) -> Result<Vec<Sample>> {
    let rows = sqlx::query!(
        "SELECT ts, cpu_percent, memory_bytes, net_rx_bytes, net_tx_bytes FROM metric_samples
         WHERE service_id = $1 AND ts > now() - make_interval(mins => $2)
         ORDER BY ts",
        service_id,
        minutes.clamp(5, 24 * 60)
    )
    .fetch_all(db)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    let mut previous: Option<(DateTime<Utc>, i64, i64)> = None;
    for r in rows {
        let (rx_rate, tx_rate) = match previous {
            // Counters reset when a container is replaced; show zero, not a negative spike.
            Some((ts, rx, tx)) if r.net_rx_bytes >= rx && r.net_tx_bytes >= tx => {
                let secs = (r.ts - ts).num_milliseconds().max(1) as f64 / 1000.0;
                (
                    (r.net_rx_bytes - rx) as f64 / secs,
                    (r.net_tx_bytes - tx) as f64 / secs,
                )
            }
            _ => (0.0, 0.0),
        };
        previous = Some((r.ts, r.net_rx_bytes, r.net_tx_bytes));
        out.push(Sample {
            ts: r.ts,
            cpu_percent: r.cpu_percent,
            memory_bytes: r.memory_bytes,
            net_rx_rate: rx_rate,
            net_tx_rate: tx_rate,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats};

    #[test]
    fn cpu_percent_comes_from_the_deltas() {
        let cpu = |total, system| ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(total),
                ..Default::default()
            }),
            system_cpu_usage: Some(system),
            online_cpus: Some(2),
            ..Default::default()
        };
        let stats = ContainerStatsResponse {
            cpu_stats: Some(cpu(300, 2000)),
            precpu_stats: Some(cpu(100, 1000)),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(1000),
                stats: Some(HashMap::from([("inactive_file".to_string(), 400)])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let t = totals(&stats);
        assert!(
            (t.cpu - 40.0).abs() < 1e-9,
            "200/1000 * 2 cpus = 40%, got {}",
            t.cpu
        );
        assert_eq!(t.memory, 600);
    }
}
