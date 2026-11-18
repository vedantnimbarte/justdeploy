import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, type Project } from "../api";
import { useMe } from "../me";
import { Button, cx, ErrorText, Field, Input, Modal, Select, useToast } from "../ui";

/** The project's name without its environment suffix: shop for shop-staging. */
export function baseName(p: Pick<Project, "name" | "environment">): string {
  return p.environment === "production" ? p.name : p.name.replace(new RegExp(`-${p.environment}$`), "");
}

/** Switches between a project's environments, and makes or syncs one. */
export function EnvironmentBar({ project }: { project: Project }) {
  const { can } = useMe();
  const [creating, setCreating] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const others = project.environments.filter((e) => e.id !== project.id);

  return (
    <div className="mt-6 flex flex-wrap items-center gap-x-4 gap-y-3">
      <nav aria-label="Environments" className="inline-flex flex-wrap rounded-control border border-line bg-subtle p-0.5">
        {project.environments.map((e) => (
          <Link
            key={e.id}
            to={`/projects/${e.id}`}
            aria-current={e.id === project.id ? "page" : undefined}
            className={cx(
              "rounded-[5px] px-3 py-1 text-sm font-medium no-underline",
              e.id === project.id ? "bg-surface text-fg shadow-[0_1px_2px_rgb(0_0_0/0.08)]" : "text-muted hover:text-fg",
            )}
          >
            {e.environment}
          </Link>
        ))}
      </nav>
      {can("member") && (
        <div className="flex gap-1">
          <Button variant="tertiary" className="h-8" onClick={() => setCreating(true)}>
            New environment
          </Button>
          {others.length > 0 && (
            <Button variant="tertiary" className="h-8" onClick={() => setSyncing(true)}>
              Copy settings from…
            </Button>
          )}
        </div>
      )}
      <CreateEnvironment project={project} open={creating} onClose={() => setCreating(false)} />
      {others.length > 0 && <SyncEnvironment project={project} open={syncing} onClose={() => setSyncing(false)} />}
    </div>
  );
}

function CreateEnvironment({ project, open, onClose }: { project: Project; open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [name, setName] = useState(project.environments.some((e) => e.environment === "staging") ? "" : "staging");
  const [branch, setBranch] = useState("");
  const create = useMutation({
    mutationFn: () => api.post<Project>(`/projects/${project.id}/environments`, { environment: name, branch: branch || null }),
    onSuccess: (created) => {
      toast(`Created ${created.environment}; its services are deploying`);
      queryClient.invalidateQueries({ queryKey: ["projects"] });
      onClose();
      navigate(`/projects/${created.id}`);
    },
  });
  const hasGit = project.services.some((s) => s.kind === "app");

  return (
    <Modal open={open} title="New environment" onClose={onClose}>
      <form onSubmit={(e: FormEvent) => (e.preventDefault(), create.mutate())} className="space-y-5">
        <p className="text-muted">
          Copies the services of <strong className="text-fg">{project.environment}</strong> with their settings and variables into a
          separate environment with its own network, databases and addresses, then deploys it. Data, domains, host ports and previews
          aren't copied.
        </p>
        <Field label="Name" hint={`Its services answer at service-${baseName(project)}-${name || "name"}.<base domain>.`}>
          <Input value={name} onChange={(e) => setName(e.target.value.toLowerCase())} required autoFocus />
        </Field>
        {hasGit && (
          <Field label="Branch" hint="For services built from Git, e.g. develop. Empty keeps each service's own branch.">
            <Input value={branch} onChange={(e) => setBranch(e.target.value.trim())} placeholder="develop" />
          </Field>
        )}
        <ErrorText error={create.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" busy={create.isPending}>
            Create and deploy
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function SyncEnvironment({ project, open, onClose }: { project: Project; open: boolean; onClose: () => void }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const others = project.environments.filter((e) => e.id !== project.id);
  const [from, setFrom] = useState(others[0]?.id ?? "");
  const sync = useMutation({
    mutationFn: () => api.post<{ updated: string[]; added: string[] }>(`/projects/${project.id}/sync`, { from }),
    onSuccess: (r) => {
      const parts = [
        r.updated.length ? `updated ${r.updated.join(", ")}` : "",
        r.added.length ? `added ${r.added.join(", ")}` : "",
      ].filter(Boolean);
      toast(parts.length ? `Copied: ${parts.join("; ")}. Redeploying.` : "Already in step; nothing changed");
      queryClient.invalidateQueries({ queryKey: ["project", project.id] });
      onClose();
    },
  });
  const source = others.find((e) => e.id === from)?.environment ?? "";

  return (
    <Modal open={open} title={`Copy settings into ${project.environment}`} onClose={onClose}>
      <form onSubmit={(e: FormEvent) => (e.preventDefault(), sync.mutate())} className="space-y-5">
        <Field label="From">
          <Select value={from} onChange={(e) => setFrom(e.target.value)}>
            {others.map((e) => (
              <option key={e.id} value={e.id}>
                {e.environment}
              </option>
            ))}
          </Select>
        </Field>
        <div className="grid gap-4 text-sm sm:grid-cols-2">
          <div>
            <p className="mb-1.5 font-medium">Copied from {source}</p>
            <ul className="list-disc space-y-1 pl-5 text-muted">
              <li>Source, build, image, port, health check, limits, schedule and pre-deploy command</li>
              <li>Services {project.environment} doesn't have yet</li>
              <li>Variables it doesn't have yet</li>
            </ul>
          </div>
          <div>
            <p className="mb-1.5 font-medium">Kept in {project.environment}</p>
            <ul className="list-disc space-y-1 pl-5 text-muted">
              <li>Branches, and the values of variables it already has</li>
              <li>Domains, host ports and previews</li>
              <li>Data, and services only it has</li>
            </ul>
          </div>
        </div>
        <p className="text-sm text-muted">Services that change redeploy.</p>
        <ErrorText error={sync.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" busy={sync.isPending}>
            Copy settings
          </Button>
        </div>
      </form>
    </Modal>
  );
}
