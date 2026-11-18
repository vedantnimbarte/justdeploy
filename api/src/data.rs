//! Moving data between databases: a copy of a database as a new one, replacing
//! a database's data with another's (a point-in-time copy back over the
//! original, say), and PostgreSQL read replicas.

use crate::{
    backups, cluster,
    docker::{self, MANAGED},
    error::{bad_request, Error, Result},
    models::{Service, Volume},
    services,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use bollard::models::{ContainerCreateBody, HostConfig};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

#[derive(Serialize)]
pub struct Copy {
    id: Uuid,
    source_id: Option<Uuid>,
    source: Option<String>,
    target_id: Uuid,
    /// pending | running | succeeded | failed
    status: String,
    error: Option<String>,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

/// The newest copies into a database.
pub async fn list(state: &SharedState, target: Uuid) -> Result<Vec<Copy>> {
    Ok(sqlx::query_as!(
        Copy,
        r#"SELECT c.id, c.source_id, s.name AS "source?", c.target_id, c.status, c.error,
                  c.created_at, c.finished_at
           FROM data_copies c LEFT JOIN services s ON s.id = c.source_id
           WHERE c.target_id = $1 ORDER BY c.created_at DESC LIMIT 10"#,
        target
    )
    .fetch_all(&state.db)
    .await?)
}

fn engine(service: &Service) -> Result<&str> {
    service
        .engine
        .as_deref()
        .filter(|_| service.kind == "database")
        .ok_or_else(|| bad_request(format!("{} is not a database", service.name)))
}

/// The first free name of `base`, `base-2`, `base-3`… in the project.
async fn free_name(state: &SharedState, project: Uuid, base: &str) -> Result<String> {
    let taken: Vec<String> =
        sqlx::query_scalar!("SELECT name FROM services WHERE project_id = $1", project)
            .fetch_all(&state.db)
            .await?;
    Ok((1..)
        .map(|n| match n {
            1 => base.to_string(),
            n => format!("{base}-{n}"),
        })
        .find(|name| !taken.contains(name))
        .unwrap_or_default())
}

/// A new database beside `source`: same engine, image and credentials, filled
/// with `source`'s data once it first runs.
pub async fn create_copy(state: &SharedState, source: &Service) -> Result<Service> {
    engine(source)?;
    if source.current_deployment_id.is_none() || source.desired_state != "running" {
        return Err(bad_request(format!("start {} first", source.name)));
    }
    let new = services::NewService {
        name: free_name(state, source.project_id, &format!("{}-copy", source.name)).await?,
        kind: "database".into(),
        engine: source.engine.clone(),
        image: source.image.clone(),
        env: services::env(state, source.id).await?.into_iter().collect(),
        copy_from: Some(source.id),
        ..Default::default()
    };
    services::create_service(state, source.project_id, new).await
}

/// Replaces `target`'s data with `source`'s, in the background. Tables (or
/// collections, or for Redis everything) in the dump are dropped and recreated.
pub async fn replace(state: &SharedState, target: &Service, source: &Service) -> Result<Uuid> {
    if engine(target)? != engine(source)? {
        return Err(bad_request("both databases have to be the same engine"));
    }
    if target.id == source.id {
        return Err(bad_request("pick another database to copy from"));
    }
    if target.replica_of.is_some() {
        return Err(bad_request(
            "a read replica follows its primary; change the primary instead",
        ));
    }
    let busy = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM data_copies
           WHERE target_id = $1 AND status IN ('pending', 'running')"#,
        target.id
    )
    .fetch_one(&state.db)
    .await?;
    if busy > 0 {
        return Err(bad_request(format!(
            "{} is already being filled; wait for that to finish",
            target.name
        )));
    }
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO data_copies (id, source_id, target_id) VALUES ($1, $2, $3)",
        id,
        source.id,
        target.id
    )
    .execute(&state.db)
    .await?;
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run(&state, id, |_| {}).await {
            tracing::warn!(%error, "copying data failed");
        }
    });
    Ok(id)
}

/// After a database's deploy: the copies waiting to fill it.
pub async fn fill_pending(
    state: &SharedState,
    service: &Service,
    log: impl Fn(String),
) -> anyhow::Result<()> {
    let pending = sqlx::query_scalar!(
        "SELECT id FROM data_copies WHERE target_id = $1 AND status = 'pending' ORDER BY created_at",
        service.id
    )
    .fetch_all(&state.db)
    .await?;
    for id in pending {
        run(state, id, &log).await.map_err(|e| anyhow!("{e}"))?;
    }
    Ok(())
}

/// Copies one pending row's data and records how it went.
async fn run(state: &SharedState, id: Uuid, log: impl Fn(String)) -> Result<()> {
    let row = sqlx::query!(
        "UPDATE data_copies SET status = 'running' WHERE id = $1 AND status = 'pending'
         RETURNING source_id, target_id",
        id
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else { return Ok(()) };
    let target = services::get_service(&state.db, row.target_id).await?;
    let outcome = async {
        let source_id = row
            .source_id
            .ok_or_else(|| anyhow!("the database to copy from was deleted"))?;
        let source = services::get_service(&state.db, source_id)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        log(format!(
            "==> Copying {}'s data into {}",
            source.name, target.name
        ));
        let size = backups::copy_between(state, &source, &target).await?;
        log(format!(
            "Copied {:.1} MB (compressed)",
            size as f64 / 1_048_576.0
        ));
        anyhow::Ok(source.name)
    }
    .await;
    let (status, error) = match &outcome {
        Ok(_) => ("succeeded", None),
        Err(e) => ("failed", Some(format!("{e:#}"))),
    };
    sqlx::query!(
        "UPDATE data_copies SET status = $2, error = $3, finished_at = now() WHERE id = $1",
        id,
        status,
        error
    )
    .execute(&state.db)
    .await?;
    let phrase = match &outcome {
        Ok(source) => format!("filled with {source}'s data:"),
        Err(_) => "failed to fill with another database's data:".to_string(),
    };
    services::audit(&state.db, None, &phrase, &target.name, error.as_deref()).await;
    if let Err(e) = outcome {
        log(format!("Copying failed: {e:#}"));
        return Err(Error::Internal(e));
    }
    Ok(())
}

// ---------------------------------------------------------------- read replicas

/// A read-only PostgreSQL copy of `primary` that follows every change through
/// streaming replication. Its own URL is for reads only.
pub async fn create_replica(state: &SharedState, primary: &Service) -> Result<Service> {
    if engine(primary)? != "postgres" {
        return Err(bad_request("read replicas are for PostgreSQL databases"));
    }
    if primary.replica_of.is_some() {
        return Err(bad_request(
            "this is a replica itself; add replicas to its primary",
        ));
    }
    if primary.current_deployment_id.is_none() || primary.desired_state != "running" {
        return Err(bad_request(format!("start {} first", primary.name)));
    }
    let mut env: std::collections::BTreeMap<String, String> = services::env(state, primary.id)
        .await?
        .into_iter()
        .collect();
    // The replica's own Postgres signs in to the primary with it to follow it.
    let password = env.get("POSTGRES_PASSWORD").cloned().unwrap_or_default();
    env.insert("PGPASSWORD".into(), password);
    let new = services::NewService {
        name: free_name(
            state,
            primary.project_id,
            &format!("{}-replica", primary.name),
        )
        .await?,
        kind: "database".into(),
        engine: primary.engine.clone(),
        image: primary.image.clone(),
        env,
        replica_of: Some(primary.id),
        ..Default::default()
    };
    services::create_service(state, primary.project_id, new).await
}

/// The replication slot that keeps a replica's changes on its primary until
/// it has them.
pub fn slot(replica: Uuid) -> String {
    format!("jd_replica_{}", &replica.simple().to_string()[..12])
}

/// Lets `slot` replicate from a running Postgres container: a replication rule
/// in pg_hba.conf, a cap of 10 GB on what a slow replica can hold back, and
/// the slot. Safe to repeat.
pub async fn allow_replication(
    docker: &bollard::Docker,
    container: &str,
    slot: &str,
) -> anyhow::Result<()> {
    let script = format!(
        r#"set -eu
f="$PGDATA/pg_hba.conf"
grep -q '^host replication all all' "$f" || echo 'host replication all all scram-sha-256' >> "$f"
psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -qAt \
  -c "ALTER SYSTEM SET max_slot_wal_keep_size = '10GB'" \
  -c "SELECT pg_reload_conf()" \
  -c "SELECT pg_create_physical_replication_slot('{slot}') WHERE NOT EXISTS (SELECT 1 FROM pg_replication_slots WHERE slot_name = '{slot}')""#
    );
    let (code, output) = docker::exec(docker, container, "root", &script).await?;
    if code != 0 {
        bail!("preparing the primary for replication: {}", output.trim());
    }
    Ok(())
}

/// Copies the primary's data directory into an empty volume, set up to follow
/// it. `$JD_PRIMARY_HOST`, `$JD_PRIMARY_PORT`, `$JD_SLOT` and the image's
/// `$POSTGRES_USER` / `$PGPASSWORD` say where from.
pub const BASE_BACKUP: &str = r#"
set -eu
D=/var/lib/postgresql/data
if [ ! -s "$D/PG_VERSION" ]; then
  mkdir -p "$D"
  until find "$D" -mindepth 1 -delete && chown postgres:postgres "$D" && chmod 700 "$D" &&
    gosu postgres pg_basebackup -h "$JD_PRIMARY_HOST" -p "$JD_PRIMARY_PORT" -U "$POSTGRES_USER" \
      -D "$D" -R -X stream -S "$JD_SLOT"; do
    echo "Copying from the primary failed; trying again in 15 seconds"
    sleep 15
  done
  echo "Copied the primary; following it from now on"
fi
"#;

/// Before a replica's first start: the primary allows it, and its volume gets
/// the primary's data, in a helper on this server's project network.
pub async fn prepare_replica(
    state: &SharedState,
    service: &Service,
    volumes: &[Volume],
    network: &str,
    image: &str,
    log: impl Fn(String),
) -> anyhow::Result<()> {
    let Some(primary_id) = service.replica_of else {
        return Ok(());
    };
    let primary = services::get_service(&state.db, primary_id)
        .await
        .map_err(|e| anyhow!("its primary: {e}"))?;
    let (docker, container) = cluster::running_container(state, primary.id)
        .await?
        .ok_or_else(|| anyhow!("{} isn't running", primary.name))?;
    let slot = slot(service.id);
    allow_replication(&docker, &container, &slot).await?;
    let volume = volumes
        .first()
        .ok_or_else(|| anyhow!("the replica has no volume"))?;
    let mut env: HashMap<String, String> = services::env(state, service.id)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .into_iter()
        .collect();
    env.insert("JD_PRIMARY_HOST".into(), primary.name.clone());
    env.insert("JD_PRIMARY_PORT".into(), "5432".into());
    env.insert("JD_SLOT".into(), slot);
    log(format!(
        "==> Making sure {} follows {}",
        service.name, primary.name
    ));
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        entrypoint: Some(vec!["sh".into(), "-c".into()]),
        cmd: Some(vec![BASE_BACKUP.to_string()]),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        labels: Some(HashMap::from([(MANAGED.to_string(), "helper".to_string())])),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{}:/var/lib/postgresql/data", volume.name)]),
            network_mode: Some(network.to_string()),
            log_config: Some(docker::spec::bounded_logs()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let name = format!("jd-replica-{}", &service.id.simple().to_string()[..12]);
    let code = docker::run_to_completion(
        &state.docker,
        &name,
        body,
        Duration::from_secs(12 * 60 * 60),
        |line| log(format!("    {line}")),
    )
    .await?;
    if code != 0 {
        bail!("copying the primary exited with code {code}");
    }
    Ok(())
}

/// When a replica goes, its slot on the primary goes too, so the primary stops
/// keeping changes for it: once the replica has disconnected, within half a
/// minute. Best effort: the primary may be gone or down.
pub async fn forget_replica(state: &SharedState, replica: &Service) {
    let Some(primary) = replica.replica_of else {
        return;
    };
    let Ok(Some((docker, container))) = cluster::running_container(state, primary).await else {
        return;
    };
    let slot = slot(replica.id);
    let script = format!(
        r#"for i in $(seq 1 30); do
  psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -qAt -c "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = '{slot}' AND NOT active" >/dev/null
  psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -qAt -c "SELECT count(*) FROM pg_replication_slots WHERE slot_name = '{slot}'" | grep -qx 0 && exit 0
  sleep 1
done
exit 1"#
    );
    if let Err(error) = docker::exec(&docker, &container, "root", &script).await {
        tracing::warn!(%error, "could not drop the replica's slot");
    }
}

#[derive(Serialize)]
pub struct Replica {
    id: Uuid,
    name: String,
    /// streaming | catchup | startup | backup | stopped (no connection)
    state: String,
    /// Bytes the replica has yet to replay.
    lag_bytes: Option<i64>,
    /// How far behind its last replayed change is, in seconds.
    lag_seconds: Option<f64>,
}

/// A primary's replicas and how far behind each is, as the primary sees them.
pub async fn replicas(state: &SharedState, primary: &Service) -> Result<Vec<Replica>> {
    let rows = sqlx::query!(
        "SELECT id, name FROM services WHERE replica_of = $1 AND desired_state <> 'deleting' ORDER BY name",
        primary.id
    )
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let mut seen: HashMap<String, (String, Option<i64>, Option<f64>)> = HashMap::new();
    if let Ok(Some((docker, container))) = cluster::running_container(state, primary.id).await {
        let script = r#"psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -qAt -F '|' -c "SELECT s.slot_name, r.state, pg_wal_lsn_diff(pg_current_wal_lsn(), r.replay_lsn)::bigint, extract(epoch FROM r.replay_lag) FROM pg_replication_slots s JOIN pg_stat_replication r ON r.pid = s.active_pid""#;
        if let Ok((0, output)) = docker::exec(&docker, &container, "root", script).await {
            for line in output.lines() {
                let f: Vec<&str> = line.split('|').collect();
                if let [slot, state, lag, seconds] = f[..] {
                    seen.insert(
                        slot.to_string(),
                        (state.to_string(), lag.parse().ok(), seconds.parse().ok()),
                    );
                }
            }
        }
    }
    Ok(rows
        .into_iter()
        .map(|r| {
            let (state, lag_bytes, lag_seconds) = seen
                .remove(&slot(r.id))
                .unwrap_or_else(|| ("stopped".into(), None, None));
            Replica {
                id: r.id,
                name: r.name,
                state,
                lag_bytes,
                lag_seconds,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_valid_postgres_names() {
        let s = slot(Uuid::new_v4());
        assert!(s.len() <= 63);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    }
}
