import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api, duration, timeAgo, type Deployment, type ServiceDetail } from "../../api";
import { Badge, Button, EmptyState, ErrorText, SectionTitle, StatusDot, useToast } from "../../ui";
import { LogPanel } from "./LogPanel";

export default function Deployments({ service }: { service: ServiceDetail }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const deployments = useQuery({
    queryKey: ["deployments", service.id],
    queryFn: () => api.get<Deployment[]>(`/services/${service.id}/deployments`),
    refetchInterval: 3_000,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const rollback = useMutation({
    mutationFn: (id: string) => api.post(`/deployments/${id}/rollback`),
    onSuccess: () => {
      toast("Rolling back");
      queryClient.invalidateQueries({ queryKey: ["deployments", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });

  // Open the newest deployment's log by default.
  const newest = deployments.data?.[0]?.id ?? null;
  useEffect(() => {
    if (selected === null && newest) setSelected(newest);
  }, [newest, selected]);

  if (deployments.isPending) return <p>Loading…</p>;
  if (deployments.isError) return <ErrorText error={deployments.error} />;
  if (deployments.data.length === 0) return <EmptyState title="No deployments yet" />;

  const previewHost = (pr: number) =>
    service.generated_host ? `${window.location.protocol}//pr-${pr}-${service.generated_host}` : null;

  return (
    <div className="grid gap-8 xl:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]">
      <div>
        <SectionTitle>History</SectionTitle>
        <ul className="overflow-hidden rounded-md border border-hairline bg-canvas">
          {deployments.data.map((d) => (
            <li key={d.id} className="border-b border-divider last:border-b-0">
              <button
                onClick={() => setSelected(d.id)}
                className={`block w-full px-5 py-4 text-left ${selected === d.id ? "bg-canvas-tint" : "hover:bg-canvas-soft"}`}
              >
                <div className="flex items-center justify-between gap-3">
                  <StatusDot status={d.status} />
                  <span className="text-sm text-ink/70">{timeAgo(d.created_at)}</span>
                </div>
                <p className="mt-2 truncate font-medium">
                  {d.commit_message || (d.image_ref && !d.image_ref.startsWith("jd-") ? d.image_ref : "Deployment")}
                </p>
                <div className="mt-2 flex flex-wrap items-center gap-2 text-sm text-ink/70">
                  {d.commit_sha && <code>{d.commit_sha.slice(0, 7)}</code>}
                  <Badge>{d.trigger}</Badge>
                  {d.kind === "preview" && <Badge>PR #{d.pr_number}</Badge>}
                  {d.started_at && <span>{duration(d.started_at, d.finished_at)}</span>}
                </div>
              </button>
              {(d.kind === "preview" && d.status === "running" && d.pr_number && previewHost(d.pr_number)) ||
              (d.kind === "production" && d.image_ref && ["superseded", "running"].includes(d.status) && d.id !== service.current_deployment_id) ? (
                <div className="flex gap-3 px-5 pb-4">
                  {d.kind === "preview" && d.pr_number && (
                    <a href={previewHost(d.pr_number)!} target="_blank" rel="noreferrer" className="text-sm">
                      Open preview
                    </a>
                  )}
                  {d.kind === "production" && (
                    <Button variant="tertiary" className="!py-2" busy={rollback.isPending} onClick={() => rollback.mutate(d.id)}>
                      Roll back to this
                    </Button>
                  )}
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      </div>
      <div className="min-w-0">
        <SectionTitle>Log</SectionTitle>
        {selected ? (
          <>
            <LogPanel url={`/api/v1/deployments/${selected}/logs`} title="Build and release" />
            {deployments.data.find((d) => d.id === selected)?.error && (
              <p className="mt-3 text-sm text-error">{deployments.data.find((d) => d.id === selected)?.error}</p>
            )}
          </>
        ) : (
          <p>Select a deployment.</p>
        )}
      </div>
    </div>
  );
}
