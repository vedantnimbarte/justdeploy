use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{net::SocketAddr, path::PathBuf};

/// Everything read from the environment, once, at startup.
#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,
    /// 32-byte key that encrypts env values, git tokens and backup credentials.
    pub master_key: [u8; 32],
    /// Base domain used until the admin changes it in Settings.
    pub base_domain: String,
    /// Where the dashboard is reached; used to print webhook URLs.
    pub public_url: String,
    /// When set, Traefik issues Let's Encrypt certificates and routes use HTTPS.
    pub acme_email: Option<String>,
    pub cookie_secure: bool,
    /// Image that clones and builds apps, and runs health checks.
    pub builder_image: String,
    /// Repository that code templates build from.
    pub templates_repo: String,
    pub templates_branch: String,
    /// Built dashboard to serve; unset in development, where Vite serves it.
    pub web_dir: Option<PathBuf>,
    /// Root for "local directory" backup destinations.
    pub backup_dir: PathBuf,
    /// Deploys (builds included) that run at once, each for a different service.
    pub deploy_workers: usize,
    /// The address other servers reach this one on, when Docker can't pick one
    /// (a server with several addresses) while starting the Swarm.
    pub advertise_addr: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let master_key = parse_key(&required("JD_MASTER_KEY")?)?;
        let acme_email = optional("JD_ACME_EMAIL", "");
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind: optional("JD_BIND", "0.0.0.0:8080")
                .parse()
                .context("JD_BIND must be host:port")?,
            master_key,
            base_domain: optional("JD_BASE_DOMAIN", "localhost"),
            public_url: optional("JD_PUBLIC_URL", "http://localhost:8080")
                .trim_end_matches('/')
                .to_string(),
            cookie_secure: !acme_email.is_empty()
                || optional("JD_COOKIE_SECURE", "false") == "true",
            acme_email: (!acme_email.is_empty()).then_some(acme_email),
            builder_image: optional(
                "JD_BUILDER_IMAGE",
                "ghcr.io/vedantnimbarte/justdeploy-builder:latest",
            ),
            templates_repo: optional(
                "JD_TEMPLATES_REPO",
                "https://github.com/vedantnimbarte/justdeploy",
            ),
            templates_branch: optional("JD_TEMPLATES_BRANCH", "main"),
            web_dir: std::env::var("JD_WEB_DIR").ok().map(PathBuf::from),
            backup_dir: PathBuf::from(optional("JD_BACKUP_DIR", "/backups")),
            deploy_workers: optional("JD_DEPLOY_WORKERS", "2")
                .parse()
                .ok()
                .filter(|n| *n > 0)
                .context("JD_DEPLOY_WORKERS must be a whole number above 0")?,
            advertise_addr: std::env::var("JD_ADVERTISE_ADDR")
                .ok()
                .filter(|v| !v.is_empty()),
        })
    }

    pub fn tls(&self) -> bool {
        self.acme_email.is_some()
    }
}

fn required(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => bail!("{name} is required"),
    }
}

fn optional(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn parse_key(value: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD
        .decode(value.trim())
        .context("JD_MASTER_KEY must be base64; generate one with `openssl rand -base64 32`")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("JD_MASTER_KEY must decode to exactly 32 bytes"))
}
