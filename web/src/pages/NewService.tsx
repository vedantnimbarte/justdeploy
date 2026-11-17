import { useMutation, useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { api, type Project, type Service, type Settings } from "../api";
import { Band, Button, Card, Checkbox, ErrorText, Field, Input, PageHeader, Select, Tabs, Textarea } from "../ui";
import { parseDotenv } from "./service/Env";

type Kind = "app" | "image" | "database";

export default function NewServicePage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const [kind, setKind] = useState<Kind>("app");
  const project = useQuery({ queryKey: ["project", id], queryFn: () => api.get<Project>(`/projects/${id}`) });
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api.get<Settings>("/settings") });

  const [form, setForm] = useState({
    name: "",
    repo_url: "",
    branch: "main",
    root_dir: "",
    git_token: "",
    build_type: "auto",
    image: "",
    engine: "postgres",
    port: "8080",
    public: true,
    health_path: "",
    env: "",
    volumes: "",
  });
  const set = (key: keyof typeof form) => (value: string | boolean) => setForm((f) => ({ ...f, [key]: value }));

  const create = useMutation({
    mutationFn: () => {
      const port = form.port.trim() === "" ? null : Number(form.port);
      return api.post<Service>(`/projects/${id}/services`, {
        name: form.name,
        kind,
        repo_url: kind === "app" ? form.repo_url : undefined,
        branch: kind === "app" ? form.branch : undefined,
        root_dir: kind === "app" ? form.root_dir : "",
        git_token: kind === "app" && form.git_token ? form.git_token : undefined,
        build_type: kind === "app" ? form.build_type : undefined,
        image: kind === "image" ? form.image : undefined,
        engine: kind === "database" ? form.engine : undefined,
        port: kind === "database" ? undefined : port,
        public: kind === "database" ? false : form.public && port !== null,
        health_path: form.health_path || undefined,
        env: Object.fromEntries(parseDotenv(form.env).map((v) => [v.key, v.value])),
        volumes: kind === "image" ? form.volumes.split(/[\s,]+/).filter(Boolean) : [],
      });
    },
    onSuccess: (service) => navigate(`/services/${service.id}`),
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate();
  };

  return (
    <>
      <PageHeader
        eyebrow={
          <>
            <Link to="/projects">Projects</Link> / <Link to={`/projects/${id}`}>{project.data?.name ?? "…"}</Link>
          </>
        }
        title="Add a service"
        lead="Build from a Git repository, run a Docker image, or start a database."
      />
      <Tabs<Kind>
        tabs={[
          { id: "app", label: "Git repository" },
          { id: "image", label: "Docker image" },
          { id: "database", label: "Database" },
        ]}
        active={kind}
        onChange={setKind}
      />
      <Band tone="soft">
        <form onSubmit={submit} className="grid max-w-3xl gap-8">
          <Card>
            <div className="grid gap-5">
              <Field label="Service name" hint="Other services in the project reach it by this name.">
                <Input
                  required
                  value={form.name}
                  onChange={(e) => set("name")(e.target.value.toLowerCase())}
                  placeholder={kind === "database" ? "db" : "api"}
                />
              </Field>

              {kind === "app" && (
                <>
                  <Field label="Repository URL" hint="HTTPS clone URL from GitHub, GitLab or any Git host.">
                    <Input
                      required
                      type="url"
                      value={form.repo_url}
                      onChange={(e) => set("repo_url")(e.target.value)}
                      placeholder="https://github.com/you/app"
                    />
                  </Field>
                  <div className="grid gap-5 md:grid-cols-2">
                    <Field label="Branch">
                      <Input value={form.branch} onChange={(e) => set("branch")(e.target.value)} />
                    </Field>
                    <Field label="Root directory" hint="For monorepos. Leave empty for the repository root.">
                      <Input value={form.root_dir} onChange={(e) => set("root_dir")(e.target.value)} placeholder="apps/web" />
                    </Field>
                  </div>
                  <div className="grid gap-5 md:grid-cols-2">
                    <Field label="Build with" hint="Auto uses the Dockerfile when there is one, Railpack otherwise.">
                      <Select value={form.build_type} onChange={(e) => set("build_type")(e.target.value)}>
                        <option value="auto">Auto</option>
                        <option value="dockerfile">Dockerfile</option>
                        <option value="railpack">Railpack</option>
                      </Select>
                    </Field>
                    <Field label="Access token" hint="Only for private repositories. Stored encrypted.">
                      <Input
                        type="password"
                        autoComplete="off"
                        value={form.git_token}
                        onChange={(e) => set("git_token")(e.target.value)}
                      />
                    </Field>
                  </div>
                </>
              )}

              {kind === "image" && (
                <>
                  <Field label="Image" hint="Any public image, for example nginx:alpine or ghcr.io/owner/app:1.2.">
                    <Input required value={form.image} onChange={(e) => set("image")(e.target.value)} placeholder="nginx:alpine" />
                  </Field>
                  <Field label="Persistent volumes" hint="Absolute paths inside the container, separated by spaces.">
                    <Input value={form.volumes} onChange={(e) => set("volumes")(e.target.value)} placeholder="/data" />
                  </Field>
                </>
              )}

              {kind === "database" && (
                <Field label="Engine" hint="Credentials are generated, and the data lives on a persistent volume.">
                  <Select value={form.engine} onChange={(e) => set("engine")(e.target.value)}>
                    {(settings.data?.engines ?? []).map((engine) => (
                      <option key={engine.id} value={engine.id}>
                        {engine.label} ({engine.image})
                      </option>
                    ))}
                  </Select>
                </Field>
              )}
            </div>
          </Card>

          {kind !== "database" && (
            <Card>
              <div className="grid gap-5">
                <div className="grid gap-5 md:grid-cols-2">
                  <Field label="Port" hint="The port the app listens on; PORT is set to it. Empty for a worker.">
                    <Input inputMode="numeric" value={form.port} onChange={(e) => set("port")(e.target.value)} />
                  </Field>
                  <Field label="Health check path" hint="Optional. Without it, an open port counts as healthy.">
                    <Input value={form.health_path} onChange={(e) => set("health_path")(e.target.value)} placeholder="/health" />
                  </Field>
                </div>
                <Checkbox
                  label="Public"
                  hint={`Serve it at ${form.name || "name"}-${project.data?.name ?? "project"}.${settings.data?.base_domain ?? "…"}`}
                  checked={form.public}
                  onChange={set("public")}
                />
                <Field label="Environment variables" hint="One KEY=value per line. You can change them later.">
                  <Textarea rows={5} value={form.env} onChange={(e) => set("env")(e.target.value)} placeholder={"NODE_ENV=production\nAPI_KEY=..."} />
                </Field>
              </div>
            </Card>
          )}

          <ErrorText error={create.error} />
          <div className="flex gap-3">
            <Button type="submit" busy={create.isPending}>
              Create and deploy
            </Button>
            <Button variant="secondary" onClick={() => navigate(`/projects/${id}`)}>
              Cancel
            </Button>
          </div>
        </form>
      </Band>
    </>
  );
}
