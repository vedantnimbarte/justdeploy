import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api, duration, timeAgo, type Deployment, type ServiceDetail } from "../../api";
import { Badge, Button, cx, Dot, EmptyState, ErrorText, SectionTitle, StatusDot, useToast } from "../../ui";
import { LogPanel } from "./LogPanel";

/** What started a deployment, in words. */
const TRIGGERS: Record<string, string> = {
  create: "First deploy",
  manual: "Deployed by hand",
  webhook: "Git push",
  rollback: "Rollback",
  env: "Variables changed",
  domains: "Domains changed",
  settings: "Settings changed",
  reconcile: "Recovered",
};

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
        <ol className="relative">
          {/* The rail the timeline's dots sit on. */}
          <span className="absolute top-3 bottom-3 left-[15px] w-px bg-line" aria-hidden />
          {deployments.data.map((d) => {
            const canRollback =
              d.kind === "production" &&
              d.image_ref &&
              ["superseded", "running"].includes(d.status) &&
              d.id !== service.current_deployment_id;
            const preview = d.kind === "preview" && d.status === "running" && d.pr_number ? previewHost(d.pr_number) : null;
            return (
              <li key={d.id} className="relative pl-10">
                <span className="absolute top-[18px] left-[11px] flex size-[9px] items-center justify-center rounded-full bg-page ring-4 ring-page" aria-hidden>
                  <Dot status={d.status} className="size-[9px]" />
                </span>
                <button
                  onClick={() => setSelected(d.id)}
                  aria-current={selected === d.id}
                  className={cx(
                    "block w-full rounded-panel px-3 py-3 text-left",
                    selected === d.id ? "bg-surface ring-1 ring-line" : "hover:bg-subtle",
                  )}
                >
                  <div className="flex items-baseline justify-between gap-3">
                    <p className="truncate font-medium">
                      {d.commit_message || (d.image_ref && !d.image_ref.startsWith("jd-") ? d.image_ref : "Deployment")}
                    </p>
                    <span className="shrink-0 text-[13px] text-muted">{timeAgo(d.created_at)}</span>
                  </div>
                  <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted">
                    <StatusDot status={d.status} className="!text-[13px]" />
                    {d.commit_sha && <code className="text-[12px]">{d.commit_sha.slice(0, 7)}</code>}
                    <span>{TRIGGERS[d.trigger] ?? d.trigger}</span>
                    {d.kind === "preview" && <Badge>Preview of PR #{d.pr_number}</Badge>}
                    {d.started_at && <span>{duration(d.started_at, d.finished_at)}</span>}
                  </div>
                </button>
                {(canRollback || preview) && (
                  <div className="flex gap-2 px-3 pb-2">
                    {preview && (
                      <a href={preview} target="_blank" rel="noreferrer" className="text-sm">
                        Open preview
                      </a>
                    )}
                    {canRollback && (
                      <Button variant="secondary" className="h-8" busy={rollback.isPending} onClick={() => rollback.mutate(d.id)}>
                        Roll back to this
                      </Button>
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ol>
      </div>
      <div className="min-w-0">
        <SectionTitle>Log</SectionTitle>
        {selected ? (
          <>
            <LogPanel url={`/api/v1/deployments/${selected}/logs`} title="Build and release log" />
            {deployments.data.find((d) => d.id === selected)?.error && (
              <p className="mt-3 text-sm text-err">{deployments.data.find((d) => d.id === selected)?.error}</p>
            )}
          </>
        ) : (
          <p className="text-muted">Pick a deployment to see its log.</p>
        )}
      </div>
    </div>
  );
}
