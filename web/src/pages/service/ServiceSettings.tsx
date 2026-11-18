import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useNavigate } from "react-router";
import { api, type ClusterServer, type ServiceDetail } from "../../api";
import { Button, Card, Checkbox, CodeLine, ConfirmDelete, ErrorText, Field, Input, SectionTitle, Segmented, Select, useToast } from "../../ui";

const num = (v: string) => (v.trim() === "" ? null : Number(v));
const str = (v: number | null) => (v == null ? "" : String(v));

export default function ServiceSettings({ service: s }: { service: ServiceDetail }) {
  const toast = useToast();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [deleting, setDeleting] = useState(false);
  const [form, setForm] = useState({
    repo_url: s.repo_url ?? "",
    branch: s.branch ?? "main",
    root_dir: s.root_dir,
    build_type: s.build_type,
    dockerfile_path: s.dockerfile_path,
    git_token: "",
    image: s.image ?? "",
    port: str(s.port),
    public: s.public,
    health_path: s.health_path ?? "",
    cpu_millis: str(s.cpu_millis),
    memory_mb: str(s.memory_mb),
    public_port: str(s.public_port),
    preview_enabled: s.preview_enabled,
    mode: (s.cron_schedule ? "schedule" : "always") as "always" | "schedule",
    cron_schedule: s.cron_schedule ?? "0 3 * * *",
    pre_deploy_command: s.pre_deploy_command ?? "",
    replicas: String(s.replicas),
    autoscale: s.autoscale_max !== null,
    autoscale_min: String(s.autoscale_min ?? Math.max(1, s.replicas)),
    autoscale_max: String(s.autoscale_max ?? Math.max(3, s.replicas)),
    autoscale_cpu: String(s.autoscale_cpu),
    node_id: s.node_id ?? "",
    node_label: s.node_label ?? "",
  });
  const scheduled = s.kind !== "database" && form.mode === "schedule";
  const servers = useQuery({ queryKey: ["servers"], queryFn: () => api.get<ClusterServer[]>("/servers") });
  // One copy, on its own server: anything with a volume, a host port or a schedule.
  const single = s.kind === "database" || s.volumes.length > 0 || form.public_port.trim() !== "" || scheduled;
  const pinnedByData = s.volumes.length > 0 && s.node_id !== null;
  const set = <K extends keyof typeof form>(key: K) => (value: (typeof form)[K]) => setForm((f) => ({ ...f, [key]: value }));

  const save = useMutation({
    mutationFn: () =>
      api.patch(`/services/${s.id}`, {
        ...(s.kind === "app" && {
          repo_url: form.repo_url,
          branch: form.branch,
          root_dir: form.root_dir,
          build_type: form.build_type,
          dockerfile_path: form.dockerfile_path,
          ...(form.git_token && { git_token: form.git_token }),
        }),
        ...(s.kind !== "app" && { image: form.image }),
        ...(s.kind !== "database" && {
          port: num(form.port),
          public: form.public && !scheduled,
          health_path: form.health_path,
          cron_schedule: scheduled ? form.cron_schedule : "",
          pre_deploy_command: form.pre_deploy_command,
        }),
        cpu_millis: num(form.cpu_millis),
        memory_mb: num(form.memory_mb),
        public_port: scheduled ? null : num(form.public_port),
        preview_enabled: form.preview_enabled,
        ...(single
          ? { replicas: 1, autoscale_min: null, autoscale_max: null }
          : form.autoscale
            ? {
                replicas: Number(form.autoscale_min),
                autoscale_min: Number(form.autoscale_min),
                autoscale_max: Number(form.autoscale_max),
                autoscale_cpu: Number(form.autoscale_cpu),
              }
            : { replicas: Number(form.replicas), autoscale_min: null, autoscale_max: null }),
        ...(!pinnedByData && { node_id: form.node_id || null }),
        node_label: form.node_label.trim() || null,
      }),
    onSuccess: () => {
      toast("Saved. Redeploying.");
      queryClient.invalidateQueries({ queryKey: ["service", s.id] });
    },
  });
  const clearToken = useMutation({
    mutationFn: () => api.patch(`/services/${s.id}`, { git_token: "" }),
    onSuccess: () => {
      toast("Token removed");
      queryClient.invalidateQueries({ queryKey: ["service", s.id] });
    },
  });
  const remove = useMutation({
    mutationFn: () => api.del(`/services/${s.id}`),
    onSuccess: () => navigate(`/projects/${s.project.id}`),
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate();
  };

  return (
    <div className="grid max-w-4xl gap-8">
      <form onSubmit={submit} className="grid gap-8">
        {s.kind === "app" && (
          <Card>
            <SectionTitle>Source</SectionTitle>
            <div className="grid gap-5">
              <Field label="Repository URL">
                <Input type="url" value={form.repo_url} onChange={(e) => set("repo_url")(e.target.value)} />
              </Field>
              <div className="grid gap-5 md:grid-cols-2">
                <Field label="Branch">
                  <Input value={form.branch} onChange={(e) => set("branch")(e.target.value)} />
                </Field>
                <Field label="Root directory">
                  <Input value={form.root_dir} onChange={(e) => set("root_dir")(e.target.value)} />
                </Field>
                <Field label="Build with">
                  <Select value={form.build_type} onChange={(e) => set("build_type")(e.target.value as typeof form.build_type)}>
                    <option value="auto">Auto (Dockerfile if present)</option>
                    <option value="dockerfile">Dockerfile</option>
                    <option value="railpack">Railpack</option>
                  </Select>
                </Field>
                <Field label="Dockerfile path">
                  <Input value={form.dockerfile_path} onChange={(e) => set("dockerfile_path")(e.target.value)} />
                </Field>
              </div>
              <Field
                label="Access token"
                hint={s.has_git_token ? "A token is stored. Enter a new one to replace it." : "Only needed for private repositories."}
              >
                <div className="flex gap-3">
                  <Input type="password" autoComplete="off" value={form.git_token} onChange={(e) => set("git_token")(e.target.value)} />
                  {s.has_git_token && (
                    <Button variant="tertiary" onClick={() => clearToken.mutate()}>
                      Remove
                    </Button>
                  )}
                </div>
              </Field>
            </div>
          </Card>
        )}

        {s.kind !== "app" && (
          <Card>
            <SectionTitle>Image</SectionTitle>
            <Field label="Image" hint="Changing it pulls and redeploys.">
              <Input value={form.image} onChange={(e) => set("image")(e.target.value)} />
            </Field>
          </Card>
        )}

        <Card>
          <SectionTitle>Runtime</SectionTitle>
          <div className="grid gap-5">
            {s.kind !== "database" && (
              <div className="grid justify-items-start gap-3">
                <Segmented
                  label="How it runs"
                  value={form.mode}
                  onChange={set("mode")}
                  options={[
                    { value: "always", label: "Always on" },
                    { value: "schedule", label: "On a schedule" },
                  ]}
                />
                <p className="text-[13px] text-muted">
                  {scheduled
                    ? "Nothing stays up. On each scheduled time a container starts from the live image, runs its command and exits. Each run shows on the Runs tab."
                    : "A container stays up and is restarted if it stops."}
                </p>
              </div>
            )}
            {scheduled && (
              <Field
                label="Schedule"
                hint={
                  <>
                    Five fields in UTC: minute, hour, day of month, month, day of week. <code>0 3 * * *</code> is every day at 03:00,{" "}
                    <code>*/15 * * * *</code> every 15 minutes.
                  </>
                }
              >
                <Input className="font-mono text-[14px] md:max-w-xs" value={form.cron_schedule} onChange={(e) => set("cron_schedule")(e.target.value)} required />
              </Field>
            )}
            {s.kind !== "database" && !scheduled && (
              <>
                <div className="grid gap-5 md:grid-cols-2">
                  <Field label="Port" hint="Empty for a worker with no port.">
                    <Input inputMode="numeric" value={form.port} onChange={(e) => set("port")(e.target.value)} />
                  </Field>
                  <Field label="Health check path" hint="Empty: an open port counts as healthy.">
                    <Input value={form.health_path} onChange={(e) => set("health_path")(e.target.value)} placeholder="/health" />
                  </Field>
                </div>
                <Checkbox label="Public" hint="Route a hostname to this service through the proxy." checked={form.public} onChange={set("public")} />
              </>
            )}
            <div className="grid gap-5 md:grid-cols-3">
              <Field label="CPU limit (millicores)" hint="1000 = one core. Empty: no limit.">
                <Input inputMode="numeric" value={form.cpu_millis} onChange={(e) => set("cpu_millis")(e.target.value)} />
              </Field>
              <Field label="Memory limit (MB)" hint="Empty: no limit.">
                <Input inputMode="numeric" value={form.memory_mb} onChange={(e) => set("memory_mb")(e.target.value)} />
              </Field>
              {!scheduled && (
                <Field label="Host port" hint={s.kind === "database" ? "Expose the database on this server's port." : "Also publish the port on the host."}>
                  <Input inputMode="numeric" value={form.public_port} onChange={(e) => set("public_port")(e.target.value)} placeholder="none" />
                </Field>
              )}
            </div>
            {s.volumes.length > 0 && (
              <div>
                <p className="mb-2 text-sm font-semibold">Volumes</p>
                <ul className="text-sm">
                  {s.volumes.map((v) => (
                    <li key={v.id}>
                      <code>{v.mount_path}</code> <span className="text-muted">on {v.name}</span>
                    </li>
                  ))}
                </ul>
                <p className="mt-2 text-sm text-muted">
                  A service with a volume or host port restarts in place on deploy, with a short gap.
                </p>
              </div>
            )}
          </div>
        </Card>

        <Card>
          <SectionTitle>Copies and servers</SectionTitle>
          <div className="grid gap-5">
            {single ? (
              <p className="text-muted">
                Runs as one copy: it has {s.kind === "database" || s.volumes.length > 0 ? "a volume" : scheduled ? "a schedule" : "a host port"}
                {s.kind === "database" || s.volumes.length > 0 ? ", and its data stays on one server" : ""}.
              </p>
            ) : (
              <>
                <Checkbox
                  label="Autoscale"
                  hint="Adds a copy when the average CPU per copy stays above the target for three minutes, and removes one after ten quiet minutes."
                  checked={form.autoscale}
                  onChange={set("autoscale")}
                />
                {form.autoscale ? (
                  <div className="grid gap-5 md:grid-cols-3">
                    <Field label="Fewest copies">
                      <Input type="number" min={1} max={50} value={form.autoscale_min} onChange={(e) => set("autoscale_min")(e.target.value)} />
                    </Field>
                    <Field label="Most copies">
                      <Input type="number" min={1} max={50} value={form.autoscale_max} onChange={(e) => set("autoscale_max")(e.target.value)} />
                    </Field>
                    <Field label="CPU target (%)" hint={s.cpu_millis ? "Of each copy's CPU limit." : "Of one core per copy."}>
                      <Input type="number" min={10} max={100} value={form.autoscale_cpu} onChange={(e) => set("autoscale_cpu")(e.target.value)} />
                    </Field>
                  </div>
                ) : (
                  <Field label="Copies" hint="Spread across servers; traffic is shared between them." className="md:max-w-xs">
                    <Input type="number" min={1} max={50} value={form.replicas} onChange={(e) => set("replicas")(e.target.value)} />
                  </Field>
                )}
              </>
            )}
            <div className="grid gap-5 md:grid-cols-2">
              <Field
                label="Server"
                hint={
                  pinnedByData
                    ? "Its volume lives there, so it stays. Data can't move between servers."
                    : "Any lets the cluster choose and spread copies."
                }
              >
                <Select value={form.node_id} disabled={pinnedByData} onChange={(e) => set("node_id")(e.target.value)}>
                  <option value="">Any server</option>
                  {servers.data?.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.hostname}
                      {n.this ? " (justdeploy's)" : ""}
                    </option>
                  ))}
                </Select>
              </Field>
              {!pinnedByData && (
                <Field label="Only servers labelled" hint="key=value, set on the Servers page. Empty: no restriction.">
                  <Input value={form.node_label} onChange={(e) => set("node_label")(e.target.value)} placeholder="region=eu" />
                </Field>
              )}
            </div>
          </div>
        </Card>

        {s.kind !== "database" && (
          <Card>
            <SectionTitle>Before each release</SectionTitle>
            <Field
              label="Pre-deploy command"
              hint="Runs once from the new image, with this service's variables, before it goes live: database migrations, for example. If it fails, the deploy stops and the current version keeps running. Previews run it against their own database copies, and skip it when they reach the database by a fixed address."
            >
              <Input
                className="font-mono text-[14px]"
                value={form.pre_deploy_command}
                onChange={(e) => set("pre_deploy_command")(e.target.value)}
                placeholder="npm run db:migrate"
              />
            </Field>
          </Card>
        )}

        <div className="flex gap-3">
          <Button type="submit" busy={save.isPending}>
            Save and redeploy
          </Button>
        </div>
        <ErrorText error={save.error} />
      </form>

      {s.kind === "app" && (
        <Card>
          <SectionTitle>Automatic deploys</SectionTitle>
          <p className="mb-4">
            With the GitHub App installed on this repository (Settings, GitHub), pushes already deploy and nothing is needed here.
            Otherwise, add a webhook in your Git host pointing at one of these URLs, with the secret below. A push to{" "}
            <code>{s.branch}</code> deploys. In GitHub choose content type <code>application/json</code>.
          </p>
          <div className="grid gap-3">
            <Field label="GitHub webhook URL">
              <CodeLine value={s.webhooks.github} />
            </Field>
            <Field label="GitLab webhook URL">
              <CodeLine value={s.webhooks.gitlab} />
            </Field>
            <Field label="Secret">
              <CodeLine value={s.webhooks.secret} secret />
            </Field>
          </div>
          <div className="mt-6 border-t border-line pt-6">
            <Checkbox
              label="Preview deploys for pull requests (GitHub)"
              hint={
                <>
                  Each open pull request gets its own container at pr-NUMBER-{s.generated_host ?? "service-project.domain"}, removed
                  when it closes. Also tick "Pull requests" in the webhook's events. Each pull request gets its own copy of the
                  project's databases, filled from their latest backup, when this service reaches them through references like
                  {" ${{db.URL}}"}.
                </>
              }
              checked={form.preview_enabled}
              onChange={(v) => {
                set("preview_enabled")(v);
              }}
            />
            <p className="mt-2 text-sm text-muted">Save above to apply.</p>
          </div>
        </Card>
      )}

      <Card>
        <SectionTitle>Delete service</SectionTitle>
        <p className="mb-4">
          Removes its containers, built images{s.volumes.length > 0 ? " and volumes, with all their data" : ""}.
        </p>
        <Button variant="danger" onClick={() => setDeleting(true)}>
          Delete {s.name}
        </Button>
      </Card>
      <ConfirmDelete
        open={deleting}
        name={s.name}
        what="service"
        consequence={
          <p>
            <strong>{s.name}</strong> stops serving immediately
            {s.volumes.length > 0 ? ", and its volume data is deleted" : ""}. This cannot be undone.
          </p>
        }
        busy={remove.isPending}
        onClose={() => setDeleting(false)}
        onConfirm={() => remove.mutate()}
      />
    </div>
  );
}
