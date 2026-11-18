import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type FormEvent } from "react";
import { api, timeAgo, type Project, type ServiceUptime, type Settings } from "../../api";
import { DnsPanel } from "../../DnsSetup";
import { useMe } from "../../me";
import { Button, Card, Checkbox, CodeLine, cx, EmptyState, ErrorText, Field, Input, SectionTitle, useToast } from "../../ui";

/** How long between two moments, in the largest sensible units. */
function lasted(from: string, to: string | null) {
  const m = Math.max(1, Math.round(((to ? new Date(to).getTime() : Date.now()) - new Date(from).getTime()) / 60_000));
  if (m < 60) return `${m} min`;
  if (m < 48 * 60) return `${Math.floor(m / 60)} h ${m % 60} min`;
  return `${Math.floor(m / 1440)} days`;
}

const pct = (v: number | null) => (v === null ? "–" : `${v >= 99.995 ? 100 : v.toFixed(2)}%`);

/** A bar per day, oldest first: full when all checks passed. */
function dayTone(v: number | null) {
  if (v === null) return "bg-line";
  if (v >= 99.9) return "bg-ok";
  if (v >= 95) return "bg-warn";
  return "bg-err";
}

/** One service's uptime: state, windows, 90 days of bars and recent incidents. Also used on the public page. */
export function UptimeRow({ s, showUrl = true }: { s: ServiceUptime; showUrl?: boolean }) {
  const today = new Date();
  return (
    <div className="px-5 py-5 md:px-6">
      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
        <h3 className="font-semibold">{s.name}</h3>
        {showUrl && s.url && (
          <a href={s.url} target="_blank" rel="noreferrer" className="min-w-0 truncate text-sm text-muted">
            {s.url.replace(/^https?:\/\//, "")}
          </a>
        )}
        <span className="ml-auto inline-flex items-center gap-2 text-sm">
          <span className={cx("size-2 rounded-full", s.up === null ? "bg-off" : s.up ? "bg-ok" : "bg-err")} aria-hidden />
          {s.up === null ? "Not checked yet" : s.up ? "Up" : "Down"}
          {s.latency_ms !== null && s.up && <span className="text-muted tabular-nums">{s.latency_ms} ms</span>}
        </span>
      </div>
      <div className="mt-3 flex h-8 gap-[2px]" role="img" aria-label={`Daily uptime for the last 90 days, ${pct(s.uptime_90d)} overall`}>
        {s.days.map((v, i) => {
          const day = new Date(today);
          day.setDate(today.getDate() - (s.days.length - 1 - i));
          return (
            <span
              key={i}
              title={`${day.toLocaleDateString([], { day: "numeric", month: "short" })}: ${v === null ? "no checks" : pct(v)}`}
              className={cx("min-w-0 flex-1 rounded-[2px]", dayTone(v))}
            />
          );
        })}
      </div>
      <div className="mt-2 flex flex-wrap justify-between gap-x-6 gap-y-1 text-[13px] text-muted tabular-nums">
        <span>90 days ago</span>
        <span className="flex gap-4">
          <span>24 hours {pct(s.uptime_24h)}</span>
          <span>7 days {pct(s.uptime_7d)}</span>
          <span>90 days {pct(s.uptime_90d)}</span>
        </span>
        <span>Today</span>
      </div>
      {s.incidents.length > 0 && (
        <ul className="mt-4 grid gap-1 text-sm">
          {s.incidents.map((i) => (
            <li key={i.from} className="flex flex-wrap gap-x-3">
              <span className={cx(i.to ? "text-muted" : "font-medium text-err")}>{i.to ? "Was down" : "Down now"}</span>
              <span>{new Date(i.from).toLocaleString()}</span>
              <span className="text-muted">
                for {lasted(i.from, i.to)}
                {!i.to && " so far"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export default function Uptime({ project }: { project: Project }) {
  const { can } = useMe();
  const uptime = useQuery({
    queryKey: ["uptime", project.id],
    queryFn: () => api.get<ServiceUptime[]>(`/projects/${project.id}/uptime`),
    refetchInterval: 60_000,
  });
  const last = uptime.data
    ?.map((s) => s.checked_at)
    .filter(Boolean)
    .sort()
    .at(-1);

  return (
    <div className="grid max-w-6xl gap-10">
      <section>
        <SectionTitle>Uptime</SectionTitle>
        <p className="-mt-2 mb-5 max-w-[68ch] text-muted">
          Every public service is asked for its health check path (or <code>/</code>) once a minute. Two misses in a row count as down and
          notify your channels; so does coming back. {last && `Last checked ${timeAgo(last)}.`}
        </p>
        <ErrorText error={uptime.error} />
        {uptime.data?.length === 0 ? (
          <EmptyState title="No public services">Make a service public and it gets checked here every minute.</EmptyState>
        ) : (
          <Card className="!p-0">
            <ul className="divide-y divide-line">
              {uptime.data?.map((s) => (
                <li key={s.id}>
                  <UptimeRow s={s} />
                </li>
              ))}
            </ul>
          </Card>
        )}
      </section>
      {can("member") && uptime.data && uptime.data.length > 0 && <StatusPageSettings project={project} services={uptime.data} />}
    </div>
  );
}

function StatusPageSettings({ project, services }: { project: Project; services: ServiceUptime[] }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ["settings"],
    queryFn: () => api.get<Settings>("/settings"),
  });
  const [enabled, setEnabled] = useState(project.status_page);
  const [domain, setDomain] = useState(project.status_domain ?? "");
  const [picked, setPicked] = useState<Set<string>>(() => new Set(project.services.filter((s) => s.on_status_page).map((s) => s.id)));
  useEffect(() => {
    setEnabled(project.status_page);
    setDomain(project.status_domain ?? "");
  }, [project.status_page, project.status_domain]);

  const save = useMutation({
    mutationFn: () =>
      api.put(`/projects/${project.id}/status-page`, {
        enabled,
        domain: domain.trim() || null,
        services: [...picked],
      }),
    onSuccess: () => {
      toast(enabled ? "Status page published" : "Status page hidden");
      queryClient.invalidateQueries({ queryKey: ["project", project.id] });
    },
    onError: (e) => toast(e.message, true),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate();
  };

  const s = settings.data;
  const scheme = s?.tls ? "https" : "http";
  const generated = s && s.base_domain !== "localhost" ? `status-${project.name}.${s.base_domain}` : null;
  const toggle = (id: string, on: boolean) => {
    const next = new Set(picked);
    if (on) next.add(id);
    else next.delete(id);
    setPicked(next);
  };

  return (
    <section>
      <SectionTitle>Status page</SectionTitle>
      <p className="-mt-2 mb-5 max-w-[68ch] text-muted">
        A public page anyone can open, without signing in, showing the services you pick: whether each is up and its last 90 days.
      </p>
      <Card>
        <form onSubmit={submit} className="grid gap-5">
          <Checkbox label="Publish the status page" checked={enabled} onChange={setEnabled} />
          <fieldset className="grid gap-3" disabled={!enabled}>
            <legend className="mb-2 text-sm font-medium">Services on the page</legend>
            {services.map((svc) => (
              <Checkbox
                key={svc.id}
                label={svc.name}
                checked={picked.has(svc.id)}
                onChange={(on) => toggle(svc.id, on)}
                disabled={!enabled}
              />
            ))}
          </fieldset>
          <Field label="Custom domain" hint={generated ? `Optional. Add a CNAME record pointing it at ${generated}.` : "Optional, like status.example.com."}>
            <Input value={domain} onChange={(e) => setDomain(e.target.value)} placeholder="status.example.com" disabled={!enabled} />
          </Field>
          <div className="flex flex-wrap items-center gap-3">
            <Button type="submit" busy={save.isPending}>
              Save
            </Button>
            <ErrorText error={save.error} />
          </div>
        </form>
        {project.status_page && s && (
          <div className="mt-6 grid gap-3 border-t border-line pt-5">
            <p className="text-sm font-medium">Published at</p>
            <CodeLine value={`${s.public_url.replace(/\/$/, "")}/status/${project.name}`} />
            {generated && <CodeLine value={`${scheme}://${generated}`} />}
          </div>
        )}
      </Card>
      {project.status_page && project.status_domain && s && (
        <Card className="mt-4 !p-0">
          <DnsPanel host={project.status_domain} cnameTarget={generated ?? undefined} tls={s.tls} />
        </Card>
      )}
    </section>
  );
}
