//! Uptime. Every public service is fetched once a minute at its public address,
//! the way visitors reach it. Two misses in a row is down: a notification goes
//! out, and another when it answers again. A project can publish a status page
//! of the services it picks.

use crate::{
    error::{bad_request, Error, Result},
    names, notify, services,
    state::SharedState,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

const MISSES_TO_DOWN: u32 = 2;

struct Outcome {
    up: bool,
    status: Option<i32>,
    latency_ms: Option<i32>,
    error: Option<String>,
}

/// Up means it answered, with anything but a server error, within ten seconds.
async fn probe(url: &str) -> Outcome {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("justdeploy-uptime")
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    });
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status().as_u16() as i32;
            Outcome {
                up: status < 500,
                status: Some(status),
                latency_ms: Some(started.elapsed().as_millis() as i32),
                error: (status >= 500).then(|| format!("answered {status}")),
            }
        }
        Err(e) => Outcome {
            up: false,
            status: None,
            latency_ms: None,
            error: Some(if e.is_timeout() {
                "no answer within 10 seconds".into()
            } else if e.is_connect() {
                "couldn't connect".into()
            } else {
                e.to_string()
            }),
        },
    }
}

/// Every minute. Skipped while the base domain is `localhost`: there's no
/// public address to check.
pub async fn check(state: SharedState) -> Result<()> {
    let base = services::base_domain(&state).await?;
    if base == "localhost" {
        return Ok(());
    }
    let scheme = if state.config.tls() { "https" } else { "http" };
    let rows = sqlx::query!(
        "SELECT s.id, s.name, s.health_path, p.name AS project FROM services s
         JOIN projects p ON p.id = s.project_id
         WHERE s.public AND s.port IS NOT NULL AND s.cron_schedule IS NULL
           AND s.desired_state = 'running' AND s.current_deployment_id IS NOT NULL"
    )
    .fetch_all(&state.db)
    .await?;
    let checks = futures::future::join_all(rows.into_iter().map(|row| {
        let url = format!(
            "{scheme}://{}{}",
            names::generated_host(&row.name, &row.project, &base),
            row.health_path.as_deref().unwrap_or("/")
        );
        async move { (row, probe(&url).await) }
    }))
    .await;
    let now = Utc::now();
    for (row, outcome) in checks {
        sqlx::query!(
            "INSERT INTO uptime_checks (service_id, ts, up, status, latency_ms, error)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            row.id,
            now,
            outcome.up,
            outcome.status,
            outcome.latency_ms,
            outcome.error
        )
        .execute(&state.db)
        .await?;
        let label = format!("{} in {}", row.name, row.project);
        match transition(row.id, outcome.up) {
            Some(true) => notify::send(
                &state,
                notify::Event {
                    kind: "service.down",
                    text: format!(
                        "{label} is down: {}.",
                        outcome.error.unwrap_or_else(|| "no answer".into())
                    ),
                    path: format!("/services/{}?tab=logs", row.id),
                },
            ),
            Some(false) => notify::send(
                &state,
                notify::Event {
                    kind: "service.recovered",
                    text: format!("{label} is answering again."),
                    path: format!("/services/{}", row.id),
                },
            ),
            None => {}
        }
    }
    Ok(())
}

/// Some(true) when a service just went down, Some(false) when it just came back.
fn transition(service: Uuid, up: bool) -> Option<bool> {
    static STATE: OnceLock<Mutex<HashMap<Uuid, (u32, bool)>>> = OnceLock::new();
    let mut all = STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (misses, down) = all.entry(service).or_insert((0, false));
    if up {
        *misses = 0;
        return std::mem::replace(down, false).then_some(false);
    }
    *misses += 1;
    (*misses == MISSES_TO_DOWN && !std::mem::replace(down, true)).then_some(true)
}

#[derive(Serialize)]
pub struct ServiceUptime {
    id: Uuid,
    name: String,
    url: Option<String>,
    /// The last check: None before the first.
    up: Option<bool>,
    latency_ms: Option<i32>,
    checked_at: Option<DateTime<Utc>>,
    /// Share of checks that were up, in percent.
    uptime_24h: Option<f64>,
    uptime_7d: Option<f64>,
    uptime_90d: Option<f64>,
    /// The last 90 days, oldest first; None for a day without checks.
    days: Vec<Option<f64>>,
    /// The latest ten times it was down.
    incidents: Vec<Incident>,
}

#[derive(Serialize)]
struct Incident {
    from: DateTime<Utc>,
    /// None while still down.
    to: Option<DateTime<Utc>>,
}

/// The public services of a project, or only those on its status page.
pub async fn summary(
    state: &SharedState,
    project_id: Uuid,
    status_page_only: bool,
) -> Result<Vec<ServiceUptime>> {
    let project = services::get_project(&state.db, project_id).await?;
    let base = services::base_domain(state).await?;
    let scheme = if state.config.tls() { "https" } else { "http" };
    let mut out = Vec::new();
    for service in services::list_services(&state.db, project_id).await? {
        if !service.public
            || service.port.is_none()
            || (status_page_only && !service.on_status_page)
        {
            continue;
        }
        let id = service.id;
        let latest = sqlx::query!(
            "SELECT ts, up, latency_ms FROM uptime_checks WHERE service_id = $1 ORDER BY ts DESC LIMIT 1",
            id
        )
        .fetch_optional(&state.db)
        .await?;
        let windows = sqlx::query!(
            r#"SELECT 100 * avg(up::int) FILTER (WHERE ts > now() - interval '1 day') AS "d1: f64",
                      100 * avg(up::int) FILTER (WHERE ts > now() - interval '7 days') AS "d7: f64",
                      100 * avg(up::int) AS "d90: f64"
               FROM uptime_checks WHERE service_id = $1 AND ts > now() - interval '90 days'"#,
            id
        )
        .fetch_one(&state.db)
        .await?;
        let daily: HashMap<i64, f64> = sqlx::query!(
            r#"SELECT (now()::date - ts::date) AS "ago!", 100 * avg(up::int)::float8 AS "pct!"
               FROM uptime_checks WHERE service_id = $1 AND ts > now() - interval '90 days'
               GROUP BY 1"#,
            id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|r| (r.ago as i64, r.pct))
        .collect();
        let downs = sqlx::query_scalar!(
            "SELECT ts FROM uptime_checks WHERE service_id = $1 AND NOT up
               AND ts > now() - interval '90 days' ORDER BY ts",
            id
        )
        .fetch_all(&state.db)
        .await?;
        let still_down = latest.as_ref().is_some_and(|l| !l.up);
        out.push(ServiceUptime {
            id,
            url: Some(format!(
                "{scheme}://{}",
                names::generated_host(&service.name, &project.name, &base)
            )),
            name: service.name,
            up: latest.as_ref().map(|l| l.up),
            latency_ms: latest.as_ref().and_then(|l| l.latency_ms),
            checked_at: latest.as_ref().map(|l| l.ts),
            uptime_24h: windows.d1,
            uptime_7d: windows.d7,
            uptime_90d: windows.d90,
            days: (0..90).rev().map(|ago| daily.get(&ago).copied()).collect(),
            incidents: incidents(&downs, still_down),
        });
    }
    Ok(out)
}

/// Runs of failed checks a minute or two apart, from at least two misses (the
/// same rule that sends the notification), newest first.
fn incidents(downs: &[DateTime<Utc>], still_down: bool) -> Vec<Incident> {
    let mut runs: Vec<(DateTime<Utc>, DateTime<Utc>, usize)> = Vec::new();
    for &ts in downs {
        match runs.last_mut() {
            Some((_, last, count)) if (ts - *last).num_seconds() <= 150 => {
                *last = ts;
                *count += 1;
            }
            _ => runs.push((ts, ts, 1)),
        }
    }
    let total = runs.len();
    runs.into_iter()
        .enumerate()
        .filter(|(_, (_, _, count))| *count as u32 >= MISSES_TO_DOWN)
        .map(|(i, (from, last, _))| Incident {
            from,
            to: (!(still_down && i + 1 == total)).then(|| last + chrono::Duration::minutes(1)),
        })
        .rev()
        .take(10)
        .collect()
}

// ---------------------------------------------------------------- domains

/// Every ten minutes: whether each custom domain still resolves to the
/// cluster. A change either way is reported once; "unknown" (this server's own
/// address can't be worked out) changes nothing.
pub async fn check_domains(state: SharedState) -> Result<()> {
    let rows = sqlx::query!(
        r#"SELECT d.id, d.hostname, d.dns_ok, s.id AS service_id, s.name AS service
           FROM domains d JOIN services s ON s.id = d.service_id
           WHERE s.desired_state = 'running'"#
    )
    .fetch_all(&state.db)
    .await?;
    for row in rows {
        let status = crate::handlers::admin::check_host(&state, &row.hostname)
            .await?
            .status;
        let Some(ok) = domain_ok(status) else {
            continue;
        };
        if row.dns_ok == Some(ok) {
            continue;
        }
        sqlx::query!("UPDATE domains SET dns_ok = $2 WHERE id = $1", row.id, ok)
            .execute(&state.db)
            .await?;
        let path = format!("/services/{}?tab=domains", row.service_id);
        match (row.dns_ok, ok) {
            (Some(true), false) => notify::send(
                &state,
                notify::Event {
                    kind: "domain.unreachable",
                    text: format!(
                        "{} no longer points at this server ({}), so {} isn't reachable there.",
                        row.hostname,
                        if status == "missing" {
                            "it has no DNS record"
                        } else {
                            "its record points elsewhere"
                        },
                        row.service
                    ),
                    path,
                },
            ),
            (Some(false), true) => notify::send(
                &state,
                notify::Event {
                    kind: "domain.reachable",
                    text: format!("{} points at this server again.", row.hostname),
                    path,
                },
            ),
            _ => {}
        }
    }
    Ok(())
}

/// A DNS check's status as connected or not; None when it can't be told.
fn domain_ok(status: &str) -> Option<bool> {
    match status {
        "connected" => Some(true),
        "missing" | "elsewhere" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------- status page

#[derive(Serialize)]
pub struct StatusPage {
    title: String,
    /// operational | degraded | down
    overall: &'static str,
    services: Vec<ServiceUptime>,
}

/// What a published status page shows. Not found when it isn't published.
pub async fn status_page(state: &SharedState, project_name: &str) -> Result<StatusPage> {
    let project = sqlx::query!(
        "SELECT id, name, environment FROM projects WHERE name = $1 AND status_page",
        project_name
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(Error::NotFound)?;
    let services = summary(state, project.id, true).await?;
    let down = services.iter().filter(|s| s.up == Some(false)).count();
    Ok(StatusPage {
        title: project.name,
        overall: match down {
            0 => "operational",
            n if n == services.len() => "down",
            _ => "degraded",
        },
        services,
    })
}

/// The project whose status page a hostname shows: `status-<project>.<base
/// domain>`, or a project's own status domain.
pub async fn page_for_host(state: &SharedState, host: &str) -> Result<Option<String>> {
    let host = host.split(':').next().unwrap_or_default().to_lowercase();
    let base = services::base_domain(state).await?;
    let from_name = host
        .strip_prefix("status-")
        .and_then(|rest| rest.strip_suffix(&format!(".{base}")))
        .map(str::to_string);
    let name = match from_name {
        Some(name) => Some(name),
        None => {
            sqlx::query_scalar!("SELECT name FROM projects WHERE status_domain = $1", host)
                .fetch_optional(&state.db)
                .await?
        }
    };
    let Some(name) = name else { return Ok(None) };
    let published = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM projects WHERE name = $1 AND status_page) AS "p!""#,
        name
    )
    .fetch_one(&state.db)
    .await?;
    Ok(published.then_some(name))
}

/// Every hostname that serves a status page, for routing them to justdeploy.
pub async fn status_hosts(state: &SharedState) -> Result<Vec<String>> {
    let base = services::base_domain(state).await?;
    let rows =
        sqlx::query!("SELECT name, status_domain FROM projects WHERE status_page ORDER BY name")
            .fetch_all(&state.db)
            .await?;
    let mut hosts = Vec::new();
    for row in rows {
        hosts.push(format!("status-{}.{base}", row.name));
        hosts.extend(row.status_domain);
    }
    Ok(hosts)
}

/// Publishes or hides a project's status page and picks its services.
pub async fn configure(
    state: &SharedState,
    project_id: Uuid,
    enabled: bool,
    domain: Option<String>,
    service_ids: &[Uuid],
) -> Result<()> {
    let domain = domain
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty());
    if let Some(d) = &domain {
        names::validate_hostname(d)?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!(
        "UPDATE projects SET status_page = $2, status_domain = $3 WHERE id = $1",
        project_id,
        enabled,
        domain
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => bad_request("another project's status page uses that domain"),
        other => other,
    })?;
    sqlx::query!(
        "UPDATE services SET on_status_page = (id = ANY($2)) WHERE project_id = $1",
        project_id,
        service_ids
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_dns_changes_nothing() {
        assert_eq!(domain_ok("connected"), Some(true));
        assert_eq!(domain_ok("elsewhere"), Some(false));
        assert_eq!(domain_ok("unknown"), None);
    }

    #[test]
    fn down_needs_two_misses_and_recovery_is_reported_once() {
        let id = Uuid::new_v4();
        assert_eq!(transition(id, false), None, "one miss is a blip");
        assert_eq!(transition(id, false), Some(true), "the second is down");
        assert_eq!(transition(id, false), None, "still down, no repeat");
        assert_eq!(transition(id, true), Some(false), "back up");
        assert_eq!(transition(id, true), None);
    }

    #[test]
    fn incidents_group_misses_and_skip_blips() {
        let t = |m: i64| DateTime::<Utc>::from_timestamp(1_800_000_000 + m * 60, 0).unwrap();
        let downs = vec![t(0), t(1), t(2), t(30), t(60), t(61)];
        let found = incidents(&downs, true);
        assert_eq!(found.len(), 2, "the lone miss at 30 is a blip");
        assert_eq!(found[0].from, t(60));
        assert!(found[0].to.is_none(), "still down");
        assert_eq!(found[1].from, t(0));
        assert_eq!(found[1].to, Some(t(3)));
    }
}
