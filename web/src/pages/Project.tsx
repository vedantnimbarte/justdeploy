import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { api, kindLabel, timeAgo, type Project, type Service } from "../api";
import { Badge, Band, Button, ButtonLink, ConfirmDelete, EmptyState, ErrorText, PageHeader, StatusDot } from "../ui";

export default function ProjectPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
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

  if (project.isPending) return <Band>Loading…</Band>;
  if (project.isError) return <Band><ErrorText error={project.error} /></Band>;
  const p = project.data;

  return (
    <>
      <PageHeader
        eyebrow={<Link to="/projects">Projects</Link>}
        title={p.name}
        lead={`${p.services.length} service${p.services.length === 1 ? "" : "s"} on the private network ${p.name}.`}
        actions={
          <>
            <ButtonLink to={`/projects/${id}/new`}>Add service</ButtonLink>
            <ButtonLink to={`/templates?project=${id}`} variant="secondary">
              Add from template
            </ButtonLink>
            <Button variant="danger" onClick={() => setDeleting(true)}>
              Delete project
            </Button>
          </>
        }
      />
      <Band tone="soft">
        {p.services.length === 0 ? (
          <EmptyState title="This project is empty" action={<ButtonLink to={`/projects/${id}/new`}>Add service</ButtonLink>}>
            Add an app from a Git repository, a Docker image, or a database.
          </EmptyState>
        ) : (
          <div className="grid gap-8 md:grid-cols-2 xl:grid-cols-3">
            {p.services.map((s) => (
              <ServiceCard key={s.id} service={s} />
            ))}
          </div>
        )}
      </Band>
      <ConfirmDelete
        open={deleting}
        name={p.name}
        what="project"
        consequence={
          <p>
            Every service in <strong>{p.name}</strong> is removed, including database volumes. Backups
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

function ServiceCard({ service: s }: { service: Service }) {
  const d = s.latest_deployment;
  return (
    <div className="flex flex-col rounded-md border border-hairline bg-canvas p-6">
      <div className="flex items-start justify-between gap-3">
        <Link to={`/services/${s.id}`} className="min-w-0 text-ink!">
          <h3 className="truncate text-lg font-extrabold hover:underline">{s.name}</h3>
        </Link>
        <Badge>{kindLabel(s)}</Badge>
      </div>
      <StatusDot status={s.status} className="mt-3" />
      <div className="mt-4 min-h-[44px] text-sm">
        {s.url ? (
          <a href={s.url} target="_blank" rel="noreferrer" className="break-all">
            {s.url.replace(/^https?:\/\//, "")}
          </a>
        ) : s.kind === "database" ? (
          <span className="text-ink/70">Private, reachable as {s.name} inside the project</span>
        ) : (
          <span className="text-ink/70">{s.port ? `Private on port ${s.port}` : "Worker, no port"}</span>
        )}
        {d && (
          <p className="mt-1 truncate text-ink/70">
            {d.commit_sha ? `${d.commit_sha.slice(0, 7)} ` : ""}
            {d.commit_message ?? d.trigger} · {timeAgo(d.created_at)}
          </p>
        )}
      </div>
      <div className="mt-5">
        <ButtonLink to={`/services/${s.id}`} variant="tertiary">
          Open
        </ButtonLink>
      </div>
    </div>
  );
}
