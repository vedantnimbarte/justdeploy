import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, duration, timeAgo, type Run, type ServiceDetail } from "../../api";
import { Button, Card, cx, Dot, EmptyState, ErrorText, Input, SectionTitle, StatusDot, useToast } from "../../ui";

type RunList = { next_run_at: string | null; runs: Run[] };

const when = (iso: string) =>
  new Date(iso).toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

export default function Runs({ service }: { service: ServiceDetail }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const runs = useQuery({
    queryKey: ["runs", service.id],
    queryFn: () => api.get<RunList>(`/services/${service.id}/runs`),
    refetchInterval: 3_000,
  });
  const [command, setCommand] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const start = useMutation({
    mutationFn: (command: string | null) => api.post<Run>(`/services/${service.id}/runs`, { command }),
    onSuccess: (run) => {
      toast(run.command ? "Command started" : "Run started");
      setCommand("");
      setSelected(run.id);
      queryClient.invalidateQueries({ queryKey: ["runs", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });

  const deployed = service.current_deployment_id !== null;
  const list = runs.data?.runs ?? [];
  const shown = list.find((r) => r.id === selected) ?? list[0];
  const submit = (e: FormEvent) => {
    e.preventDefault();
    start.mutate(command);
  };

  return (
    <div className="grid gap-8">
      <Card>
        {service.cron_schedule ? (
          <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
            <p>
              Runs on <code className="rounded-control bg-subtle px-1.5 py-0.5 text-[13px]">{service.cron_schedule}</code>, in UTC.{" "}
              <span className="text-muted">
                {runs.data?.next_run_at
                  ? `Next run ${when(runs.data.next_run_at)}.`
                  : service.desired_state === "stopped"
                    ? "Paused while the service is stopped."
                    : ""}
              </span>
            </p>
            <Button variant="secondary" disabled={!deployed} busy={start.isPending && start.variables === null} onClick={() => start.mutate(null)}>
              Run now
            </Button>
          </div>
        ) : (
          <p className="mb-5 max-w-[68ch] text-muted">
            Run a command once, from the image that's live now, with this service's variables and on its project network: a
            seed script, a data fix, a report. For an interactive shell, use Terminal.
          </p>
        )}
        <form onSubmit={submit} className="flex flex-col gap-2 sm:flex-row">
          <label htmlFor="run-command" className="sr-only">
            Command
          </label>
          <Input
            id="run-command"
            className="font-mono text-[14px]"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder="npm run db:seed"
            disabled={!deployed}
            required
          />
          <Button type="submit" disabled={!deployed} busy={start.isPending && start.variables !== null}>
            Run command
          </Button>
        </form>
        {!deployed && <p className="mt-2 text-sm text-muted">Deploy the service once, then you can run commands from its image.</p>}
      </Card>

      {runs.isError && <ErrorText error={runs.error} />}
      {runs.data && list.length === 0 && (
        <EmptyState title="No runs yet">
          {service.cron_schedule ? "The first one starts at the next scheduled time." : "Commands you run show up here with their output."}
        </EmptyState>
      )}
      {list.length > 0 && (
        <div className="grid gap-8 xl:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]">
          <div>
            <SectionTitle>History</SectionTitle>
            <ol className="relative">
              <span className="absolute top-3 bottom-3 left-[15px] w-px bg-line" aria-hidden />
              {list.map((r) => (
                <li key={r.id} className="relative pl-10">
                  <span className="absolute top-[18px] left-[11px] flex size-[9px] items-center justify-center rounded-full bg-page ring-4 ring-page" aria-hidden>
                    <Dot status={r.status} className="size-[9px]" />
                  </span>
                  <button
                    onClick={() => setSelected(r.id)}
                    aria-current={shown?.id === r.id}
                    className={cx("block w-full rounded-panel px-3 py-3 text-left", shown?.id === r.id ? "bg-surface ring-1 ring-line" : "hover:bg-subtle")}
                  >
                    <div className="flex items-baseline justify-between gap-3">
                      {r.command ? (
                        <code className="truncate text-[13px] font-medium">{r.command}</code>
                      ) : (
                        <p className="truncate font-medium">{r.trigger === "schedule" ? "Scheduled run" : "Run now"}</p>
                      )}
                      <span className="shrink-0 text-[13px] text-muted">{timeAgo(r.started_at)}</span>
                    </div>
                    <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted">
                      <StatusDot status={r.status} className="!text-[13px]" />
                      {r.exit_code !== null && r.exit_code !== 0 && <span>Exit code {r.exit_code}</span>}
                      <span>{duration(r.started_at, r.finished_at)}</span>
                    </div>
                  </button>
                </li>
              ))}
            </ol>
          </div>
          {shown && (
            <div className="min-w-0">
              <SectionTitle>Output</SectionTitle>
              <div className="overflow-hidden rounded-panel bg-console text-white">
                <div className="flex items-center justify-between gap-3 border-b border-white/10 px-4 py-3 text-sm">
                  <code className="truncate">{shown.command ?? "Image's own command"}</code>
                  <span className="shrink-0 opacity-70">{shown.status === "running" ? "Running" : `Finished ${timeAgo(shown.finished_at)}`}</span>
                </div>
                <pre className="h-[28rem] overflow-auto px-4 py-3 text-[13px] leading-5 whitespace-pre-wrap break-all">
                  {shown.status === "running" ? (
                    <span className="opacity-60">Output appears here when the run finishes.</span>
                  ) : (
                    shown.output || <span className="opacity-60">No output.</span>
                  )}
                </pre>
              </div>
              {shown.error && <p className="mt-3 text-sm text-err">{shown.error}</p>}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
