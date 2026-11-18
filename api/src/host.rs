//! Whole-server resources and Docker disk usage.
//!
//! In production the control plane runs in a container and cannot see the
//! server's network interfaces or its disk. A small idle "agent" container on the
//! host network reads `/proc` and `df` for it, through `docker exec`. The same
//! path works on Docker Desktop, where it reports the Docker VM.

use crate::{
    cluster,
    docker::{self, spec, MANAGED},
    error::Result,
    state::SharedState,
};
use anyhow::{anyhow, bail};
use bollard::{
    exec::{CreateExecOptions, StartExecResults},
    models::{ContainerCreateBody, HostConfig, RestartPolicy, RestartPolicyNameEnum},
    Docker,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, RwLock},
};

const AGENT: &str = "jd-agent";

/// Each server's previous `/proc/stat` CPU counters (total, idle), for the next delta.
static LAST_CPU: Mutex<Option<HashMap<String, (u64, u64)>>> = Mutex::new(None);
/// The last `docker system df`, refreshed every ten minutes.
static DOCKER_DISK: RwLock<Option<DockerDisk>> = RwLock::new(None);

// ---------------------------------------------------------------- parsing

/// (total, idle) jiffies from the aggregate `cpu` line of /proc/stat.
fn parse_cpu(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal; guest time is already in user.
    let total = values.iter().take(8).sum();
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some((total, idle))
}

/// (used, total) bytes; "used" excludes cache the kernel can reclaim.
fn parse_memory(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        meminfo
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    Some((total.saturating_sub(available), total))
}

/// Received and sent bytes summed over physical interfaces.
fn parse_network(dev: &str) -> (u64, u64) {
    dev.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(iface, _)| {
            let iface = iface.trim();
            !(iface == "lo"
                || iface.starts_with("veth")
                || iface.starts_with("docker")
                || iface.starts_with("br-")
                || iface.starts_with("cni")
                || iface.starts_with("flannel"))
        })
        .fold((0, 0), |(rx, tx), (_, counters)| {
            let c: Vec<u64> = counters
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            (
                rx + c.first().copied().unwrap_or(0),
                tx + c.get(8).copied().unwrap_or(0),
            )
        })
}

/// (used, total) bytes from `df -Pk` output.
fn parse_disk(df: &str) -> Option<(u64, u64)> {
    let fields: Vec<&str> = df
        .lines()
        .filter(|l| !l.trim().is_empty())
        .nth(1)?
        .split_whitespace()
        .collect();
    let total: u64 = fields.get(1)?.parse().ok()?;
    let used: u64 = fields.get(2)?.parse().ok()?;
    Some((used * 1024, total * 1024))
}

fn parse_uptime(uptime: &str) -> Option<u64> {
    uptime
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

// ---------------------------------------------------------------- the agent

async fn ensure_agent(state: &SharedState, docker: &Docker) -> anyhow::Result<()> {
    match docker.inspect_container(AGENT, None).await {
        Ok(found) => {
            if found.state.and_then(|s| s.running) != Some(true) {
                docker::start(docker, AGENT).await?;
            }
            return Ok(());
        }
        Err(e) if docker::is_not_found(&e) => {}
        Err(e) => return Err(e.into()),
    }
    let image = &state.config.builder_image;
    if !docker::image_exists(docker, image).await {
        if image.ends_with(":dev") {
            bail!("builder image {image} is missing");
        }
        docker::pull_image(docker, image, |_| {}).await?;
    }
    let body = ContainerCreateBody {
        image: Some(image.clone()),
        cmd: Some(vec!["sleep".into(), "infinity".into()]),
        labels: Some(HashMap::from([(MANAGED.to_string(), "agent".to_string())])),
        host_config: Some(HostConfig {
            // The host's network namespace, so /proc/net/dev lists its interfaces.
            network_mode: Some("host".into()),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            memory: Some(16 * 1024 * 1024),
            log_config: Some(spec::bounded_logs()),
            ..Default::default()
        }),
        ..Default::default()
    };
    docker::create_and_start(docker, AGENT, body).await?;
    Ok(())
}

async fn read_proc(state: &SharedState, docker: &Docker) -> anyhow::Result<String> {
    ensure_agent(state, docker).await?;
    let script = "cat /proc/stat; echo '#---'; cat /proc/meminfo; echo '#---'; cat /proc/net/dev; \
                  echo '#---'; cat /proc/uptime; echo '#---'; df -Pk /";
    let exec = docker
        .create_exec(
            AGENT,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec!["sh", "-c", script]),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None).await?
    else {
        bail!("could not attach to the agent");
    };
    let mut text = Vec::new();
    while let Some(Ok(chunk)) = output.next().await {
        text.extend_from_slice(&docker::output_bytes(&chunk));
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// Takes one sample of every server and stores it.
pub async fn sample(state: SharedState) -> Result<()> {
    for node in docker::ready_nodes(&state.docker).await? {
        let sampled = match cluster::node_docker(&state, &node) {
            Ok(docker) => sample_node(&state, &docker, &node).await,
            Err(e) => Err(e),
        };
        if let Err(error) = sampled {
            tracing::debug!(%node, error = format!("{error:#}"), "no sample from this server");
        }
    }
    Ok(())
}

async fn sample_node(state: &SharedState, docker: &Docker, node: &str) -> anyhow::Result<()> {
    let text = read_proc(state, docker).await?;
    let parts: Vec<&str> = text.split("#---").map(str::trim).collect();
    let [stat, meminfo, net, uptime, df] = parts.as_slice() else {
        bail!("unexpected agent output");
    };
    let (total, idle) = parse_cpu(stat).ok_or_else(|| anyhow!("unreadable /proc/stat"))?;
    // CPU use is a rate between two readings; the first one after a start only
    // primes it, rather than storing a false 0%.
    let cpu_percent = {
        let mut last = LAST_CPU.lock().map_err(|_| anyhow!("poisoned"))?;
        let previous = last
            .get_or_insert_with(HashMap::new)
            .insert(node.to_string(), (total, idle));
        match previous {
            Some((t0, i0)) if total > t0 => {
                let busy = (total - t0).saturating_sub(idle.saturating_sub(i0));
                busy as f64 / (total - t0) as f64 * 100.0
            }
            _ => return Ok(()),
        }
    };
    let cores = stat
        .lines()
        .filter(|l| l.starts_with("cpu") && !l.starts_with("cpu "))
        .count()
        .max(1);
    let (memory_used, memory_total) =
        parse_memory(meminfo).ok_or_else(|| anyhow!("unreadable meminfo"))?;
    let (disk_used, disk_total) = parse_disk(df).ok_or_else(|| anyhow!("unreadable df output"))?;
    let (rx, tx) = parse_network(net);
    sqlx::query!(
        "INSERT INTO host_samples (node_id, ts, cpu_percent, cpu_cores, memory_used, memory_total,
            disk_used, disk_total, net_rx_bytes, net_tx_bytes, uptime_seconds)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT DO NOTHING",
        node,
        Utc::now(),
        cpu_percent,
        cores as i32,
        memory_used as i64,
        memory_total as i64,
        disk_used as i64,
        disk_total as i64,
        rx as i64,
        tx as i64,
        parse_uptime(uptime).unwrap_or(0) as i64
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- docker disk usage

#[derive(Debug, Clone, Serialize)]
pub struct DiskCategory {
    pub count: i64,
    pub size: i64,
    pub reclaimable: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DockerDisk {
    pub images: DiskCategory,
    pub containers: DiskCategory,
    pub volumes: DiskCategory,
    pub build_cache: DiskCategory,
    pub sampled_at: DateTime<Utc>,
}

/// `docker system df` can take seconds (it sizes every volume), so it runs in the
/// background and the overview reads the last result.
pub async fn refresh_docker_disk(state: SharedState) -> Result<()> {
    let df = state.docker.df(None).await?;
    let category = |count: Option<i64>, size: Option<i64>, reclaimable: Option<i64>| DiskCategory {
        count: count.unwrap_or(0),
        size: size.unwrap_or(0),
        reclaimable: reclaimable.unwrap_or(0),
    };
    let images = df
        .image_usage
        .map(|u| category(u.total_count, u.total_size, u.reclaimable));
    let containers = df
        .container_usage
        .map(|u| category(u.total_count, u.total_size, u.reclaimable));
    let volumes = df
        .volume_usage
        .map(|u| category(u.total_count, u.total_size, u.reclaimable));
    let cache = df
        .build_cache_usage
        .map(|u| category(u.total_count, u.total_size, u.reclaimable));
    let empty = || category(None, None, None);
    let disk = DockerDisk {
        images: images.unwrap_or_else(empty),
        containers: containers.unwrap_or_else(empty),
        volumes: volumes.unwrap_or_else(empty),
        build_cache: cache.unwrap_or_else(empty),
        sampled_at: Utc::now(),
    };
    if let Ok(mut slot) = DOCKER_DISK.write() {
        *slot = Some(disk);
    }
    Ok(())
}

pub fn docker_disk() -> Option<DockerDisk> {
    DOCKER_DISK.read().ok().and_then(|d| d.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_totals_count_iowait_as_idle() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\n";
        assert_eq!(parse_cpu(stat), Some((1000, 850)));
    }

    #[test]
    fn memory_used_leaves_out_reclaimable_cache() {
        let meminfo =
            "MemTotal:       2000 kB\nMemFree:         100 kB\nMemAvailable:    1500 kB\n";
        assert_eq!(parse_memory(meminfo), Some((500 * 1024, 2000 * 1024)));
    }

    #[test]
    fn network_counts_physical_interfaces_only() {
        let dev = "Inter-|   Receive |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes\n\
                   lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n\
                   eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n\
                   veth12ab: 5 1 0 0 0 0 0 0 5 1 0 0 0 0 0 0\n\
                   docker0: 7 1 0 0 0 0 0 0 7 1 0 0 0 0 0 0\n";
        assert_eq!(parse_network(dev), (1000, 2000));
    }

    #[test]
    fn disk_and_uptime_parse() {
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\noverlay 1000 250 750 25% /\n";
        assert_eq!(parse_disk(df), Some((250 * 1024, 1000 * 1024)));
        // The agent's output sections start with a newline.
        assert_eq!(
            parse_disk(&format!(
                "
{df}"
            )),
            Some((250 * 1024, 1000 * 1024))
        );
        assert_eq!(parse_uptime("12345.67 99.0\n"), Some(12345));
    }
}
