import { useMutation, useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { api, kindLabel, type Project, type Template } from "../api";
import { Badge, Band, Button, ErrorText, Field, Input, Modal, PageHeader, SectionTitle, Select } from "../ui";

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
        lead="Working examples to deploy in one click. Code templates build from this project's own repository, so open one up to see how it is put together."
      />
      {templates.isError && (
        <Band>
          <ErrorText error={templates.error} />
        </Band>
      )}
      {CATEGORIES.map((category, i) => {
        const items = templates.data?.filter((t) => t.category === category.id) ?? [];
        if (items.length === 0) return null;
        return (
          <Band key={category.id} tone={i % 2 === 0 ? "soft" : "white"}>
            <SectionTitle>{category.title}</SectionTitle>
            <p className="-mt-2 mb-8 max-w-3xl">{category.lead}</p>
            <div className="grid gap-8 md:grid-cols-2 xl:grid-cols-4">
              {items.map((t) => (
                <div
                  key={t.id}
                  className={`flex flex-col rounded-md p-6 ${i % 2 === 0 ? "border border-hairline bg-canvas" : "bg-canvas-warm"}`}
                >
                  <h3 className="text-lg font-extrabold">{t.name}</h3>
                  <p className="mt-2 flex-1">{t.description}</p>
                  <div className="mt-4 flex flex-wrap gap-2">
                    {t.services.map((s) => (
                      <Badge key={s.name}>
                        {s.name} · {kindLabel({ kind: s.kind, engine: s.engine ?? null, image: s.image ?? null })}
                      </Badge>
                    ))}
                  </div>
                  <div className="mt-6">
                    <Button onClick={() => setChosen(t)}>Deploy</Button>
                  </div>
                </div>
              ))}
            </div>
          </Band>
        );
      })}
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
