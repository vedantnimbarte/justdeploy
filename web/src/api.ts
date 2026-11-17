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
  del: <T>(path: string) => request<T>("DELETE", path),
};

export type Status = "deploying" | "running" | "crashed" | "stopped" | "failed" | "idle" | "deleting";

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
  desired_state: "running" | "stopped" | "deleting";
  current_deployment_id: string | null;
  created_at: string;
  status: Status;
  url: string | null;
  connection_url: string | null;
  has_git_token: boolean;
  latest_deployment: Deployment | null;
}

export interface Project {
  id: string;
  name: string;
  created_at: string;
  services: Service[];
}

export interface Domain {
  id: string;
  hostname: string;
  created_at: string;
}

export interface ServiceDetail extends Service {
  project: { id: string; name: string };
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
