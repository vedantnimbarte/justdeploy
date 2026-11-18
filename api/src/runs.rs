//! Runs: a container from a service's current image that does one thing and
//! exits. A service with a schedule is run that way instead of staying up;
//! any app or image service can also run a one-off command.

use crate::{
    backups,
    docker::{self, spec},
    error::{bad_request, Result},
    models::{Run, Service},
    names, notify, services,
    state::SharedState,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{collections::VecDeque, time::Duration};
use uuid::Uuid;

/// A run still going after this long is stopped and marked failed.
const RUN_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);
/// Output kept per run: the last lines are the ones that explain a failure.
const KEEP_LINES: usize = 2000;

#[derive(Serialize)]
pub struct Runs {
    next_run_at: Option<DateTime<Utc>>,
    runs: Vec<Run>,
}

pub async fn list(state: &SharedState, service: &Service) -> Result<Runs> {
    let runs = sqlx::query_as!(
        Run,
        "SELECT * FROM runs WHERE service_id = $1 ORDER BY started_at DESC LIMIT 50",
        service.id
    )
    .fetch_all(&state.db)
    .await?;
    let next_run_at = match (&service.cron_schedule, service.desired_state.as_str()) {
        (Some(expr), "running") => backups::parse_cron(expr)?.upcoming(Utc).next(),
        _ => None,
    };
    Ok(Runs { next_run_at, runs })
}

/// Starts a run in the background and returns it. No command runs the image's own.
pub async fn start(
    state: &SharedState,
    service: &Service,
    command: Option<String>,
    trigger: &'static str,
) -> Result<Run> {
    if !matches!(service.kind.as_str(), "app" | "image") {
        return Err(bad_request("only app and image services can run commands"));
    }
    let image = match service.current_deployment_id {
        Some(id) => services::get_deployment(&state.db, id).await?.image_ref,
        None => None,
    }
    .ok_or_else(|| bad_request("deploy the service once before running it"))?;
    let command = command
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());

    let run = sqlx::query_as!(
        Run,
        "INSERT INTO runs (id, service_id, command, trigger) VALUES ($1, $2, $3, $4) RETURNING *",
        Uuid::new_v4(),
        service.id,
        command,
        trigger
    )
    .fetch_one(&state.db)
    .await?;
    tokio::spawn(execute(
        state.clone(),
        service.clone(),
        run.id,
        image,
        command,
        trigger,
    ));
    Ok(run)
}

async fn execute(
    state: SharedState,
    service: Service,
    run_id: Uuid,
    image: String,
    command: Option<String>,
    trigger: &'static str,
) {
    let mut output = VecDeque::new();
    let outcome = async {
        let project = services::get_project(&state.db, service.project_id).await?;
        let network = names::project_network(project.id);
        docker::ensure_network(&state.docker, &network).await?;
        let env = services::runtime_env(&state, &service).await?;
        let body = spec::one_off(&image, command.as_deref(), &env, &service, &network);
        let name = format!("jd-run-{}", &run_id.simple().to_string()[..12]);
        let code = docker::run_to_completion(&state.docker, &name, body, RUN_TIMEOUT, |line| {
            if output.len() == KEEP_LINES {
                output.pop_front();
            }
            output.push_back(line);
        })
        .await?;
        anyhow::Ok((project.name, code))
    }
    .await;

    let text = Vec::from(output).join("\n");
    let (status, code, error) = match &outcome {
        Ok((_, 0)) => ("succeeded", Some(0), None),
        Ok((_, code)) => (
            "failed",
            Some(*code),
            Some(format!("exited with code {code}")),
        ),
        Err(e) => ("failed", None, Some(format!("{e:#}"))),
    };
    if let Err(e) = sqlx::query!(
        "UPDATE runs SET status = $2, exit_code = $3, error = $4, output = $5, finished_at = now()
         WHERE id = $1",
        run_id,
        status,
        code,
        error,
        text
    )
    .execute(&state.db)
    .await
    {
        tracing::warn!(%run_id, error = %e, "could not record the run");
    }

    if let Some(error) = error {
        let what = match &command {
            Some(c) => format!("`{c}`"),
            None if trigger == "schedule" => "The scheduled run".to_string(),
            None => "A run".to_string(),
        };
        let project = outcome.map(|(p, _)| p).unwrap_or_default();
        notify::send(
            &state,
            notify::Event {
                kind: "run.failed",
                text: format!("{what} on {} in {project} failed: {error}.", service.name),
                path: format!("/services/{}?tab=runs", service.id),
            },
        );
    }
}

/// Starts every scheduled service whose next time has come, unless its previous
/// run is still going.
pub async fn schedule_due(state: SharedState) -> Result<()> {
    let rows = sqlx::query!(
        r#"SELECT s.id, s.cron_schedule AS "cron!", s.updated_at,
               (SELECT max(r.started_at) FROM runs r WHERE r.service_id = s.id AND r.trigger = 'schedule') AS last_run,
               EXISTS (SELECT 1 FROM runs r WHERE r.service_id = s.id AND r.status = 'running') AS "busy!"
           FROM services s
           WHERE s.cron_schedule IS NOT NULL AND s.desired_state = 'running'
             AND s.current_deployment_id IS NOT NULL"#
    )
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    for row in rows.into_iter().filter(|r| !r.busy) {
        let Ok(schedule) = backups::parse_cron(&row.cron) else {
            continue;
        };
        // Counting from the last run (or the last settings change) means a
        // restart that misses a time runs it once on the way back, not again and again.
        let since = row.last_run.unwrap_or(row.updated_at).max(row.updated_at);
        if schedule
            .after(&since)
            .next()
            .is_some_and(|next| next <= now)
        {
            let service = services::get_service(&state.db, row.id).await?;
            start(&state, &service, None, "schedule").await?;
        }
    }
    Ok(())
}

/// Runs are watched from this process, so any still marked running at startup
/// were cut off by the restart (their containers are removed as stale helpers).
pub async fn fail_interrupted(state: &SharedState) -> Result<()> {
    sqlx::query!(
        "UPDATE runs SET status = 'failed', error = 'interrupted by a restart of justdeploy', finished_at = now()
         WHERE status = 'running'"
    )
    .execute(&state.db)
    .await?;
    Ok(())
}
