import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { api, kindLabel, timeAgo, triggerLabel, type Project, type Service } from "../api";
import { useMe } from "../me";
import { Badge, Band, Button, ButtonLink, ConfirmDelete, EmptyState, ErrorText, PageHeader, StatusDot } from "../ui";
import { EnvEditor } from "./service/Env";

export default function ProjectPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { can } = useMe();
  const [deleting, setDeleting] = useState(false);
  const project = useQuery({
    queryKey: ["project", id],
    queryFn: () => api.get<Project>(`/projects/${id}`),
    refetchInterval: 4_000,
  });
  const remove = useMutation({
    mutationFn: () => api.del(`/projects/${id}`),
    onSuccess: () => navigate("/projects"),
  });

  if (project.isPending) return <Band className="pt-10 text-muted">Loading…</Band>;
  if (project.isError)
    return (
      <Band className="pt-10">
        <ErrorText error={project.error} />
      </Band>
    );
  const p = project.data;

  return (
    <>
      <PageHeader
        eyebrow={<Link to="/projects">Projects</Link>}
        title={p.name}
        lead={`${p.services.length} service${p.services.length === 1 ? "" : "s"} on a private network. Services reach each other by name.`}
        actions={
          can("member") && (
            <>
              <ButtonLink to={`/templates?project=${id}`} variant="secondary">
                Add from template
              </ButtonLink>
              <ButtonLink to={`/projects/${id}/new`}>Add service</ButtonLink>
            </>
          )
        }
      />
      <Band>
        {p.services.length === 0 ? (
          <EmptyState title="This project is empty" action={can("member") && <ButtonLink to={`/projects/${id}/new`}>Add service</ButtonLink>}>
            Add an app from a Git repository, a Docker image, or a database.
          </EmptyState>
        ) : (
          <div className="overflow-hidden rounded-panel border border-line bg-surface">
            <div className="hidden grid-cols-[minmax(0,1.1fr)_8rem_minmax(0,1.2fr)_minmax(0,1.3fr)] gap-6 border-b border-line bg-subtle px-5 py-2.5 text-[13px] font-medium text-muted lg:grid">
              <span>Service</span>
              <span>Status</span>
              <span>Address</span>
              <span>Last deploy</span>
            </div>
            <ul>
              {p.services.map((s) => (
                <ServiceRow key={s.id} service={s} />
              ))}
            </ul>
          </div>
        )}

        {can("member") && (
          <div className="mt-12 max-w-5xl">
            <EnvEditor
              url={`/projects/${id}/env`}
              title="Shared variables"
              intro="Every service in this project gets these, at build and run time. A service's own variable with the same name wins. Saving redeploys the project's running services."
            />
          </div>
        )}

        {can("admin") && (
          <div className="mt-12 flex flex-wrap items-center justify-between gap-4 border-t border-line pt-6">
            <p className="text-sm text-muted">Deleting the project removes every service in it, including database data.</p>
            <Button variant="danger" onClick={() => setDeleting(true)}>
              Delete project
            </Button>
          </div>
        )}
      </Band>
      <ConfirmDelete
        open={deleting}
        name={p.name}
        what="project"
        consequence={
          <p>
            Every service in <strong className="text-fg">{p.name}</strong> is removed, including database volumes. Backups
            already uploaded are kept. This cannot be undone.
          </p>
        }
        busy={remove.isPending}
        onClose={() => setDeleting(false)}
        onConfirm={() => remove.mutate()}
      />
    </>
  );
}

function ServiceRow({ service: s }: { service: Service }) {
  const d = s.latest_deployment;
  const address = s.url ? (
    <a href={s.url} target="_blank" rel="noreferrer" className="font-mono text-[13px] break-all" onClick={(e) => e.stopPropagation()}>
      {s.url.replace(/^https?:\/\//, "")}
    </a>
  ) : s.port ? (
    <span className="font-mono text-[13px] text-muted">
      {s.name}:{s.port}
    </span>
  ) : s.cron_schedule ? (
    <span className="text-sm text-muted">
      Runs on <code className="text-[13px]">{s.cron_schedule}</code>
    </span>
  ) : (
    <span className="text-sm text-muted">Worker, no port</span>
  );

  return (
    <li className="border-b border-line last:border-b-0">
      <div className="relative grid gap-x-6 gap-y-2 px-5 py-4 hover:bg-subtle lg:grid-cols-[minmax(0,1.1fr)_8rem_minmax(0,1.2fr)_minmax(0,1.3fr)] lg:items-center">
        <div className="flex min-w-0 items-center gap-2.5">
          <Link to={`/services/${s.id}`} className="truncate font-semibold no-underline after:absolute after:inset-0">
            {s.name}
          </Link>
          <Badge>{kindLabel(s)}</Badge>
        </div>
        <StatusDot status={s.status} />
        <div className="relative z-10 min-w-0">{address}</div>
        <div className="min-w-0 text-sm">
          {d ? (
            <p className="flex min-w-0 items-baseline gap-2">
              {d.commit_sha && <code className="shrink-0 text-[12px] text-muted">{d.commit_sha.slice(0, 7)}</code>}
              <span className="truncate">{d.commit_message ?? triggerLabel(d.trigger)}</span>
              <span className="ml-auto shrink-0 text-muted">{timeAgo(d.created_at)}</span>
            </p>
          ) : (
            <span className="text-muted">Not deployed yet</span>
          )}
        </div>
      </div>
    </li>
  );
}
