use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// production for a base project; staging and the like for its copies.
    pub environment: String,
    /// The production project this environment was made from.
    pub base_id: Option<Uuid>,
    /// Its public status page is published.
    pub status_page: bool,
    /// A custom hostname for the status page, besides status-<project>.<base domain>.
    pub status_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Service {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    /// app | image | database
    pub kind: String,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub root_dir: String,
    #[serde(skip)]
    pub git_token_enc: Option<String>,
    /// auto | dockerfile | railpack
    pub build_type: String,
    pub dockerfile_path: String,
    pub image: Option<String>,
    pub engine: Option<String>,
    pub port: Option<i32>,
    pub public: bool,
    pub health_path: Option<String>,
    pub cpu_millis: Option<i32>,
    pub memory_mb: Option<i32>,
    pub public_port: Option<i32>,
    pub preview_enabled: bool,
    pub webhook_secret: String,
    /// running | stopped | deleting
    pub desired_state: String,
    pub current_deployment_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Runs from the new image before release, e.g. migrations.
    pub pre_deploy_command: Option<String>,
    /// Five-field cron, UTC. Set: the service runs on this schedule instead of staying up.
    pub cron_schedule: Option<String>,
    /// Copies running now. Autoscaling moves it between the two bounds below.
    pub replicas: i32,
    pub autoscale_min: Option<i32>,
    /// Set: autoscaling is on.
    pub autoscale_max: Option<i32>,
    /// Average CPU per copy to aim for, in percent of its limit (or of one core).
    pub autoscale_cpu: i32,
    /// The server (Swarm node id) it is pinned to; always set once a service has a volume.
    pub node_id: Option<String>,
    /// `key=value`: run only on servers with that label.
    pub node_label: Option<String>,
    /// Listed on its project's public status page.
    pub on_status_page: bool,
    /// Postgres point-in-time recovery: the S3 destination its write-ahead log streams to.
    pub pitr_destination_id: Option<Uuid>,
    pub pitr_base_backup_at: Option<DateTime<Utc>>,
    /// A database made from another's history at `restore_target`, filled before it first starts.
    pub restore_from: Option<Uuid>,
    pub restore_target: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: Uuid,
    pub service_id: Uuid,
    pub command: Option<String>,
    /// schedule | manual
    pub trigger: String,
    /// running | succeeded | failed
    pub status: String,
    pub exit_code: Option<i64>,
    pub output: String,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Deployment {
    pub id: Uuid,
    pub service_id: Uuid,
    /// production | preview
    pub kind: String,
    pub pr_number: Option<i32>,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub image_ref: Option<String>,
    /// queued | building | deploying | running | failed | superseded | removed
    pub status: String,
    pub trigger: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub id: Uuid,
    pub service_id: Uuid,
    pub name: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Domain {
    pub id: Uuid,
    pub service_id: Uuid,
    pub hostname: String,
    pub created_at: DateTime<Utc>,
}
