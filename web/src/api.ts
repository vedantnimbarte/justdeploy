// Typed client for the justdeploy API. Same origin: Vite proxies in development,
// the API serves this bundle in production.

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.status === 401 && !path.startsWith("/auth/")) {
    window.location.assign("/");
  }
  const text = await response.text();
  const data = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new ApiError(response.status, data?.error ?? response.statusText);
  }
  return data as T;
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body ?? {}),
  put: <T>(path: string, body?: unknown) => request<T>("PUT", path, body),
  patch: <T>(path: string, body?: unknown) => request<T>("PATCH", path, body),
  del: <T>(path: string, body?: unknown) => request<T>("DELETE", path, body),
};

export type Status = "deploying" | "running" | "scheduled" | "crashed" | "stopped" | "failed" | "idle" | "deleting";

export interface Deployment {
  id: string;
  service_id: string;
  kind: "production" | "preview";
  pr_number: number | null;
  repo_url: string | null;
  branch: string | null;
  commit_sha: string | null;
  commit_message: string | null;
  image_ref: string | null;
  status: "queued" | "building" | "deploying" | "running" | "failed" | "superseded" | "removed";
  trigger: string;
  error: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface Service {
  id: string;
  project_id: string;
  name: string;
  kind: "app" | "image" | "database";
  repo_url: string | null;
  branch: string | null;
  root_dir: string;
  build_type: "auto" | "dockerfile" | "railpack";
  dockerfile_path: string;
  image: string | null;
  engine: string | null;
  port: number | null;
  public: boolean;
  health_path: string | null;
  cpu_millis: number | null;
  memory_mb: number | null;
  public_port: number | null;
  preview_enabled: boolean;
  pre_deploy_command: string | null;
  cron_schedule: string | null;
  /** Copies to run now; autoscaling moves it between the bounds. */
  replicas: number;
  autoscale_min: number | null;
  autoscale_max: number | null;
  autoscale_cpu: number;
  /** The server (Swarm node id) it's pinned to. */
  node_id: string | null;
  /** key=value: only servers with that label. */
  node_label: string | null;
  on_status_page: boolean;
  pitr_destination_id: string | null;
  desired_state: "running" | "stopped" | "deleting";
  current_deployment_id: string | null;
  created_at: string;
  status: Status;
  url: string | null;
  connection_url: string | null;
  has_git_token: boolean;
  latest_deployment: Deployment | null;
  copies_running: number;
  copies_desired: number;
}

export interface Project {
  id: string;
  name: string;
  created_at: string;
  /** production for the base project. */
  environment: string;
  base_id: string | null;
  services: Service[];
  /** Every environment of the project, production first. */
  environments: { id: string; environment: string }[];
  status_page: boolean;
  status_domain: string | null;
}

export interface Domain {
  id: string;
  hostname: string;
  created_at: string;
}

export interface ServiceDetail extends Service {
  project: { id: string; name: string; environment: string };
  volumes: { id: string; name: string; mount_path: string }[];
  domains: Domain[];
  generated_host: string | null;
  webhooks: { github: string; gitlab: string; secret: string };
}

export interface EnvVar {
  key: string;
  value: string;
}

export interface TemplateService {
  name: string;
  kind: Service["kind"];
  engine?: string;
  image?: string;
  root_dir?: string;
}

export interface Template {
  id: string;
  name: string;
  category: "frontend" | "backend" | "fullstack" | "selfhosted" | "database";
  description: string;
  services: TemplateService[];
}

export interface Settings {
  base_domain: string;
  tls: boolean;
  acme_email: string | null;
  public_url: string;
  version: string;
  engines: { id: string; label: string; image: string }[];
}

export interface Run {
  id: string;
  command: string | null;
  trigger: "schedule" | "manual";
  status: "running" | "succeeded" | "failed";
  exit_code: number | null;
  output: string;
  error: string | null;
  started_at: string;
  finished_at: string | null;
}

export interface NotificationChannel {
  id: string;
  name: string;
  kind: "slack" | "discord" | "webhook";
  notify_success: boolean;
  target: string;
  created_at: string;
}

export type Role = "viewer" | "member" | "admin" | "owner";
const RANK: Record<Role, number> = { viewer: 0, member: 1, admin: 2, owner: 3 };

/** Whether someone with `role` may do what needs `needed`. Each role includes the ones below. */
export const allows = (role: Role | undefined, needed: Role) => role !== undefined && RANK[role] >= RANK[needed];

export const ROLE_LABEL: Record<Role, string> = { viewer: "Viewer", member: "Member", admin: "Admin", owner: "Owner" };
export const ROLE_HINT: Record<Role, string> = {
  viewer: "Sees projects, logs and deploys. No secrets, no changes.",
  member: "Deploys and changes services, variables and domains, runs commands, opens terminals.",
  admin: "Everything a member can, plus deleting projects and services, restoring backups and changing settings.",
  owner: "Everything an admin can, plus inviting people and changing their access.",
};

export interface Person {
  id: string;
  email: string;
  role: Role;
  two_factor: boolean;
  created_at: string;
}

export interface Invite {
  id: string;
  email: string;
  role: Role;
  expires_at: string;
  created_at: string;
}

export interface ApiToken {
  id: string;
  name: string;
  last_used_at: string | null;
  created_at: string;
}

export interface ActivityEntry {
  id: number;
  actor: string | null;
  action: string;
  target: string;
  detail: string | null;
  created_at: string;
}

export interface GithubOverview {
  app: { slug: string; html_url: string; install_url: string } | null;
  installations: { id: number; account: string }[];
  error: string | null;
}

export interface GithubRepo {
  full_name: string;
  clone_url: string;
  default_branch: string;
  private: boolean;
}

export interface ClusterServer {
  id: string;
  hostname: string;
  role: "manager" | "worker";
  leader: boolean;
  /** The server justdeploy runs on. */
  this: boolean;
  state: "ready" | "down" | "unknown" | "disconnected";
  availability: "active" | "drain" | "pause";
  address: string;
  cpus: number;
  memory_bytes: number;
  platform: string;
  docker_version: string;
  labels: Record<string, string>;
  tasks: number;
  usage: {
    cpu_percent: number;
    memory_used: number;
    memory_total: number;
    disk_used: number;
    disk_total: number;
    sampled_at: string;
  } | null;
}

export interface UsagePoint {
  t: string;
  service_id: string;
  /** Percent of one core, all copies added up. */
  cpu: number;
  memory: number;
  disk: number;
}

export interface ServiceUptime {
  id: string;
  name: string;
  url: string | null;
  up: boolean | null;
  latency_ms: number | null;
  checked_at: string | null;
  uptime_24h: number | null;
  uptime_7d: number | null;
  uptime_90d: number | null;
  /** 90 days, oldest first. */
  days: (number | null)[];
  incidents: { from: string; to: string | null }[];
}

export interface StatusPageData {
  title: string;
  overall: "operational" | "degraded" | "down";
  services: ServiceUptime[];
}

export interface PitrStatus {
  destination_id: string | null;
  base_backup_at: string | null;
  earliest: string | null;
  archived_at: string | null;
  failed_count: number;
  failed_at: string | null;
  error: string | null;
}

export interface LogLine {
  id: number;
  ts: string;
  line: string;
}

export interface DnsCheck {
  expected: string[];
  found: string[];
  status: "connected" | "missing" | "elsewhere" | "unknown";
}

export interface Destination {
  id: string;
  name: string;
  kind: "s3" | "local";
  summary: string;
  created_at: string;
}

export interface Backup {
  id: string;
  destination_id: string | null;
  object_key: string;
  status: "running" | "succeeded" | "failed";
  size_bytes: number | null;
  error: string | null;
  trigger: string;
  created_at: string;
  finished_at: string | null;
  restore_status: "queued" | "running" | "succeeded" | "failed" | null;
  restore_error: string | null;
  restored_at: string | null;
}

export interface Schedule {
  destination_id: string;
  cron: string;
  keep: number;
  last_run_at?: string | null;
}

export interface Sample {
  ts: string;
  cpu_percent: number;
  memory_bytes: number;
  net_rx_rate: number;
  net_tx_rate: number;
}

// ---------------------------------------------------------------- formatting

export function timeAgo(iso: string | null | undefined): string {
  if (!iso) return "";
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

export function duration(start: string | null, end: string | null): string {
  if (!start) return "";
  const ms = (end ? new Date(end).getTime() : Date.now()) - new Date(start).getTime();
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

export function bytes(n: number | null | undefined): string {
  if (n == null) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value < 10 && unit > 0 ? 1 : 0)} ${units[unit]}`;
}

/** What started a deployment, in words. */
export function triggerLabel(trigger: string): string {
  const labels: Record<string, string> = {
    create: "First deploy",
    manual: "Deployed by hand",
    webhook: "Git push",
    rollback: "Rollback",
    env: "Variables changed",
    domains: "Domains changed",
    settings: "Settings changed",
    reconcile: "Recovered",
  };
  return labels[trigger] ?? trigger;
}

export function kindLabel(service: Pick<Service, "kind" | "engine" | "image">): string {
  if (service.kind === "database") {
    const labels: Record<string, string> = {
      postgres: "PostgreSQL",
      mysql: "MySQL",
      mariadb: "MariaDB",
      redis: "Redis",
      mongodb: "MongoDB",
    };
    return labels[service.engine ?? ""] ?? "Database";
  }
  return service.kind === "app" ? "Git" : "Image";
}
