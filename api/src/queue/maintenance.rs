//! Periodic work: reconcile containers with the database, sample metrics, start
//! scheduled backups and runs, and clean up old data.

use crate::{
    backups,
    docker::{self, LABEL_KIND, LABEL_SERVICE},
    error::Result,
    host, metrics, notify, runs, services,
    state::SharedState,
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
        "scheduled runs",
        Duration::from_secs(60),
        runs::schedule_due,
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

/// Brings Docker back in line with the database: removes containers of deleted
/// services, restarts stopped ones, and redeploys a service whose container is
/// gone (removed by hand, or lost with a pruned host).
async fn reconcile(state: SharedState) -> Result<()> {
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

    for container in &containers {
        let service = docker::label(container, LABEL_SERVICE).and_then(|s| s.parse::<Uuid>().ok());
        if service.is_some_and(|id| !known.contains(&id)) {
            if let Some(id) = &container.id {
                tracing::info!(container = %id, "removing a container whose service no longer exists");
                docker::remove(&state.docker, id).await?;
            }
        }
    }

    for row in services {
        // A scheduled service has nothing that stays up.
        if busy.contains(&row.id) || row.current_deployment_id.is_none() || row.scheduled {
            continue;
        }
        let id = row.id.to_string();
        let production: Vec<_> = containers
            .iter()
            .filter(|c| docker::label(c, LABEL_SERVICE) == Some(id.as_str()))
            .filter(|c| docker::label(c, LABEL_KIND) == Some("production"))
            .collect();
        match row.desired_state.as_str() {
            "running" if production.is_empty() => {
                tracing::warn!(service = %row.id, "container missing; redeploying");
                let service = services::get_service(&state.db, row.id).await?;
                services::rerelease(&state, &service, "reconcile").await?;
            }
            "running" => {
                let down: Vec<_> = production
                    .iter()
                    .filter(|c| !docker::is_running(c))
                    .collect();
                if !down.is_empty() && notify::first_in_an_hour(row.id) {
                    notify::send(
                        &state,
                        notify::Event {
                            kind: "service.crashed",
                            text: format!(
                                "{} in {} crashed and is being restarted.",
                                row.name, row.project
                            ),
                            path: format!("/services/{}?tab=logs", row.id),
                        },
                    );
                }
                for container in down {
                    if let Some(cid) = &container.id {
                        let _ = docker::start(&state.docker, cid).await;
                    }
                }
            }
            "stopped" => {
                for container in production.iter().filter(|c| docker::is_running(c)) {
                    if let Some(cid) = &container.id {
                        docker::stop(&state.docker, cid).await?;
                    }
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
