//! Point-in-time recovery for PostgreSQL, with wal-g.
//!
//! Turned on, a database switches to the justdeploy Postgres image (the same
//! Postgres, plus wal-g) and archives every finished write-ahead log segment,
//! at least one a minute, to an S3 destination, with a base backup a day (the
//! newest seven kept). Restoring never touches the original: a new database is
//! filled from the base backup before the chosen moment, replays the log up to
//! it, and starts as an ordinary service beside the first.

use crate::{
    backups::{self, DestinationConfig},
    cluster,
    docker::{self, MANAGED},
    error::{bad_request, Result},
    models::{Service, Volume},
    notify, services,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use bollard::models::{ContainerCreateBody, HostConfig};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

/// wal-g's settings for a database's archive: a folder of its own in the destination.
pub async fn walg_env(
    state: &SharedState,
    service_id: Uuid,
    destination_id: Uuid,
) -> anyhow::Result<Vec<(String, String)>> {
    let DestinationConfig::S3 {
        endpoint,
        region,
        bucket,
        access_key_id,
        secret_access_key,
        prefix,
    } = backups::load_destination(state, destination_id).await?
    else {
        bail!("point-in-time recovery needs an S3 destination");
    };
    let prefix = prefix.trim_matches('/');
    let path = match prefix {
        "" => format!("pitr/{service_id}"),
        p => format!("{p}/pitr/{service_id}"),
    };
    let mut env = vec![
        (
            "WALG_S3_PREFIX".to_string(),
            format!("s3://{bucket}/{path}"),
        ),
        ("AWS_ACCESS_KEY_ID".to_string(), access_key_id),
        ("AWS_SECRET_ACCESS_KEY".to_string(), secret_access_key),
        ("AWS_REGION".to_string(), region),
    ];
    if !endpoint.is_empty() {
        env.push(("AWS_ENDPOINT".to_string(), endpoint));
        env.push(("AWS_S3_FORCE_PATH_STYLE".to_string(), "true".to_string()));
    }
    Ok(env)
}

/// Postgres with archiving on: each finished segment goes to the archive, and
/// a segment closes at least once a minute so recent changes are never far behind.
pub fn archive_args() -> Vec<String> {
    [
        "postgres",
        "-c",
        "wal_level=replica",
        "-c",
        "archive_mode=on",
        "-c",
        "archive_command=wal-g wal-push %p",
        "-c",
        "archive_timeout=60",
    ]
    .map(String::from)
    .to_vec()
}

pub fn archiving(service: &Service) -> bool {
    service.engine.as_deref() == Some("postgres") && service.pitr_destination_id.is_some()
}

/// Turns archiving on (with an S3 destination) or off, then redeploys the
/// database with the image and settings that go with it.
pub async fn configure(
    state: &SharedState,
    service: &Service,
    destination: Option<Uuid>,
) -> Result<()> {
    if service.engine.as_deref() != Some("postgres") {
        return Err(bad_request(
            "point-in-time recovery is for PostgreSQL databases",
        ));
    }
    if service.replica_of.is_some() {
        return Err(bad_request(
            "a read replica has its primary's history; turn recovery on for the primary",
        ));
    }
    if let Some(id) = destination {
        walg_env(state, service.id, id)
            .await
            .map_err(|e| bad_request(format!("{e:#}")))?;
    }
    sqlx::query!(
        "UPDATE services SET pitr_destination_id = $2, pitr_base_backup_at = NULL WHERE id = $1",
        service.id,
        destination
    )
    .execute(&state.db)
    .await?;
    let service = services::get_service(&state.db, service.id).await?;
    services::deploy(state, &service, "settings").await?;
    Ok(())
}

/// Runs a shell script in a running container and returns its exit code and output.
const CONNECT: &str =
    r#"export PGHOST=/var/run/postgresql PGUSER="$POSTGRES_USER" PGDATABASE="$POSTGRES_DB""#;

/// Every ten minutes: a base backup for each archiving database without one
/// from the last day. wal-g keeps the newest seven, and the log they need.
pub async fn base_backups(state: SharedState) -> Result<()> {
    let due = sqlx::query!(
        "SELECT s.id, s.name FROM services s
         WHERE s.pitr_destination_id IS NOT NULL AND s.desired_state = 'running'
           AND s.current_deployment_id IS NOT NULL
           AND (s.pitr_base_backup_at IS NULL OR s.pitr_base_backup_at < now() - interval '1 day')
           AND NOT EXISTS (SELECT 1 FROM deployments d WHERE d.service_id = s.id
                           AND d.status IN ('queued', 'building', 'deploying'))"
    )
    .fetch_all(&state.db)
    .await?;
    for row in due {
        let outcome = async {
            let (docker, container) = cluster::running_container(&state, row.id)
                .await?
                .ok_or_else(|| anyhow!("the database is not running"))?;
            let script = format!(
                r#"{CONNECT}; wal-g backup-push "$PGDATA" && wal-g delete retain FULL 7 --confirm"#
            );
            let (code, output) = docker::exec(&docker, &container, "postgres", &script).await?;
            if code != 0 {
                bail!("wal-g exited with code {code}: {}", last_lines(&output));
            }
            anyhow::Ok(())
        }
        .await;
        match outcome {
            Ok(()) => {
                sqlx::query!(
                    "UPDATE services SET pitr_base_backup_at = now() WHERE id = $1",
                    row.id
                )
                .execute(&state.db)
                .await?;
            }
            Err(error) => {
                tracing::warn!(service = %row.id, error = format!("{error:#}"), "base backup failed");
                if notify::first_in_an_hour(row.id) {
                    notify::send(
                        &state,
                        notify::Event {
                            kind: "backup.failed",
                            text: format!(
                                "The point-in-time base backup of {} failed: {error:#}",
                                row.name
                            ),
                            path: format!("/services/{}?tab=backups", row.id),
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

fn last_lines(output: &str) -> String {
    let lines: Vec<&str> = output.trim().lines().collect();
    lines[lines.len().saturating_sub(5)..].join(" / ")
}

#[derive(Deserialize)]
struct BaseBackup {
    backup_name: String,
    finish_time: DateTime<Utc>,
}

/// The base backups in an archive, oldest first.
fn parse_backups(json: &str) -> anyhow::Result<Vec<BaseBackup>> {
    // wal-g prints log lines (which may hold brackets) before the JSON; the list
    // starts on a line of its own.
    let mut offset = 0;
    let mut start = None;
    for line in json.split_inclusive('\n') {
        if line.trim_start().starts_with('[') {
            start = Some(offset);
            break;
        }
        offset += line.len();
    }
    let start = start.ok_or_else(|| anyhow!("no backups listed: {}", last_lines(json)))?;
    let mut backups: Vec<BaseBackup> = serde_json::from_str(json[start..].trim())?;
    backups.sort_by_key(|b| b.finish_time);
    Ok(backups)
}

#[derive(Serialize, Default)]
pub struct Status {
    destination_id: Option<Uuid>,
    base_backup_at: Option<DateTime<Utc>>,
    /// The earliest moment that can be restored: when the oldest base backup finished.
    earliest: Option<DateTime<Utc>>,
    /// The last log segment that reached the archive.
    archived_at: Option<DateTime<Utc>>,
    failed_count: i64,
    failed_at: Option<DateTime<Utc>>,
    /// Why the rest couldn't be read, when it couldn't.
    error: Option<String>,
}

pub async fn status(state: &SharedState, service: &Service) -> Result<Status> {
    let mut status = Status {
        destination_id: service.pitr_destination_id,
        base_backup_at: service.pitr_base_backup_at,
        ..Default::default()
    };
    if !archiving(service) {
        return Ok(status);
    }
    let read = async {
        let (docker, container) = cluster::running_container(state, service.id)
            .await?
            .ok_or_else(|| anyhow!("the database is not running"))?;
        let (_, listed) =
            docker::exec(&docker, &container, "postgres", "wal-g backup-list --detail --json").await?;
        let earliest = parse_backups(&listed).ok().and_then(|b| b.first().map(|b| b.finish_time));
        let script = format!(
            r#"{CONNECT}; psql -tAF'|' -c "SELECT coalesce(last_archived_time::text, ''), failed_count, coalesce(last_failed_time::text, '') FROM pg_stat_archiver""#
        );
        let (code, row) = docker::exec(&docker, &container, "postgres", &script).await?;
        if code != 0 {
            bail!("{}", last_lines(&row));
        }
        anyhow::Ok((earliest, row))
    }
    .await;
    match read {
        Ok((earliest, row)) => {
            let fields: Vec<&str> = row.trim().split('|').collect();
            let time = |s: Option<&&str>| {
                s.and_then(|s| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%#z").ok())
                    .map(|t| t.with_timezone(&Utc))
            };
            status.earliest = earliest;
            status.archived_at = time(fields.first());
            status.failed_count = fields.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            status.failed_at = time(fields.get(2));
        }
        Err(e) => status.error = Some(format!("{e:#}")),
    }
    Ok(status)
}

/// A new database: `source` as it was at `target`, beside it in the same project.
pub async fn restore(
    state: &SharedState,
    source: &Service,
    target: DateTime<Utc>,
) -> Result<Service> {
    if !archiving(source) {
        return Err(bad_request(
            "turn on point-in-time recovery for this database first",
        ));
    }
    if target > Utc::now() - chrono::Duration::minutes(1) {
        return Err(bad_request(
            "pick a moment at least a minute ago; newer changes may not be archived yet",
        ));
    }
    let env: std::collections::BTreeMap<String, String> =
        services::env(state, source.id).await?.into_iter().collect();
    let new = services::NewService {
        name: format!("{}-at-{}", source.name, target.format("%m%d-%H%M")),
        kind: "database".into(),
        engine: source.engine.clone(),
        env,
        restore: Some((source.id, target)),
        ..Default::default()
    };
    services::create_service(state, source.project_id, new).await
}

/// Before a restored database first starts: fill its volume from the source's
/// archive, replaying the log up to the target, in a helper on this server.
pub async fn prepare(
    state: &SharedState,
    service: &Service,
    volumes: &[Volume],
    log: impl Fn(String),
) -> anyhow::Result<()> {
    let (Some(source_id), Some(target)) = (service.restore_from, service.restore_target) else {
        return Ok(());
    };
    let source = services::get_service(&state.db, source_id)
        .await
        .map_err(|e| anyhow!("the database it's restored from: {e}"))?;
    let destination = source
        .pitr_destination_id
        .ok_or_else(|| anyhow!("{} no longer archives its log", source.name))?;
    let volume = volumes
        .first()
        .ok_or_else(|| anyhow!("the database has no volume"))?;
    let mut env: HashMap<String, String> = services::env(state, service.id)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .into_iter()
        .collect();
    env.extend(walg_env(state, source.id, destination).await?);
    let image = state.config.postgres_image.clone();
    if !docker::image_exists(&state.docker, &image).await {
        docker::pull_image(&state.docker, &image, |_| {}).await?;
    }

    log(format!(
        "==> Restoring {} as it was at {target}",
        source.name
    ));
    let mut listed = String::new();
    let body = helper(&image, "wal-g backup-list --detail --json", &env, None);
    let name = format!("jd-pitr-{}", &service.id.simple().to_string()[..12]);
    docker::run_to_completion(&state.docker, &name, body, Duration::from_secs(300), |l| {
        listed.push_str(&l);
        listed.push('\n');
    })
    .await?;
    let backups = parse_backups(&listed)?;
    let base = backups
        .iter()
        .rev()
        .find(|b| b.finish_time <= target)
        .ok_or_else(|| match backups.first() {
            Some(first) => anyhow!(
                "the earliest moment it can be restored to is {} (UTC)",
                first.finish_time
            ),
            None => anyhow!("there is no base backup yet; the first runs within ten minutes of turning recovery on"),
        })?;
    log(format!("Starting from base backup {}", base.backup_name));

    env.insert("JD_BACKUP".into(), base.backup_name.clone());
    env.insert("JD_TARGET".into(), target.to_rfc3339());
    let body = helper(&image, RECOVER, &env, Some(&volume.name));
    let code = docker::run_to_completion(
        &state.docker,
        &name,
        body,
        Duration::from_secs(6 * 60 * 60),
        |line| log(format!("    {line}")),
    )
    .await?;
    if code != 0 {
        bail!("restoring exited with code {code}");
    }
    sqlx::query!(
        "UPDATE services SET restore_from = NULL, restore_target = NULL WHERE id = $1",
        service.id
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Fetches the base backup, replays the log to the target with Postgres not
/// listening, waits for it to finish and promote, then stops it cleanly.
const RECOVER: &str = r#"
set -eu
D=/var/lib/postgresql/data
if [ -f "$D/.jd-restored" ]; then echo "Already restored"; exit 0; fi
# Whatever an interrupted attempt left goes; the restore starts over.
find "$D" -mindepth 1 -delete
wal-g backup-fetch "$D" "$JD_BACKUP"
chown -R postgres:postgres "$D" && chmod 700 "$D"
touch "$D/recovery.signal"
printf "restore_command = 'wal-g wal-fetch %%f %%p'\nrecovery_target_time = '%s'\nrecovery_target_action = 'promote'\n" "$JD_TARGET" >> "$D/postgresql.auto.conf"
echo "Replaying changes up to $JD_TARGET"
gosu postgres pg_ctl -D "$D" -o "-c listen_addresses=''" -w -t 3600 start
until gosu postgres psql -h /var/run/postgresql -U "$POSTGRES_USER" -d postgres -tAc 'SELECT pg_is_in_recovery()' 2>/dev/null | grep -q f; do
  if ! gosu postgres pg_ctl -D "$D" status >/dev/null; then
    echo "Postgres stopped before reaching the target; the log above says why"; exit 1
  fi
  sleep 1
done
gosu postgres pg_ctl -D "$D" -m fast -w stop
sed -i '/^restore_command\|^recovery_target/d' "$D/postgresql.auto.conf"
touch "$D/.jd-restored"
echo "Recovered to $JD_TARGET"
"#;

fn helper(
    image: &str,
    script: &str,
    env: &HashMap<String, String>,
    volume: Option<&str>,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_string()),
        entrypoint: Some(vec!["sh".into(), "-c".into()]),
        cmd: Some(vec![script.to_string()]),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        labels: Some(HashMap::from([(MANAGED.to_string(), "helper".to_string())])),
        host_config: Some(HostConfig {
            binds: volume.map(|v| vec![format!("{v}:/var/lib/postgresql/data")]),
            log_config: Some(docker::spec::bounded_logs()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_lists_parse_after_log_noise() {
        let json = r#"INFO: 2026/11/18 10:00:00.000000 List backups from storages: [default]
[{"backup_name":"base_000000010000000000000004","time":"2026-11-18T10:00:00Z","finish_time":"2026-11-18T10:00:02Z","wal_file_name":"x"},
 {"backup_name":"base_000000010000000000000002","time":"2026-11-17T10:00:00Z","finish_time":"2026-11-17T10:00:02Z","wal_file_name":"y"}]"#;
        let backups = parse_backups(json).unwrap();
        assert_eq!(
            backups[0].backup_name, "base_000000010000000000000002",
            "oldest first"
        );
        assert!(parse_backups("ERROR: no backups found").is_err());
    }

    #[test]
    fn archiving_runs_wal_g_on_every_segment() {
        let args = archive_args();
        assert_eq!(args[0], "postgres");
        assert!(args.contains(&"archive_command=wal-g wal-push %p".to_string()));
    }
}
