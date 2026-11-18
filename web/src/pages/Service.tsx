import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { api, kindLabel, type ServiceDetail } from "../api";
import { useMe } from "../me";
import { baseName } from "./Environments";
import { Badge, Band, Button, CodeLine, ErrorText, PageHeader, StatusDot, Tabs, useToast } from "../ui";
import Backups from "./service/Backups";
import Deployments from "./service/Deployments";
import Domains from "./service/Domains";
import Env from "./service/Env";
import Logs from "./service/Logs";
import Runs from "./service/Runs";
import ServiceSettings from "./service/ServiceSettings";

// Charts and the terminal are the heaviest code; load them when opened.
const Metrics = lazy(() => import("./service/Metrics"));
const Terminal = lazy(() => import("./service/Terminal"));

type Tab = "deployments" | "runs" | "logs" | "env" | "domains" | "metrics" | "terminal" | "backups" | "settings";

export default function ServicePage() {
  const { id = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const toast = useToast();
  const queryClient = useQueryClient();
  const { can } = useMe();
  const service = useQuery({
    queryKey: ["service", id],
    queryFn: () => api.get<ServiceDetail>(`/services/${id}`),
    refetchInterval: 4_000,
  });
  const action = useMutation({
    mutationFn: (name: string) => api.post(`/services/${id}/actions`, { action: name }),
    onSuccess: (_, name) => {
      toast(name === "stop" ? "Stopping" : name === "deploy" ? "Deployment queued" : "Restarting");
      queryClient.invalidateQueries({ queryKey: ["service", id] });
      queryClient.invalidateQueries({ queryKey: ["deployments", id] });
    },
    onError: (e) => toast(e.message, true),
  });

  if (service.isPending) return <Band>Loading…</Band>;
  if (service.isError) return <Band><ErrorText error={service.error} /></Band>;
  const s = service.data;

  const tabs: { id: Tab; label: string }[] = [
    { id: "deployments", label: "Deployments" },
    ...(s.kind !== "database" ? [{ id: "runs" as Tab, label: "Runs" }] : []),
    { id: "logs", label: "Logs" },
    ...(can("member") ? [{ id: "env" as Tab, label: "Variables" }] : []),
    ...(s.kind !== "database" ? [{ id: "domains" as Tab, label: "Domains" }] : []),
    { id: "metrics", label: "Metrics" },
    ...(can("member") ? [{ id: "terminal" as Tab, label: "Terminal" }] : []),
    ...(s.kind === "database" ? [{ id: "backups" as Tab, label: "Backups" }] : []),
    { id: "settings", label: "Settings" },
  ];
  const requested = params.get("tab") as Tab | null;
  const tab = tabs.some((t) => t.id === requested) ? (requested as Tab) : "deployments";
  const stopped = s.desired_state === "stopped";

  return (
    <>
      <PageHeader
        eyebrow={
          <>
            <Link to="/projects">Projects</Link> / <Link to={`/projects/${s.project.id}`}>
              {baseName(s.project)}
              {s.project.environment !== "production" && ` (${s.project.environment})`}
            </Link>
          </>
        }
        title={s.name}
        actions={
          can("member") && (
            <>
              <Button busy={action.isPending && action.variables === "deploy"} onClick={() => action.mutate("deploy")}>
                {s.kind === "app" ? "Deploy latest" : "Pull and redeploy"}
              </Button>
              <Button variant="secondary" disabled={stopped} onClick={() => action.mutate("restart")}>
                Restart
              </Button>
              <Button variant="secondary" onClick={() => action.mutate(stopped ? "start" : "stop")}>
                {stopped ? "Start" : "Stop"}
              </Button>
            </>
          )
        }
      >
        <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2">
          <StatusDot status={s.status} />
          {(s.copies_desired > 1 || s.replicas > 1) && (
            <span className="text-sm text-muted">
              {s.copies_running} of {s.copies_desired || s.replicas} copies
              {s.autoscale_max !== null && `, autoscaling ${s.autoscale_min}–${s.autoscale_max}`}
            </span>
          )}
          <Badge>{kindLabel(s)}</Badge>
          {s.url && (
            <a href={s.url} target="_blank" rel="noreferrer" className="font-mono text-[13px] break-all">
              {s.url.replace(/^https?:\/\//, "")}
            </a>
          )}
          {s.latest_deployment?.commit_sha && (
            <span className="text-sm text-muted">
              Running <code className="text-[12px] text-fg">{s.latest_deployment.commit_sha.slice(0, 7)}</code>
            </span>
          )}
        </div>
        {s.connection_url && (
          <div className="mt-6 max-w-2xl">
            <p className="mb-2 text-sm font-medium">Connection URL for services in {s.project.name}</p>
            <CodeLine value={s.connection_url} secret />
          </div>
        )}
      </PageHeader>
      <Tabs<Tab> tabs={tabs} active={tab} onChange={(t) => setParams({ tab: t }, { replace: true })} />
      <Band tone="soft">
        {tab === "deployments" && <Deployments service={s} />}
        {tab === "runs" && <Runs service={s} />}
        {tab === "logs" && <Logs service={s} />}
        {tab === "env" && <Env service={s} />}
        {tab === "domains" && <Domains service={s} />}
        <Suspense fallback={<p>Loading…</p>}>
          {tab === "metrics" && <Metrics service={s} />}
          {tab === "terminal" && <Terminal service={s} />}
        </Suspense>
        {tab === "backups" && <Backups service={s} />}
        {tab === "settings" && <ServiceSettings service={s} />}
      </Band>
    </>
  );
}
