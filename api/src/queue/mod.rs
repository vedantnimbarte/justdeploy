//! Durable background work. Jobs live in Postgres, so a deploy survives a restart.
//!
//! Two queues: `deploys` (builds, releases, removals), worked by
//! `JD_DEPLOY_WORKERS` loops, and `backups`, worked by one. Jobs that share a
//! lock key (one service, or one project) run one at a time and in order, so two
//! deploys of the same service can never race.

pub mod deploy;
pub mod maintenance;

use crate::{error::Result, state::SharedState};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

const DEPLOYS: &str = "deploys";
const BACKUPS: &str = "backups";
/// A lock this old belongs to a worker that died mid-job.
const STALE_LOCK_MINUTES: i32 = 45;
const MAX_ATTEMPTS: i32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    Deploy {
        deployment_id: Uuid,
        /// Only the lock key uses it; jobs queued before it existed carry nil.
        #[serde(default)]
        service_id: Uuid,
    },
    RemoveService {
        service_id: Uuid,
    },
    RemoveProject {
        project_id: Uuid,
    },
    RemovePreview {
        service_id: Uuid,
        pr: i32,
    },
    Backup {
        backup_id: Uuid,
    },
    Restore {
        backup_id: Uuid,
    },
}

impl Job {
    fn queue(&self) -> &'static str {
        match self {
            Job::Backup { .. } | Job::Restore { .. } => BACKUPS,
            _ => DEPLOYS,
        }
    }

    fn lock_key(&self) -> Option<String> {
        match self {
            Job::Deploy { service_id, .. }
            | Job::RemoveService { service_id }
            | Job::RemovePreview { service_id, .. } => Some(service_id.to_string()),
            Job::RemoveProject { project_id } => Some(project_id.to_string()),
            Job::Backup { .. } | Job::Restore { .. } => None,
        }
    }
}

pub async fn enqueue(db: &PgPool, job: Job) -> Result<()> {
    let payload = serde_json::to_value(&job).map_err(anyhow::Error::from)?;
    let kind = payload["kind"].as_str().unwrap_or_default().to_string();
    sqlx::query!(
        "INSERT INTO jobs (queue, kind, payload, lock_key) VALUES ($1, $2, $3, $4)",
        job.queue(),
        kind,
        payload,
        job.lock_key()
    )
    .execute(db)
    .await?;
    Ok(())
}

struct Claimed {
    id: i64,
    payload: serde_json::Value,
    attempts: i32,
}

async fn claim(db: &PgPool, queue: &str) -> Result<Option<Claimed>> {
    Ok(sqlx::query_as!(
        Claimed,
        r#"
        UPDATE jobs SET locked_at = now(), attempts = attempts + 1
        WHERE id = (
            SELECT id FROM jobs j
            WHERE queue = $1
              AND run_at <= now()
              AND (locked_at IS NULL OR locked_at < now() - make_interval(mins => $2))
              -- an older job with the same key is running, waiting or retrying
              AND NOT EXISTS (SELECT 1 FROM jobs o WHERE o.lock_key = j.lock_key AND o.id < j.id)
            ORDER BY id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        RETURNING id, payload, attempts
        "#,
        queue,
        STALE_LOCK_MINUTES
    )
    .fetch_optional(db)
    .await?)
}

/// One control plane runs at a time, so any job still locked at startup belonged
/// to the previous process and can be picked up again straight away.
pub async fn release_locks(db: &PgPool) -> Result<()> {
    let released = sqlx::query!("UPDATE jobs SET locked_at = NULL WHERE locked_at IS NOT NULL")
        .execute(db)
        .await?
        .rows_affected();
    if released > 0 {
        tracing::info!(released, "resuming jobs interrupted by a restart");
    }
    Ok(())
}

pub fn spawn(state: SharedState) {
    let deploy_workers = std::iter::repeat_n(DEPLOYS, state.config.deploy_workers);
    for queue in deploy_workers.chain([BACKUPS]) {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                match work_once(&state, queue).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => tracing::error!(%queue, %error, "worker loop failed"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }
    maintenance::spawn(state);
}

async fn work_once(state: &SharedState, queue: &str) -> Result<bool> {
    let Some(claimed) = claim(&state.db, queue).await? else {
        return Ok(false);
    };
    let result = match serde_json::from_value::<Job>(claimed.payload.clone()) {
        Ok(job) => {
            tracing::info!(?job, "running job");
            run(state, job).await
        }
        Err(e) => Err(anyhow::anyhow!("unreadable job payload: {e}").into()),
    };
    match result {
        Ok(()) => {
            sqlx::query!("DELETE FROM jobs WHERE id = $1", claimed.id)
                .execute(&state.db)
                .await?;
        }
        Err(error) if claimed.attempts >= MAX_ATTEMPTS => {
            tracing::error!(%error, job = claimed.id, "job failed for good");
            sqlx::query!("DELETE FROM jobs WHERE id = $1", claimed.id)
                .execute(&state.db)
                .await?;
        }
        Err(error) => {
            tracing::warn!(%error, job = claimed.id, "job failed, retrying in a minute");
            sqlx::query!(
                "UPDATE jobs SET locked_at = NULL, run_at = now() + interval '1 minute', last_error = $2 WHERE id = $1",
                claimed.id,
                error.to_string()
            )
            .execute(&state.db)
            .await?;
        }
    }
    Ok(true)
}

async fn run(state: &SharedState, job: Job) -> Result<()> {
    match job {
        Job::Deploy { deployment_id, .. } => deploy::run(state, deployment_id).await,
        Job::RemoveService { service_id } => deploy::remove_service(state, service_id).await,
        Job::RemoveProject { project_id } => deploy::remove_project(state, project_id).await,
        Job::RemovePreview { service_id, pr } => {
            deploy::remove_preview(state, service_id, pr).await
        }
        Job::Backup { backup_id } => crate::backups::run_backup(state, backup_id).await,
        Job::Restore { backup_id } => crate::backups::run_restore(state, backup_id).await,
    }
}
