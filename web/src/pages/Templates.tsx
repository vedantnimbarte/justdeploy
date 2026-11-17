import { useMutation, useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { api, kindLabel, type Project, type Template } from "../api";
import { BrandIcon, templateIcons } from "../brandIcons";
import { Badge, Band, Button, ErrorText, Field, Input, Modal, PageHeader, Select } from "../ui";

const CATEGORIES: { id: Template["category"]; title: string; lead: string }[] = [
  { id: "frontend", title: "Frontends", lead: "Static sites and single-page apps, and server-rendered React." },
  { id: "backend", title: "Backends", lead: "APIs in Node.js, Python, Go and Rust." },
  { id: "fullstack", title: "Full-stack", lead: "An app and its database, wired together with environment variables." },
  { id: "selfhosted", title: "Self-hosted apps", lead: "Ready-made images: monitoring, automation, analytics." },
  { id: "database", title: "Databases", lead: "A persistent volume and generated credentials, ready to back up." },
];

export default function TemplatesPage() {
  const [params] = useSearchParams();
  const intoProject = params.get("project");
  const templates = useQuery({ queryKey: ["templates"], queryFn: () => api.get<Template[]>("/templates") });
  const [chosen, setChosen] = useState<Template | null>(null);

  return (
    <>
      <PageHeader
        title="Templates"
        lead="Working setups you can deploy in one click. Code templates build from justdeploy's own repository, so each is also an example to read."
      />
      {templates.isError && (
        <Band>
          <ErrorText error={templates.error} />
        </Band>
      )}
      <Band className="grid gap-12">
        {CATEGORIES.map((category) => {
          const items = templates.data?.filter((t) => t.category === category.id) ?? [];
          if (items.length === 0) return null;
          return (
            <section key={category.id} aria-labelledby={`cat-${category.id}`}>
              <div className="mb-4 border-b border-line pb-3">
                <h2 id={`cat-${category.id}`} className="text-lg font-semibold">
                  {category.title}
                </h2>
                <p className="mt-0.5 text-sm text-muted">{category.lead}</p>
              </div>
              <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                {items.map((t) => (
                  <button
                    key={t.id}
                    onClick={() => setChosen(t)}
                    className="group flex flex-col rounded-panel border border-line bg-surface p-5 text-left hover:border-line-strong"
                  >
                    <span className="flex items-start justify-between gap-3">
                      <span className="flex gap-1.5">
                        {templateIcons(t.id).map((icon) => (
                          <BrandIcon key={icon.slug} icon={icon} />
                        ))}
                      </span>
                      <span className="text-sm font-medium text-muted group-hover:text-fg">Deploy</span>
                    </span>
                    <span className="mt-4 font-semibold">{t.name}</span>
                    <span className="mt-1 flex-1 text-sm text-muted">{t.description}</span>
                    <span className="mt-4 flex flex-wrap gap-1.5">
                      {t.services.map((s) => (
                        <Badge key={s.name}>
                          {s.name}: {kindLabel({ kind: s.kind, engine: s.engine ?? null, image: s.image ?? null })}
                        </Badge>
                      ))}
                    </span>
                  </button>
                ))}
              </div>
            </section>
          );
        })}
      </Band>
      {chosen && <DeployTemplate template={chosen} projectId={intoProject} onClose={() => setChosen(null)} />}
    </>
  );
}

function DeployTemplate({
  template,
  projectId,
  onClose,
}: {
  template: Template;
  projectId: string | null;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/projects") });
  const [target, setTarget] = useState<string>(projectId ?? "new");
  const [name, setName] = useState(template.id);
  const deploy = useMutation({
    mutationFn: () =>
      api.post<{ project_id: string }>(
        `/templates/${template.id}/deploy`,
        target === "new" ? { project_name: name } : { project_id: target },
      ),
    onSuccess: (result) => navigate(`/projects/${result.project_id}`),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    deploy.mutate();
  };
  return (
    <Modal open title={`Deploy ${template.name}`} onClose={onClose}>
      <form onSubmit={submit} className="space-y-5">
        <p>{template.description}</p>
        <Field label="Into">
          <Select value={target} onChange={(e) => setTarget(e.target.value)}>
            <option value="new">A new project</option>
            {projects.data?.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </Select>
        </Field>
        {target === "new" && (
          <Field label="Project name">
            <Input value={name} onChange={(e) => setName(e.target.value.toLowerCase())} required />
          </Field>
        )}
        <ErrorText error={deploy.error} />
        <div className="flex justify-end gap-3">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" busy={deploy.isPending}>
            Deploy
          </Button>
        </div>
      </form>
    </Modal>
  );
}
