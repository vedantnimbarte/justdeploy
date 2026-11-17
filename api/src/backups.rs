//! Database backups: destinations (S3-compatible or a local directory),
//! per-database schedules, dump and restore.
//!
//! A dump runs inside the database's own container with `docker exec` and is
//! gzipped and streamed straight to the destination, so it never has to fit in
//! memory or on the control plane's disk.

use crate::{
    docker::{self, spec, LABEL_KIND, LABEL_SERVICE},
    engines,
    error::{bad_request, Error, Result},
    models::Service,
    queue::{self, Job},
    services,
    state::SharedState,
};
use anyhow::{anyhow, bail, Context};
use async_compression::tokio::write::GzipEncoder;
use bollard::{
    container::LogOutput,
    exec::{CreateExecOptions, StartExecResults},
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use object_store::{
    aws::AmazonS3Builder, buffered::BufWriter, local::LocalFileSystem, path::Path, ObjectStore,
    ObjectStoreExt,
};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

// ---------------------------------------------------------------- destinations

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DestinationConfig {
    S3 {
        /// Leave empty for AWS; set for R2, B2, MinIO and the like.
        #[serde(default)]
        endpoint: String,
        #[serde(default = "default_region")]
        region: String,
        bucket: String,
        access_key_id: String,
        secret_access_key: String,
        #[serde(default)]
        prefix: String,
    },
    Local {
        /// A folder under JD_BACKUP_DIR.
        path: String,
    },
}

fn default_region() -> String {
    "us-east-1".into()
}

#[derive(Serialize)]
pub struct Destination {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    /// Where it writes, without credentials.
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

impl DestinationConfig {
    fn summary(&self) -> String {
        match self {
            Self::S3 {
                endpoint,
                bucket,
                prefix,
                ..
            } => {
                let host = if endpoint.is_empty() {
                    "s3.amazonaws.com"
                } else {
                    endpoint
                };
                format!("{host}/{bucket}/{prefix}")
                    .trim_end_matches('/')
                    .to_string()
            }
            Self::Local { path } => format!("local: {path}"),
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::S3 {
                bucket,
                access_key_id,
                secret_access_key,
                ..
            } => {
                if bucket.is_empty() || access_key_id.is_empty() || secret_access_key.is_empty() {
                    return Err(bad_request("bucket, access key and secret are required"));
                }
            }
            Self::Local { path } => {
                if path.is_empty() || path.contains("..") || path.starts_with('/') {
                    return Err(bad_request("use a plain folder name, like `nightly`"));
                }
            }
        }
        Ok(())
    }
}

pub async fn list_destinations(state: &SharedState) -> Result<Vec<Destination>> {
    let rows = sqlx::query!("SELECT * FROM backup_destinations ORDER BY name")
        .fetch_all(&state.db)
        .await?;
    rows.into_iter()
        .map(|r| {
            let config: DestinationConfig =
                serde_json::from_str(&state.cipher.decrypt(&r.config_enc)?)
                    .map_err(anyhow::Error::from)?;
            Ok(Destination {
                id: r.id,
                name: r.name,
                kind: r.kind,
                summary: config.summary(),
                created_at: r.created_at,
            })
        })
        .collect()
}

/// Saves a destination after proving it can be written to.
pub async fn create_destination(
    state: &SharedState,
    name: &str,
    config: DestinationConfig,
) -> Result<Uuid> {
    if name.trim().is_empty() {
        return Err(bad_request("give the destination a name"));
    }
    config.validate()?;
    let (store, prefix) = open_store(state, &config)?;
    let probe = Path::from(format!("{prefix}.justdeploy-write-test").trim_start_matches('/'));
    match tokio::time::timeout(Duration::from_secs(15), store.put(&probe, "ok".into())).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(bad_request(format!("cannot write to the destination: {e}"))),
        Err(_) => {
            return Err(bad_request(
                "the destination did not answer within 15 seconds",
            ))
        }
    }
    let _ = store.delete(&probe).await;

    let kind = match config {
        DestinationConfig::S3 { .. } => "s3",
        DestinationConfig::Local { .. } => "local",
    };
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO backup_destinations (id, name, kind, config_enc) VALUES ($1, $2, $3, $4)",
        id,
        name.trim(),
        kind,
        state
            .cipher
            .encrypt(&serde_json::to_string(&config).map_err(anyhow::Error::from)?)
    )
    .execute(&state.db)
    .await?;
    Ok(id)
}

pub async fn delete_destination(state: &SharedState, id: Uuid) -> Result<()> {
    let removed = sqlx::query!("DELETE FROM backup_destinations WHERE id = $1", id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

async fn load_destination(state: &SharedState, id: Uuid) -> anyhow::Result<DestinationConfig> {
    let enc = sqlx::query_scalar!(
        "SELECT config_enc FROM backup_destinations WHERE id = $1",
        id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow!("the backup destination was deleted"))?;
    Ok(serde_json::from_str(&state.cipher.decrypt(&enc)?)?)
}

/// The store and the key prefix (empty or ending in `/`).
fn open_store(
    state: &SharedState,
    config: &DestinationConfig,
) -> Result<(Arc<dyn ObjectStore>, String)> {
    match config {
        DestinationConfig::S3 {
            endpoint,
            region,
            bucket,
            access_key_id,
            secret_access_key,
            prefix,
        } => {
            let mut builder = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_region(region)
                .with_access_key_id(access_key_id)
                .with_secret_access_key(secret_access_key);
            if !endpoint.is_empty() {
                builder = builder
                    .with_endpoint(endpoint)
                    .with_allow_http(endpoint.starts_with("http://"));
            }
            let store = builder
                .build()
                .map_err(|e| bad_request(format!("invalid S3 settings: {e}")))?;
            let prefix = prefix.trim_matches('/');
            let prefix = if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}/")
            };
            Ok((Arc::new(store), prefix))
        }
        DestinationConfig::Local { path } => {
            let dir = state.config.backup_dir.join(path);
            std::fs::create_dir_all(&dir)
                .map_err(|e| bad_request(format!("cannot create {}: {e}", dir.display())))?;
            let store = LocalFileSystem::new_with_prefix(&dir).map_err(anyhow::Error::from)?;
            Ok((Arc::new(store), String::new()))
        }
    }
}

// ---------------------------------------------------------------- schedules

#[derive(Serialize, Deserialize)]
pub struct Schedule {
    pub destination_id: Uuid,
    /// Standard five-field cron, in UTC: `0 3 * * *` is 03:00 every day.
    pub cron: String,
    pub keep: i32,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
}

/// The cron crate wants a seconds field; people write five fields.
fn parse_cron(expr: &str) -> Result<cron::Schedule> {
    let fields = expr.split_whitespace().count();
    if fields != 5 {
        return Err(bad_request(
            "use a five-field cron expression, like `0 3 * * *`",
        ));
    }
    cron::Schedule::from_str(&format!("0 {expr}"))
        .map_err(|e| bad_request(format!("invalid cron expression: {e}")))
}

pub async fn get_schedule(state: &SharedState, service_id: Uuid) -> Result<Option<Schedule>> {
    Ok(sqlx::query_as!(
        Schedule,
        "SELECT destination_id, cron, keep, last_run_at FROM backup_schedules WHERE service_id = $1",
        service_id
    )
    .fetch_optional(&state.db)
    .await?)
}

pub async fn set_schedule(
    state: &SharedState,
    service: &Service,
    schedule: Option<Schedule>,
) -> Result<()> {
    let Some(schedule) = schedule else {
        sqlx::query!(
            "DELETE FROM backup_schedules WHERE service_id = $1",
            service.id
        )
        .execute(&state.db)
        .await?;
        return Ok(());
    };
    require_database(service)?;
    parse_cron(&schedule.cron)?;
    if !(1..=365).contains(&schedule.keep) {
        return Err(bad_request("keep between 1 and 365 backups"));
    }
    sqlx::query!(
        "INSERT INTO backup_schedules (service_id, destination_id, cron, keep, last_run_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (service_id) DO UPDATE
            SET destination_id = EXCLUDED.destination_id, cron = EXCLUDED.cron, keep = EXCLUDED.keep",
        service.id,
        schedule.destination_id,
        schedule.cron.trim(),
        schedule.keep
    )
    .execute(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => bad_request("unknown destination"),
        other => other.into(),
    })?;
    Ok(())
}

/// Called every minute: queues a backup for each schedule that has come due.
pub async fn schedule_due(state: SharedState) -> Result<()> {
    let rows = sqlx::query!(
        "SELECT s.service_id, s.destination_id, s.cron, s.last_run_at
         FROM backup_schedules s JOIN services sv ON sv.id = s.service_id
         WHERE sv.desired_state = 'running'"
    )
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    for row in rows {
        let Ok(schedule) = parse_cron(&row.cron) else {
            continue;
        };
        let last = row.last_run_at.unwrap_or(now);
        let due = schedule.after(&last).next().is_some_and(|next| next <= now);
        if due {
            sqlx::query!(
                "UPDATE backup_schedules SET last_run_at = $2 WHERE service_id = $1",
                row.service_id,
                now
            )
            .execute(&state.db)
            .await?;
            let service = services::get_service(&state.db, row.service_id).await?;
            start_backup(&state, &service, row.destination_id, "schedule").await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- backups

#[derive(Serialize)]
pub struct Backup {
    pub id: Uuid,
    pub destination_id: Option<Uuid>,
    pub object_key: String,
    pub status: String,
    pub size_bytes: Option<i64>,
    pub error: Option<String>,
    pub trigger: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub restore_status: Option<String>,
    pub restore_error: Option<String>,
    pub restored_at: Option<DateTime<Utc>>,
}

fn require_database(service: &Service) -> Result<&'static engines::Engine> {
    service
        .engine
        .as_deref()
        .and_then(engines::get)
        .ok_or_else(|| bad_request("backups are for database services"))
}

pub async fn list(state: &SharedState, service_id: Uuid) -> Result<Vec<Backup>> {
    Ok(sqlx::query_as!(
        Backup,
        "SELECT id, destination_id, object_key, status, size_bytes, error, trigger, created_at,
                finished_at, restore_status, restore_error, restored_at
         FROM backups WHERE service_id = $1 ORDER BY created_at DESC LIMIT 100",
        service_id
    )
    .fetch_all(&state.db)
    .await?)
}

pub async fn start_backup(
    state: &SharedState,
    service: &Service,
    destination_id: Uuid,
    trigger: &str,
) -> Result<Uuid> {
    let engine = require_database(service)?;
    let project = services::get_project(&state.db, service.project_id).await?;
    let key = format!(
        "{}/{}/{}.{}.gz",
        project.name,
        service.name,
        Utc::now().format("%Y%m%d-%H%M%S"),
        engine.dump_ext
    );
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO backups (id, service_id, destination_id, object_key, trigger) VALUES ($1, $2, $3, $4, $5)",
        id,
        service.id,
        destination_id,
        key,
        trigger
    )
    .execute(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => bad_request("unknown destination"),
        other => other.into(),
    })?;
    queue::enqueue(&state.db, Job::Backup { backup_id: id }).await?;
    Ok(id)
}

pub async fn start_restore(state: &SharedState, backup_id: Uuid) -> Result<()> {
    let updated = sqlx::query!(
        "UPDATE backups SET restore_status = 'queued', restore_error = NULL
         WHERE id = $1 AND status = 'succeeded'",
        backup_id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(bad_request("only a completed backup can be restored"));
    }
    queue::enqueue(&state.db, Job::Restore { backup_id }).await
}

/// The running production container of a service.
async fn live_container(state: &SharedState, service: &Service) -> anyhow::Result<String> {
    let id = service.id.to_string();
    docker::list(
        &state.docker,
        &[(LABEL_SERVICE, &id), (LABEL_KIND, "production")],
    )
    .await?
    .into_iter()
    .find(docker::is_running)
    .and_then(|c| c.id)
    .ok_or_else(|| anyhow!("the database is not running"))
}

pub async fn run_backup(state: &SharedState, backup_id: Uuid) -> Result<()> {
    let row = sqlx::query!(
        "SELECT service_id, destination_id, object_key, status FROM backups WHERE id = $1",
        backup_id
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row.filter(|r| r.status == "running") else {
        return Ok(());
    };
    let outcome = async {
        let destination_id = row
            .destination_id
            .ok_or_else(|| anyhow!("the destination was deleted"))?;
        let service = services::get_service(&state.db, row.service_id).await?;
        let engine = require_database(&service)?;
        let config = load_destination(state, destination_id).await?;
        let (store, prefix) = open_store(state, &config)?;
        let path = Path::from(format!("{prefix}{}", row.object_key));
        let size = dump(state, &service, engine, store.clone(), &path).await?;
        prune(state, &service, destination_id, store).await?;
        anyhow::Ok(size)
    }
    .await;

    match outcome {
        Ok(size) => {
            sqlx::query!(
                "UPDATE backups SET status = 'succeeded', size_bytes = $2, finished_at = now() WHERE id = $1",
                backup_id,
                size as i64
            )
            .execute(&state.db)
            .await?;
        }
        Err(error) => {
            tracing::warn!(%backup_id, error = format!("{error:#}"), "backup failed");
            sqlx::query!(
                "UPDATE backups SET status = 'failed', error = $2, finished_at = now() WHERE id = $1",
                backup_id,
                format!("{error:#}")
            )
            .execute(&state.db)
            .await?;
        }
    }
    Ok(())
}

async fn dump(
    state: &SharedState,
    service: &Service,
    engine: &engines::Engine,
    store: Arc<dyn ObjectStore>,
    path: &Path,
) -> anyhow::Result<u64> {
    let container = live_container(state, service).await?;
    let command = engines::dump_command(engine.id).ok_or_else(|| anyhow!("no dump command"))?;
    let exec = state
        .docker
        .create_exec(
            &container,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec!["sh", "-c", command]),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached { mut output, .. } =
        state.docker.start_exec(&exec.id, None).await?
    else {
        bail!("could not attach to the dump");
    };

    let mut writer = GzipEncoder::new(BufWriter::new(store.clone(), path.clone()));
    let mut stderr = Vec::new();
    while let Some(chunk) = output.next().await {
        match chunk? {
            LogOutput::StdOut { message } => writer.write_all(&message).await?,
            LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
            _ => {}
        }
    }
    let code = state
        .docker
        .inspect_exec(&exec.id)
        .await?
        .exit_code
        .unwrap_or(-1);
    if code != 0 {
        drop(writer);
        let _ = store.delete(path).await;
        bail!(
            "dump exited with code {code}: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    writer.shutdown().await.context("uploading the backup")?;
    Ok(store.head(path).await?.size)
}

/// Keeps the newest `keep` backups of a service at a destination.
async fn prune(
    state: &SharedState,
    service: &Service,
    destination_id: Uuid,
    store: Arc<dyn ObjectStore>,
) -> anyhow::Result<()> {
    let keep = sqlx::query_scalar!(
        "SELECT keep FROM backup_schedules WHERE service_id = $1 AND destination_id = $2",
        service.id,
        destination_id
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(keep) = keep else { return Ok(()) };
    let config = load_destination(state, destination_id).await?;
    let (_, prefix) = open_store(state, &config).map_err(|e| anyhow!("{e}"))?;
    let stale = sqlx::query!(
        "SELECT id, object_key FROM backups
         WHERE service_id = $1 AND destination_id = $2 AND status = 'succeeded'
         ORDER BY created_at DESC OFFSET $3",
        service.id,
        destination_id,
        // The backup that just finished is still marked running, so it isn't counted here.
        (keep - 1).max(0) as i64
    )
    .fetch_all(&state.db)
    .await?;
    for backup in stale {
        let _ = store
            .delete(&Path::from(format!("{prefix}{}", backup.object_key)))
            .await;
        sqlx::query!("DELETE FROM backups WHERE id = $1", backup.id)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}

pub async fn run_restore(state: &SharedState, backup_id: Uuid) -> Result<()> {
    let row = sqlx::query!(
        "SELECT service_id, destination_id, object_key FROM backups WHERE id = $1",
        backup_id
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else { return Ok(()) };
    sqlx::query!(
        "UPDATE backups SET restore_status = 'running' WHERE id = $1",
        backup_id
    )
    .execute(&state.db)
    .await?;

    let outcome = async {
        let destination_id = row
            .destination_id
            .ok_or_else(|| anyhow!("the destination was deleted"))?;
        let service = services::get_service(&state.db, row.service_id).await?;
        let engine = require_database(&service)?;
        let config = load_destination(state, destination_id).await?;
        let (store, prefix) = open_store(state, &config)?;
        let path = Path::from(format!("{prefix}{}", row.object_key));
        if engine.id == "redis" {
            restore_redis(state, &service, store, &path).await
        } else {
            let container = live_container(state, &service).await?;
            let command =
                engines::restore_command(engine.id).ok_or_else(|| anyhow!("no restore command"))?;
            restore_into(state, &container, command, store, &path).await
        }
    }
    .await;

    let (status, error) = match &outcome {
        Ok(()) => ("succeeded", None),
        Err(e) => ("failed", Some(format!("{e:#}"))),
    };
    sqlx::query!(
        "UPDATE backups SET restore_status = $2, restore_error = $3, restored_at = now() WHERE id = $1",
        backup_id,
        status,
        error
    )
    .execute(&state.db)
    .await?;
    services::audit(
        &state.db,
        &format!("backup.restore.{status}"),
        &row.object_key,
        error.as_deref(),
    )
    .await;
    Ok(())
}

const RESTORE_FILE: &str = "/tmp/jd-restore.gz";

/// Copies a gzipped backup into a container, then runs `gzip -dc file | command`.
///
/// The file goes in through Docker's archive upload, a request of known length,
/// rather than over an exec's stdin: ending stdin needs a half-close that the
/// Windows named pipe (Docker Desktop) does not deliver, which leaves the restore
/// command waiting forever. Every database image ships gzip.
async fn restore_into(
    state: &SharedState,
    container: &str,
    command: &str,
    store: Arc<dyn ObjectStore>,
    path: &Path,
) -> anyhow::Result<()> {
    let object = store.get(path).await?;
    let size = object.meta.size;
    let header = tar_header("jd-restore.gz", size)?;
    let padding = (512 - (size % 512) as usize) % 512;
    // A tar archive: header, the file, padding to a block, two empty blocks.
    let body = futures::stream::once(async move { bytes::Bytes::copy_from_slice(&header) })
        .chain(
            object
                .into_stream()
                .take_while(|chunk| futures::future::ready(chunk.is_ok()))
                .map(|chunk| chunk.unwrap_or_default()),
        )
        .chain(futures::stream::once(async move {
            bytes::Bytes::from(vec![0u8; padding + 1024])
        }));
    let options = bollard::query_parameters::UploadToContainerOptionsBuilder::new()
        .path("/tmp")
        .build();
    state
        .docker
        .upload_to_container(container, Some(options), bollard::body_stream(body))
        .await
        .context("copying the backup into the container")?;

    let script = format!(
        "gzip -dc {RESTORE_FILE} | {command}; status=$?; rm -f {RESTORE_FILE}; exit $status"
    );
    let exec = state
        .docker
        .create_exec(
            container,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec!["sh", "-c", script.as_str()]),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached { mut output, .. } =
        state.docker.start_exec(&exec.id, None).await?
    else {
        bail!("could not attach to the restore");
    };
    let mut text = Vec::new();
    let collect = async {
        while let Some(Ok(chunk)) = output.next().await {
            text.extend_from_slice(&docker::output_bytes(&chunk));
        }
    };
    tokio::time::timeout(Duration::from_secs(60 * 60), collect)
        .await
        .context("the restore did not finish within an hour")?;
    let code = state
        .docker
        .inspect_exec(&exec.id)
        .await?
        .exit_code
        .unwrap_or(-1);
    if code != 0 {
        bail!(
            "restore exited with code {code}: {}",
            String::from_utf8_lossy(&text).trim()
        );
    }
    Ok(())
}

/// A ustar header for one regular file.
// ponytail: the octal size field tops out at 8 GiB compressed; use the GNU
// base-256 size encoding if backups grow past that.
fn tar_header(name: &str, size: u64) -> anyhow::Result<[u8; 512]> {
    if size > 0o77777777777 {
        bail!("backups over 8 GiB compressed cannot be restored this way yet");
    }
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

/// Redis loads its data from dump.rdb at start, so: stop it, swap the file using
/// a helper with the volume mounted, and start it again.
async fn restore_redis(
    state: &SharedState,
    service: &Service,
    store: Arc<dyn ObjectStore>,
    path: &Path,
) -> anyhow::Result<()> {
    let container = live_container(state, service).await?;
    let volume = services::volumes(&state.db, service.id)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("redis has no data volume"))?;

    docker::stop(&state.docker, &container).await?;
    let helper_name = format!("jd-restore-{}", &service.id.simple().to_string()[..12]);
    let mut body = spec::helper(
        &state.config.builder_image,
        &["sleep", "3600"],
        vec![],
        None,
        false,
    );
    if let Some(host) = body.host_config.as_mut() {
        host.binds = Some(vec![format!("{}:/data", volume.name)]);
    }
    let helper = docker::create_and_start(&state.docker, &helper_name, body).await?;
    let result = restore_into(state, &helper, "cat > /data/dump.rdb", store, path).await;
    docker::remove(&state.docker, &helper).await?;
    docker::start(&state.docker, &container).await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_headers_carry_size_and_a_valid_checksum() {
        let h = tar_header("jd-restore.gz", 1234).unwrap();
        assert_eq!(&h[124..136], b"00000002322\0", "1234 in octal");
        let mut blank = h;
        blank[148..156].copy_from_slice(b"        ");
        let sum: u32 = blank.iter().map(|&b| b as u32).sum();
        let stored = std::str::from_utf8(&h[148..154]).unwrap();
        assert_eq!(u32::from_str_radix(stored, 8).unwrap(), sum);
        assert!(tar_header("x", 1 << 34).is_err());
    }

    #[test]
    fn five_field_cron_is_accepted_and_others_are_not() {
        assert!(parse_cron("0 3 * * *").is_ok());
        assert!(parse_cron("*/15 * * * *").is_ok());
        assert!(parse_cron("0 0 3 * * *").is_err());
        assert!(parse_cron("nonsense").is_err());
    }

    #[test]
    fn destinations_never_show_credentials() {
        let config = DestinationConfig::S3 {
            endpoint: "https://minio.example.com".into(),
            region: "us-east-1".into(),
            bucket: "backups".into(),
            access_key_id: "AKIA".into(),
            secret_access_key: "very-secret".into(),
            prefix: "jd".into(),
        };
        let summary = config.summary();
        assert_eq!(summary, "https://minio.example.com/backups/jd");
        assert!(!summary.contains("very-secret"));
    }

    #[test]
    fn local_paths_cannot_escape_the_backup_dir() {
        assert!(DestinationConfig::Local {
            path: "../etc".into()
        }
        .validate()
        .is_err());
        assert!(DestinationConfig::Local {
            path: "/etc".into()
        }
        .validate()
        .is_err());
        assert!(DestinationConfig::Local {
            path: "nightly".into()
        }
        .validate()
        .is_ok());
    }
}
