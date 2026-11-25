//! Periodic work: reconcile containers with the database, sample metrics, start
//! scheduled backups and runs, and clean up old data.

use crate::{
    backups, cluster,
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::Result,
    host, logs, metrics, notify, pitr, runs, services,
    state::SharedState,
    uptime, usage,
};
use std::{collections::HashSet, future::Future, time::Duration};
use uuid::Uuid;

pub fn spawn(state: SharedState) {
    every(
        state.clone(),
        "reconcile",
        Duration::from_secs(60),
        reconcile,
    );
    every(
        state.clone(),
        "metrics",
        Duration::from_secs(30),
        metrics::sample,
    );
    every(
        state.clone(),
        "server metrics",
        Duration::from_secs(30),
        host::sample,
    );
    every(
        state.clone(),
        "docker disk usage",
        Duration::from_secs(10 * 60),
        host::refresh_docker_disk,
    );
    every(
        state.clone(),
        "backup schedules",
        Duration::from_secs(60),
        backups::schedule_due,
    );
    every(
        state.clone(),
        "cluster",
        Duration::from_secs(60),
        cluster::refresh,
    );
    every(
        state.clone(),
        "autoscaling",
        Duration::from_secs(60),
        cluster::autoscale,
    );
    every(
        state.clone(),
        "scheduled runs",
        Duration::from_secs(60),
        runs::schedule_due,
    );
    every(
        state.clone(),
        "log collection",
        Duration::from_secs(30),
        logs::collect,
    );
    every(
        state.clone(),
        "request routes",
        Duration::from_secs(30),
        crate::traces::refresh_routes,
    );
    every(
        state.clone(),
        "uptime checks",
        Duration::from_secs(60),
        uptime::check,
    );
    every(
        state.clone(),
        "usage rollup",
        Duration::from_secs(60 * 60),
        usage::rollup,
    );
    every(
        state.clone(),
        "point-in-time base backups",
        Duration::from_secs(10 * 60),
        pitr::base_backups,
    );
    every(
        state.clone(),
        "domain checks",
        Duration::from_secs(10 * 60),
        uptime::check_domains,
    );
    every(
        state.clone(),
        "registry cleanup",
        Duration::from_secs(24 * 60 * 60),
        cluster::clean_registry,
    );
    every(state, "cleanup", Duration::from_secs(60 * 60), cleanup);
}

fn every<F, Fut>(state: SharedState, name: &'static str, period: Duration, task: F)
where
    F: Fn(SharedState) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = task(state.clone()).await {
                tracing::warn!(task = name, %error, "periodic task failed");
            }
        }
    });
}

/// Brings the Swarm back in line with the database: removes what belongs to
/// deleted services, scales stopped ones to nothing, and redeploys a service
/// whose workload is gone, or that still runs as a plain container from before
/// the cluster (that's how an upgrade moves services onto it). Swarm itself
/// restarts copies that exit and moves them off servers that go down.
async fn reconcile(state: SharedState) -> Result<()> {
    let workloads = docker::workloads(&state.docker, &[]).await?;
    let containers = docker::list(&state.docker, &[]).await?;
    let services = sqlx::query!(
        r#"SELECT s.id, s.name, s.desired_state, s.current_deployment_id,
               s.cron_schedule IS NOT NULL AS "scheduled!", p.name AS project
           FROM services s JOIN projects p ON p.id = s.project_id"#
    )
    .fetch_all(&state.db)
    .await?;
    let known: HashSet<Uuid> = services.iter().map(|s| s.id).collect();
    let busy: HashSet<Uuid> = sqlx::query_scalar!(
        "SELECT DISTINCT service_id FROM deployments WHERE status IN ('queued', 'building', 'deploying')"
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();

    let orphan = |label: Option<&str>| {
        label
            .and_then(|s| s.parse::<Uuid>().ok())
            .is_some_and(|id| !known.contains(&id))
    };
    for w in workloads.iter().filter(|w| orphan(w.label(LABEL_SERVICE))) {
        tracing::info!(workload = %w.name, "removing a workload whose service no longer exists");
        docker::remove_service(&state.docker, &w.name).await?;
    }
    for container in containers
        .iter()
        .filter(|c| orphan(docker::label(c, LABEL_SERVICE)))
    {
        if let Some(id) = &container.id {
            tracing::info!(container = %id, "removing a container whose service no longer exists");
            docker::remove(&state.docker, id).await?;
        }
    }

    for row in services {
        // A scheduled service has nothing that stays up.
        if busy.contains(&row.id) || row.current_deployment_id.is_none() || row.scheduled {
            continue;
        }
        let id = row.id.to_string();
        let production: Vec<_> = workloads
            .iter()
            .filter(|w| w.label(LABEL_SERVICE) == Some(id.as_str()))
            .filter(|w| w.label(LABEL_KIND) == Some("production"))
            .collect();
        let legacy = containers
            .iter()
            .filter(|c| docker::label(c, LABEL_SERVICE) == Some(id.as_str()))
            .any(|c| docker::label(c, LABEL_KIND) == Some("production"));
        match row.desired_state.as_str() {
            "running" if production.is_empty() => {
                if legacy {
                    tracing::info!(service = %row.id, "moving onto the cluster");
                } else {
                    tracing::warn!(service = %row.id, "workload missing; redeploying");
                }
                let service = services::get_service(&state.db, row.id).await?;
                services::rerelease(&state, &service, "reconcile").await?;
            }
            "running" => {
                let down = production.iter().all(|w| w.running == 0 && w.desired > 0);
                if down && notify::first_in_an_hour(row.id) {
                    notify::send(
                        &state,
                        notify::Event {
                            kind: "service.crashed",
                            text: format!(
                                "{} in {} has no copy running; the cluster keeps restarting it.",
                                row.name, row.project
                            ),
                            path: format!("/services/{}?tab=logs", row.id),
                        },
                    );
                }
            }
            "stopped" => {
                for w in production.iter().filter(|w| w.desired > 0) {
                    docker::scale(&state.docker, &w.name, 0).await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn cleanup(state: SharedState) -> Result<()> {
    sqlx::query!("DELETE FROM metric_samples WHERE ts < now() - interval '24 hours'")
        .execute(&state.db)
        .await?;
    sqlx::query!("DELETE FROM host_samples WHERE ts < now() - interval '24 hours'")
        .execute(&state.db)
        .await?;
    sqlx::query!("DELETE FROM usage_hourly WHERE hour < now() - interval '90 days'")
        .execute(&state.db)
        .await?;
    sqlx::query!("DELETE FROM uptime_checks WHERE ts < now() - interval '90 days'")
        .execute(&state.db)
        .await?;
    logs::prune(&state).await?;
    crate::traces::prune(&state).await?;
    sqlx::query!("DELETE FROM sessions WHERE expires_at < now()")
        .execute(&state.db)
        .await?;
    sqlx::query!(
        "DELETE FROM deployment_logs WHERE deployment_id IN
            (SELECT id FROM deployments WHERE created_at < now() - interval '30 days')"
    )
    .execute(&state.db)
    .await?;
    // A deployment whose job was lost (the control plane died twice mid-build).
    sqlx::query!(
        "UPDATE deployments SET status = 'failed', error = 'interrupted', finished_at = now()
         WHERE status IN ('building', 'deploying') AND started_at < now() - interval '2 hours'"
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Build and health-check containers left behind by a crash.
pub async fn remove_stale_helpers(state: &SharedState) {
    let filters =
        std::collections::HashMap::from([("label", vec![format!("{}=helper", docker::MANAGED)])]);
    let options = bollard::query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    if let Ok(containers) = state.docker.list_containers(Some(options)).await {
        for id in containers.into_iter().filter_map(|c| c.id) {
            let _ = docker::remove(&state.docker, &id).await;
        }
    }
}
