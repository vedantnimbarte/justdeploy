import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState, type FormEvent } from "react";
import { api, bytes, timeAgo, type ClusterServer, type Settings } from "../api";
import { Step } from "../DnsSetup";
import { useMe } from "../me";
import { Badge, Band, Button, Card, CodeLine, cx, ErrorText, Field, Modal, PageHeader, SectionTitle, Textarea, useToast } from "../ui";

export default function ServersPage() {
  const { can } = useMe();
  const [adding, setAdding] = useState(false);
  const servers = useQuery({
    queryKey: ["servers"],
    queryFn: () => api.get<ClusterServer[]>("/servers"),
    refetchInterval: adding ? 4_000 : 15_000,
  });
  const list = servers.data ?? [];

  return (
    <>
      <PageHeader
        title="Servers"
        lead="The machines your services run on. Copies spread across them, and move off a server that goes down."
        actions={can("admin") && !adding && <Button onClick={() => setAdding(true)}>Add a server</Button>}
      />
      <Band className="grid max-w-5xl gap-6">
        {adding && <AddServer count={list.length} servers={list} onClose={() => setAdding(false)} />}
        {servers.isError && <ErrorText error={servers.error} />}
        <Card className="!p-0">
          <ul className="divide-y divide-line">
            {list.map((s) => (
              <ServerRow key={s.id} server={s} />
            ))}
          </ul>
        </Card>
        {list.length === 1 && !adding && (
          <p className="max-w-[68ch] text-sm text-muted">
            One server for now. Add another and services with more than one copy spread across both; databases stay with their data.
          </p>
        )}
      </Band>
    </>
  );
}

function ServerRow({ server: s }: { server: ClusterServer }) {
  const { can } = useMe();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [labelling, setLabelling] = useState(false);
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["servers"] });
  const update = useMutation({
    mutationFn: (body: { availability?: string; labels?: Record<string, string> }) => api.patch(`/servers/${s.id}`, body),
    onSuccess: (_, body) => {
      toast(body.availability === "drain" ? "Draining: its copies are moving to other servers" : "Saved");
      refresh();
      setLabelling(false);
    },
    onError: (e) => toast(e.message, true),
  });
  const remove = useMutation({
    mutationFn: () => api.del(`/servers/${s.id}`),
    onSuccess: () => {
      toast(`${s.hostname} is out of the cluster`);
      refresh();
    },
    onError: (e) => toast(e.message, true),
  });
  const up = s.state === "ready";
  const u = s.usage;

  return (
    <li className="grid gap-4 px-5 py-5 md:px-6 lg:grid-cols-[minmax(0,1.2fr)_minmax(0,2fr)_auto] lg:items-center">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className={cx("size-2 shrink-0 rounded-full", up ? "bg-ok" : "bg-err")} aria-hidden />
          <p className="truncate font-semibold">{s.hostname}</p>
          {s.this && <Badge>justdeploy runs here</Badge>}
          {s.availability !== "active" && <Badge>{s.availability === "drain" ? "Draining" : "Paused"}</Badge>}
        </div>
        <p className="mt-1 text-sm text-muted">
          {up ? "Up" : `Down (${s.state})`}, {s.address}, {s.cpus} CPU{s.cpus === 1 ? "" : "s"}, {bytes(s.memory_bytes)}
        </p>
        <p className="mt-0.5 text-[13px] text-faint">
          {s.platform}, Docker {s.docker_version}, {s.tasks} cop{s.tasks === 1 ? "y" : "ies"} running
        </p>
        {Object.keys(s.labels).length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {Object.entries(s.labels).map(([k, v]) => (
              <code key={k} className="rounded-control bg-subtle px-1.5 py-0.5 text-[12px]">
                {k}={v}
              </code>
            ))}
          </div>
        )}
      </div>
      <div className="grid grid-cols-3 gap-4">
        <Meter label="CPU" percent={u?.cpu_percent} detail={u ? `${u.cpu_percent.toFixed(0)}%` : "…"} />
        <Meter
          label="Memory"
          percent={u ? (u.memory_used / Math.max(1, u.memory_total)) * 100 : undefined}
          detail={u ? `${bytes(u.memory_used)} of ${bytes(u.memory_total)}` : "…"}
        />
        <Meter
          label="Disk"
          percent={u ? (u.disk_used / Math.max(1, u.disk_total)) * 100 : undefined}
          detail={u ? `${bytes(u.disk_used)} of ${bytes(u.disk_total)}` : "…"}
        />
        {u && <p className="col-span-3 -mt-2 text-[12px] text-faint">Sampled {timeAgo(u.sampled_at)}</p>}
      </div>
      {can("admin") && (
        <div className="flex flex-wrap gap-1 lg:justify-end">
          <Button variant="tertiary" className="h-8" onClick={() => setLabelling(true)}>
            Labels
          </Button>
          {!s.this &&
            (s.availability === "active" ? (
              <Button variant="tertiary" className="h-8" busy={update.isPending} onClick={() => update.mutate({ availability: "drain" })}>
                Drain
              </Button>
            ) : (
              <Button variant="tertiary" className="h-8" busy={update.isPending} onClick={() => update.mutate({ availability: "active" })}>
                Activate
              </Button>
            ))}
          {!s.this && !up && (
            <Button variant="tertiary" className="h-8" busy={remove.isPending} onClick={() => remove.mutate()}>
              Remove
            </Button>
          )}
        </div>
      )}
      <LabelEditor server={s} open={labelling} onClose={() => setLabelling(false)} onSave={(labels) => update.mutate({ labels })} busy={update.isPending} />
    </li>
  );
}

/** A thin bar; colour only when it's getting full. */
function Meter({ label, percent, detail }: { label: string; percent?: number; detail: string }) {
  const p = Math.min(100, Math.max(0, percent ?? 0));
  return (
    <div className="min-w-0">
      <div className="flex items-baseline justify-between gap-2 text-[13px]">
        <span className="text-muted">{label}</span>
      </div>
      <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-subtle" role="meter" aria-label={label} aria-valuenow={Math.round(p)} aria-valuemin={0} aria-valuemax={100}>
        <div className={cx("h-full rounded-full", p > 95 ? "bg-err" : p > 80 ? "bg-warn" : "bg-fg/70")} style={{ width: `${p}%` }} />
      </div>
      <p className="mt-1 truncate text-[12px] text-muted">{detail}</p>
    </div>
  );
}

function LabelEditor({
  server,
  open,
  onClose,
  onSave,
  busy,
}: {
  server: ClusterServer;
  open: boolean;
  onClose: () => void;
  onSave: (labels: Record<string, string>) => void;
  busy: boolean;
}) {
  const [text, setText] = useState(() =>
    Object.entries(server.labels)
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
  );
  const [error, setError] = useState<string | null>(null);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const labels: Record<string, string> = {};
    for (const line of text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)) {
      const [k, ...v] = line.split("=");
      if (!k?.trim() || v.length === 0) return setError(`${line}: write it as key=value`);
      labels[k.trim()] = v.join("=").trim();
    }
    setError(null);
    onSave(labels);
  };
  return (
    <Modal open={open} title={`Labels on ${server.hostname}`} onClose={onClose}>
      <form onSubmit={submit} className="space-y-4">
        <p className="text-muted">
          A service with a server label (in its Settings) only runs on servers that have it: <code>region=eu</code>, <code>disk=ssd</code>,{" "}
          <code>gpu=yes</code>.
        </p>
        <Field label="One key=value per line">
          <Textarea rows={5} value={text} onChange={(e) => setText(e.target.value)} placeholder={"region=eu\ndisk=ssd"} />
        </Field>
        {error && <p className="text-sm text-err">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" busy={busy}>
            Save labels
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** The steps to add a server, with a live wait for it to show up. */
function AddServer({ count, servers, onClose }: { count: number; servers: ClusterServer[]; onClose: () => void }) {
  const join = useQuery({ queryKey: ["server-join"], queryFn: () => api.get<{ command: string; manager_address: string }>("/servers/join") });
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api.get<Settings>("/settings") });
  // Whatever is in the list when the panel opens is "before"; anything new joined.
  const before = useRef<Set<string> | null>(null);
  if (before.current === null && count > 0) before.current = new Set(servers.map((s) => s.id));
  const joined = before.current ? servers.filter((s) => !before.current?.has(s.id)) : [];
  const address = join.data?.manager_address ?? "this server";

  return (
    <Card>
      <SectionTitle
        action={
          <Button variant="tertiary" className="h-8" onClick={onClose}>
            {joined.length ? "Done" : "Close"}
          </Button>
        }
      >
        Add a server
      </SectionTitle>
      <ol className="grid max-w-3xl">
        <Step n={1} state={joined.length ? "done" : "current"} title="Install Docker on the new server">
          <p className="text-muted">
            Any Linux server with Docker 24 or newer, reachable from this one. It needs nothing else: justdeploy keeps running here and
            sends work to it.
          </p>
        </Step>
        <Step n={2} state={joined.length ? "done" : "current"} title="Open the cluster ports between the servers">
          <ul className="grid gap-1 text-muted">
            <li>
              <code className="text-fg">2377/tcp</code> to {address}, for joining and management
            </li>
            <li>
              <code className="text-fg">7946/tcp</code>, <code className="text-fg">7946/udp</code> and{" "}
              <code className="text-fg">4789/udp</code> between all servers, for the network between them
            </li>
          </ul>
          <p className="mt-2 text-[13px] text-muted">Keep them closed to the internet: only your servers should reach them.</p>
        </Step>
        <Step n={3} state={joined.length ? "done" : "current"} title="Run this on the new server">
          {join.isError ? (
            <ErrorText error={join.error} />
          ) : (
            <CodeLine value={join.data?.command ?? "Loading…"} secret />
          )}
          <p className="mt-2 text-[13px] text-muted">The token in it lets a machine join, so treat it like a password.</p>
        </Step>
        <Step n={4} state={joined.length ? "done" : "todo"} title={joined.length ? "Joined" : "Waiting for it to join"} last>
          {joined.length ? (
            <p>
              <strong>{joined.map((s) => s.hostname).join(", ")}</strong> joined. New copies can run there now; existing ones move the next
              time their service deploys.
            </p>
          ) : (
            <p className="flex items-center gap-2 text-muted" aria-live="polite">
              <span className="pulse size-2 rounded-full bg-warn" aria-hidden />
              Checking every few seconds…
            </p>
          )}
        </Step>
      </ol>
      {settings.data && !settings.data.tls && (
        <p className="mt-6 rounded-control bg-subtle p-4 text-sm">
          <strong>HTTPS is off</strong>, so apps built from Git keep running on this server: other servers pull built images from the
          cluster's registry at <code>registry.{settings.data.base_domain}</code>, which needs a certificate. Images from Docker Hub and
          other registries run anywhere.
        </p>
      )}
    </Card>
  );
}
