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
          <ul className="overflow-hidden rounded-panel border border-line bg-surface">
            {projects.data.map((project) => (
              <ProjectRow key={project.id} project={project} />
            ))}
          </ul>
        )}
      </Band>
      <NewProject open={creating} onClose={() => setCreating(false)} />
    </>
  );
}

/** One project per row: its services as a strip of live dots, then what matters. */
function ProjectRow({ project }: { project: Project }) {
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
    <li className="border-b border-line last:border-b-0">
      <Link
        to={`/projects/${project.id}`}
        className="grid gap-x-6 gap-y-2 px-5 py-4 no-underline hover:bg-subtle sm:grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1fr)_17rem_9rem]"
      >
        <div className="min-w-0">
          <p className="font-semibold">{project.name}</p>
          <p className="mt-0.5 truncate text-sm text-muted">
            {services.length === 0 ? "No services yet" : services.map((s) => s.name).join(", ")}
          </p>
        </div>

        <div className="flex items-center gap-3 self-center" aria-label={`${running} of ${services.length} running`}>
          <span className="flex shrink-0 gap-1.5">
            {services.map((s) => (
              <span key={s.id} title={`${s.name}: ${s.status}`}>
                <Dot status={s.status} className="size-2.5" />
              </span>
            ))}
          </span>
          <span className={cx("whitespace-nowrap text-sm", trouble ? "font-medium text-err" : "text-muted")}>
            {trouble
              ? `${trouble} need${trouble === 1 ? "s" : ""} attention`
              : deploying
                ? `${deploying} deploying`
                : services.length === 0
                  ? "Empty"
                  : `${running} of ${services.length} running`}
          </span>
        </div>

        <p className="self-center text-sm text-muted md:text-right">
          {lastDeploy ? `Deployed ${timeAgo(lastDeploy)}` : "Never deployed"}
        </p>
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
