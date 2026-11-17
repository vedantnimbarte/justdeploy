import { useMutation, useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, timeAgo, type Project } from "../api";
import { Band, Button, ButtonLink, cx, Dot, EmptyState, ErrorText, Field, Input, Modal, PageHeader } from "../ui";

export default function ProjectsPage() {
  const [creating, setCreating] = useState(false);
  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => api.get<Project[]>("/projects"),
    refetchInterval: 5_000,
  });

  return (
    <>
      <PageHeader
        title="Projects"
        lead="Each project is a private network: its services reach each other by name, like db:5432."
        actions={
          <>
            <ButtonLink to="/templates" variant="secondary">
              Deploy a template
            </ButtonLink>
            <Button onClick={() => setCreating(true)}>New project</Button>
          </>
        }
      />
      <Band>
        {projects.isPending ? (
          <p className="text-muted">Loading…</p>
        ) : projects.isError ? (
          <ErrorText error={projects.error} />
        ) : projects.data.length === 0 ? (
          <EmptyState
            title="Nothing deployed yet"
            action={
              <>
                <ButtonLink to="/templates">Browse templates</ButtonLink>
                <Button variant="secondary" onClick={() => setCreating(true)}>
                  New project
                </Button>
              </>
            }
          >
            Start from a template, or create a project and add an app from Git, a Docker image or a database.
          </EmptyState>
        ) : (
          <ul className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {projects.data.map((project) => (
              <ProjectCard key={project.id} project={project} />
            ))}
          </ul>
        )}
      </Band>
      <NewProject open={creating} onClose={() => setCreating(false)} />
    </>
  );
}

/** One project per card: its services as live dots, then what matters. */
function ProjectCard({ project }: { project: Project }) {
  const services = project.services;
  const trouble = services.filter((s) => s.status === "crashed" || s.status === "failed").length;
  const deploying = services.filter((s) => s.status === "deploying").length;
  const running = services.filter((s) => s.status === "running").length;
  const lastDeploy = services
    .map((s) => s.latest_deployment?.created_at)
    .filter((d): d is string => Boolean(d))
    .sort()
    .at(-1);

  return (
    <li>
      <Link
        to={`/projects/${project.id}`}
        className={cx(
          "flex h-full flex-col rounded-panel border bg-surface no-underline hover:bg-subtle",
          trouble ? "border-err/45" : "border-line hover:border-line-strong",
        )}
      >
        <div className="flex-1 px-5 pt-5 pb-4">
          <div className="flex items-start justify-between gap-4">
            <p className="min-w-0 truncate text-[17px] font-semibold">{project.name}</p>
            <span className="mt-1.5 flex shrink-0 flex-wrap justify-end gap-1.5" aria-hidden>
              {services.map((s) => (
                <span key={s.id} title={`${s.name}: ${s.status}`}>
                  <Dot status={s.status} className="size-2.5" />
                </span>
              ))}
            </span>
          </div>
          <p className="mt-1 line-clamp-2 text-sm text-muted">
            {services.length === 0 ? "No services yet" : services.map((s) => s.name).join(", ")}
          </p>
        </div>
        <div className="flex items-center justify-between gap-3 border-t border-line px-5 py-3 text-sm">
          <span className={trouble ? "font-medium text-err" : "text-muted"}>
            {trouble
              ? `${trouble} need${trouble === 1 ? "s" : ""} attention`
              : deploying
                ? `${deploying} deploying`
                : services.length === 0
                  ? "Empty"
                  : `${running} of ${services.length} running`}
          </span>
          <span className="text-muted">{lastDeploy ? `Deployed ${timeAgo(lastDeploy)}` : "Never deployed"}</span>
        </div>
      </Link>
    </li>
  );
}

function NewProject({ open, onClose }: { open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const create = useMutation({
    mutationFn: () => api.post<{ id: string }>("/projects", { name }),
    onSuccess: (project) => navigate(`/projects/${project.id}`),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate();
  };
  return (
    <Modal open={open} title="New project" onClose={onClose}>
      <form onSubmit={submit} className="space-y-5">
        <Field label="Name" hint="Lowercase letters, digits and hyphens. It becomes part of each service's address.">
          <Input value={name} onChange={(e) => setName(e.target.value.toLowerCase())} placeholder="my-shop" autoFocus required />
        </Field>
        <ErrorText error={create.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" busy={create.isPending}>
            Create project
          </Button>
        </div>
      </form>
    </Modal>
  );
}
