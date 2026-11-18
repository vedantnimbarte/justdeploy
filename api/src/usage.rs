//! Usage over time. The 30-second samples (kept a day) roll up into hourly
//! averages kept 90 days, with each service's volumes measured every hour.

use crate::{cluster, docker, error::Result, state::SharedState};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

/// Hourly: the last complete hour (and the one before, in case a restart missed it).
pub async fn rollup(state: SharedState) -> Result<()> {
    for back in [2, 1] {
        sqlx::query!(
            r#"INSERT INTO usage_hourly (service_id, hour, cpu_percent, memory_bytes, net_rx_bytes, net_tx_bytes)
               SELECT service_id, date_trunc('hour', now()) - make_interval(hours => $1),
                      avg(cpu_percent), avg(memory_bytes)::bigint,
                      greatest(max(net_rx_bytes) - min(net_rx_bytes), 0),
                      greatest(max(net_tx_bytes) - min(net_tx_bytes), 0)
               FROM metric_samples
               WHERE ts >= date_trunc('hour', now()) - make_interval(hours => $1)
                 AND ts < date_trunc('hour', now()) - make_interval(hours => $1 - 1)
               GROUP BY service_id
               ON CONFLICT DO NOTHING"#,
            back
        )
        .execute(&state.db)
        .await?;
    }

    // Volume sizes, from whichever server holds each.
    let mut sizes: HashMap<String, i64> = HashMap::new();
    for node in docker::ready_nodes(&state.docker).await? {
        let Ok(docker) = cluster::node_docker(&state, &node) else {
            continue;
        };
        // Each volume's size is only computed when asked for in detail. No
        // `type` filter: a list doesn't survive bollard's query encoding.
        let options = bollard::query_parameters::DataUsageOptions {
            _type: None,
            verbose: true,
        };
        match docker.df(Some(options)).await {
            Ok(df) => {
                let items = df.volume_usage.and_then(|v| v.items).unwrap_or_default();
                for volume in items {
                    let name = volume["Name"].as_str().unwrap_or_default().to_string();
                    let size = volume["UsageData"]["Size"].as_i64().unwrap_or(0);
                    sizes.insert(name, size.max(0));
                }
            }
            Err(error) => tracing::debug!(%node, %error, "no volume sizes from this server"),
        }
    }
    let mut per_service: HashMap<Uuid, i64> = HashMap::new();
    for row in sqlx::query!("SELECT service_id, name FROM volumes")
        .fetch_all(&state.db)
        .await?
    {
        *per_service.entry(row.service_id).or_default() +=
            sizes.get(&row.name).copied().unwrap_or(0);
    }
    for (service, disk) in per_service {
        sqlx::query!(
            "UPDATE usage_hourly SET disk_bytes = $2
             WHERE service_id = $1 AND hour = date_trunc('hour', now()) - interval '1 hour'",
            service,
            disk
        )
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Point {
    t: DateTime<Utc>,
    service_id: Uuid,
    /// Percent of one core, all copies added up.
    cpu: f64,
    memory: i64,
    /// Its volumes.
    disk: i64,
}

/// A project's services over `range`: a day in 15-minute steps, a week hourly,
/// or a quarter daily.
pub async fn series(state: &SharedState, project_id: Uuid, range: &str) -> Result<Vec<Point>> {
    let points = match range {
        "week" | "quarter" => {
            let (days, step) = if range == "week" { (7, "hour") } else { (90, "day") };
            sqlx::query!(
                r#"SELECT date_trunc($3, u.hour) AS "t!", u.service_id,
                          avg(u.cpu_percent) AS "cpu!", avg(u.memory_bytes)::bigint AS "memory!",
                          max(u.disk_bytes) AS "disk!"
                   FROM usage_hourly u JOIN services s ON s.id = u.service_id
                   WHERE s.project_id = $1 AND u.hour > now() - make_interval(days => $2)
                   GROUP BY 1, 2 ORDER BY 1"#,
                project_id,
                days,
                step
            )
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .map(|r| Point {
                t: r.t,
                service_id: r.service_id,
                cpu: r.cpu,
                memory: r.memory,
                disk: r.disk,
            })
            .collect()
        }
        _ => sqlx::query!(
            r#"SELECT date_bin(interval '15 minutes', m.ts, TIMESTAMPTZ '2000-01-01') AS "t!",
                      m.service_id, avg(m.cpu_percent) AS "cpu!",
                      avg(m.memory_bytes)::bigint AS "memory!",
                      coalesce((SELECT u.disk_bytes FROM usage_hourly u WHERE u.service_id = m.service_id
                                ORDER BY u.hour DESC LIMIT 1), 0) AS "disk!"
               FROM metric_samples m JOIN services s ON s.id = m.service_id
               WHERE s.project_id = $1 AND m.ts > now() - interval '1 day'
               GROUP BY 1, 2 ORDER BY 1"#,
            project_id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|r| Point {
            t: r.t,
            service_id: r.service_id,
            cpu: r.cpu,
            memory: r.memory,
            disk: r.disk,
        })
        .collect(),
    };
    Ok(points)
}
