import { useMutation, useQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, kindLabel, type Project } from "../api";
import { Band, Button, ButtonLink, EmptyState, ErrorText, Field, Input, Modal, PageHeader, StatusDot } from "../ui";

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
        lead="Each project is a private network. Its services reach each other by name, like db:5432."
        actions={
          <>
            <Button onClick={() => setCreating(true)}>New project</Button>
            <ButtonLink to="/templates" variant="secondary">
              Deploy a template
            </ButtonLink>
          </>
        }
      />
      <Band tone="soft">
        {projects.isPending ? (
          <p>Loading…</p>
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
            Start from a template, or create a project and add an app from Git, a Docker image or a
            database.
          </EmptyState>
        ) : (
          <div className="grid gap-8 md:grid-cols-2 xl:grid-cols-3">
            {projects.data.map((project) => (
              <ProjectCard key={project.id} project={project} />
            ))}
          </div>
        )}
      </Band>
      <NewProject open={creating} onClose={() => setCreating(false)} />
    </>
  );
}

function ProjectCard({ project }: { project: Project }) {
  return (
    <Link
      to={`/projects/${project.id}`}
      className="block rounded-md border border-hairline bg-canvas p-6 text-ink! hover:border-ink/40"
    >
      <h3 className="text-lg font-extrabold">{project.name}</h3>
      <p className="mt-1 text-sm text-ink/70">
        {project.services.length} service{project.services.length === 1 ? "" : "s"}
      </p>
      <ul className="mt-5 divide-y divide-divider">
        {project.services.slice(0, 6).map((s) => (
          <li key={s.id} className="flex items-center justify-between gap-3 py-2.5">
            <span className="min-w-0 truncate font-medium">
              {s.name} <span className="font-normal text-ink/60">· {kindLabel(s)}</span>
            </span>
            <StatusDot status={s.status} />
          </li>
        ))}
        {project.services.length === 0 && <li className="py-2.5 text-sm text-ink/70">No services yet</li>}
      </ul>
    </Link>
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
        <Field label="Name" hint="Lowercase letters, digits and hyphens. It becomes part of each service's hostname.">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value.toLowerCase())}
            placeholder="my-shop"
            autoFocus
            required
          />
        </Field>
        <ErrorText error={create.error} />
        <div className="flex justify-end gap-3">
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
