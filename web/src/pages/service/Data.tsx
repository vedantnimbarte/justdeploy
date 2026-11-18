import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { api, bytes, timeAgo, type DataCopy, type Project, type Replica, type Service, type ServiceDetail } from "../../api";
import { useMe } from "../../me";
import { Button, Card, cx, ErrorText, Field, Modal, SectionTitle, Select, useToast } from "../../ui";

/** Copies of a database, its read replicas, and replacing its data with another's. */
export default function Data({ service }: { service: ServiceDetail }) {
  const project = useQuery({
    queryKey: ["project", service.project.id],
    queryFn: () => api.get<Project>(`/projects/${service.project.id}`),
  });
  const siblings = (project.data?.services ?? []).filter(
    (s) => s.kind === "database" && s.engine === service.engine && s.id !== service.id,
  );
  const primary = project.data?.services.find((s) => s.id === service.replica_of);

  if (service.replica_of) {
    return (
      <Card className="max-w-3xl">
        <SectionTitle>Read replica</SectionTitle>
        <p className="max-w-[68ch]">
          This database follows every change made to{" "}
          {primary ? <Link to={`/services/${primary.id}?tab=data`}>{primary.name}</Link> : "its primary"}, usually within a second, and
          takes no writes. Point reports, search and other read-heavy work at the connection URL above to take load off the primary.
        </p>
        <p className="mt-3 max-w-[68ch] text-sm text-muted">Deleting it is safe: the primary stops keeping changes for it.</p>
      </Card>
    );
  }

  return (
    <div className="grid max-w-5xl gap-8">
      <CopyCard service={service} />
      {service.engine === "postgres" && <Replicas service={service} />}
      <Replace service={service} siblings={siblings} />
    </div>
  );
}

function CopyCard({ service }: { service: ServiceDetail }) {
  const { can } = useMe();
  const toast = useToast();
  const navigate = useNavigate();
  const copy = useMutation({
    mutationFn: () => api.post<Service>(`/services/${service.id}/copy`),
    onSuccess: (created) => {
      toast(`Creating ${created.name}; it fills with ${service.name}'s data once it starts`);
      navigate(`/services/${created.id}`);
    },
    onError: (e) => toast(e.message, true),
  });
  return (
    <Card>
      <SectionTitle>Copy this database</SectionTitle>
      <div className="flex flex-wrap items-center justify-between gap-4">
        <p className="max-w-[60ch] text-muted">
          A new database beside this one, with the same engine, version and credentials, filled with its data as it is now. Changes to
          either never reach the other: good for trying a migration or a risky fix first.
        </p>
        {can("member") && (
          <Button busy={copy.isPending} onClick={() => copy.mutate()}>
            Copy {service.name}
          </Button>
        )}
      </div>
    </Card>
  );
}

const REPLICA_STATE: Record<string, { label: string; dot: string }> = {
  streaming: { label: "Following", dot: "bg-ok" },
  catchup: { label: "Catching up", dot: "bg-warn pulse" },
  startup: { label: "Starting", dot: "bg-warn pulse" },
  backup: { label: "Copying", dot: "bg-warn pulse" },
  stopped: { label: "Not connected", dot: "bg-err" },
};

function Replicas({ service }: { service: ServiceDetail }) {
  const { can } = useMe();
  const toast = useToast();
  const navigate = useNavigate();
  const replicas = useQuery({
    queryKey: ["replicas", service.id],
    queryFn: () => api.get<Replica[]>(`/services/${service.id}/replicas`),
    refetchInterval: 10_000,
  });
  const add = useMutation({
    mutationFn: () => api.post<Service>(`/services/${service.id}/replicas`),
    onSuccess: (created) => {
      toast(`Creating ${created.name}; it copies ${service.name} and then follows it`);
      navigate(`/services/${created.id}`);
    },
    onError: (e) => toast(e.message, true),
  });

  return (
    <Card className="!p-0">
      <div className="flex flex-wrap items-start justify-between gap-4 px-5 pt-5 md:px-6 md:pt-6">
        <div>
          <h2 className="text-lg font-semibold">Read replicas</h2>
          <p className="mt-1 max-w-[60ch] text-muted">
            Read-only copies that follow every change through streaming replication, each with its own connection URL.
          </p>
        </div>
        {can("member") && (
          <Button variant="secondary" busy={add.isPending} onClick={() => add.mutate()}>
            Add a read replica
          </Button>
        )}
      </div>
      {replicas.data?.length ? (
        <ul className="mt-5 divide-y divide-line border-t border-line">
          {replicas.data.map((r) => {
            const state = REPLICA_STATE[r.state] ?? { label: r.state, dot: "bg-off" };
            return (
              <li key={r.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-5 py-3 md:px-6">
                <Link to={`/services/${r.id}`} className="font-medium">
                  {r.name}
                </Link>
                <span className="inline-flex items-center gap-2 text-sm">
                  <span className={cx("size-2 rounded-full", state.dot)} aria-hidden />
                  {state.label}
                </span>
                {r.lag_bytes !== null && (
                  <span className="text-sm text-muted tabular-nums">
                    {r.lag_bytes === 0 ? "up to date" : `${bytes(r.lag_bytes)} behind`}
                    {r.lag_seconds !== null && r.lag_seconds >= 1 && `, ${r.lag_seconds.toFixed(0)}s`}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="px-5 pt-4 pb-5 text-sm text-muted md:px-6 md:pb-6">None yet.</p>
      )}
      <ErrorText error={replicas.error} />
    </Card>
  );
}

const COPY_STATUS: Record<DataCopy["status"], { label: string; dot: string }> = {
  pending: { label: "Waiting to start", dot: "bg-warn" },
  running: { label: "Copying", dot: "bg-warn pulse" },
  succeeded: { label: "Done", dot: "bg-ok" },
  failed: { label: "Failed", dot: "bg-err" },
};

function Replace({ service, siblings }: { service: ServiceDetail; siblings: Service[] }) {
  const { can } = useMe();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [from, setFrom] = useState("");
  const [confirming, setConfirming] = useState(false);
  const copies = useQuery({
    queryKey: ["copies", service.id],
    queryFn: () => api.get<DataCopy[]>(`/services/${service.id}/copies`),
    refetchInterval: (q) => (q.state.data?.some((c) => c.status === "running" || c.status === "pending") ? 2_000 : 30_000),
  });
  const replace = useMutation({
    mutationFn: (source: string) => api.post(`/services/${service.id}/copies`, { from: source }),
    onSuccess: () => {
      setConfirming(false);
      toast(`Replacing ${service.name}'s data`);
      queryClient.invalidateQueries({ queryKey: ["copies", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });
  const source = siblings.find((s) => s.id === (from || siblings[0]?.id));
  const history = copies.data ?? [];
  if (!can("admin") && history.length === 0) return null;

  return (
    <Card>
      <SectionTitle>Replace its data</SectionTitle>
      <p className="-mt-2 mb-5 max-w-[68ch] text-muted">
        Fill {service.name} with another database's data: say, a point-in-time copy from before something went wrong. Tables in the copy are
        dropped and recreated here; anything else stays.
      </p>
      {can("admin") &&
        (siblings.length === 0 ? (
          <p className="text-sm text-muted">There's no other {service.engine} database in this project to copy from.</p>
        ) : (
          <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
            <Field label="Copy from" className="sm:w-72">
              <Select value={from || siblings[0]?.id} onChange={(e) => setFrom(e.target.value)}>
                {siblings.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </Select>
            </Field>
            <Button variant="danger" onClick={() => setConfirming(true)}>
              Replace data
            </Button>
          </div>
        ))}
      {history.length > 0 && (
        <ul className="mt-6 grid gap-2 border-t border-line pt-4 text-sm">
          {history.map((c) => {
            const st = COPY_STATUS[c.status];
            return (
              <li key={c.id} className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className={cx("size-2 rounded-full", st.dot)} aria-hidden />
                <span className="font-medium">{st.label}</span>
                <span className="text-muted">
                  from {c.source ?? "a deleted database"}, {timeAgo(c.created_at)}
                </span>
                {c.error && <span className="w-full text-err">{c.error}</span>}
              </li>
            );
          })}
        </ul>
      )}
      <Modal open={confirming} title={`Replace ${service.name}'s data?`} onClose={() => setConfirming(false)}>
        <div className="space-y-4">
          <p>
            Everything in <strong>{source?.name}</strong> is copied into <strong>{service.name}</strong>. Tables (or collections) in it are
            dropped and recreated in {service.name}, so their current rows there are lost. Apps keep running against {service.name} while it
            copies.
          </p>
          <p className="text-sm text-muted">Back it up first if you may want today's data back.</p>
          <div className="flex justify-end gap-3">
            <Button variant="secondary" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button variant="danger" busy={replace.isPending} onClick={() => source && replace.mutate(source.id)}>
              Replace data
            </Button>
          </div>
        </div>
      </Modal>
    </Card>
  );
}
