import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { api, bytes, timeAgo, type Backup, type Destination, type PitrStatus, type Schedule, type ServiceDetail } from "../../api";
import { Button, Card, EmptyState, ErrorText, Field, Input, Modal, SectionTitle, Select, StatusDot, useToast } from "../../ui";

export default function Backups({ service }: { service: ServiceDetail }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const destinations = useQuery({
    queryKey: ["destinations"],
    queryFn: () => api.get<Destination[]>("/backup-destinations"),
  });
  const backups = useQuery({
    queryKey: ["backups", service.id],
    queryFn: () => api.get<Backup[]>(`/services/${service.id}/backups`),
    refetchInterval: 4_000,
  });
  const schedule = useQuery({
    queryKey: ["schedule", service.id],
    queryFn: () => api.get<Schedule | null>(`/services/${service.id}/backup-schedule`),
  });
  const [destination, setDestination] = useState("");
  const [restoring, setRestoring] = useState<Backup | null>(null);

  useEffect(() => {
    if (!destination && destinations.data?.[0]) setDestination(destinations.data[0].id);
  }, [destinations.data, destination]);

  const backupNow = useMutation({
    mutationFn: () =>
      api.post(`/services/${service.id}/backups`, {
        destination_id: destination,
      }),
    onSuccess: () => {
      toast("Backup started");
      queryClient.invalidateQueries({ queryKey: ["backups", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });
  const restore = useMutation({
    mutationFn: (id: string) => api.post(`/backups/${id}/restore`),
    onSuccess: () => {
      setRestoring(null);
      toast("Restore queued");
      queryClient.invalidateQueries({ queryKey: ["backups", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });

  if (destinations.data && destinations.data.length === 0) {
    return (
      <EmptyState title="No backup destination yet" action={<Link to="/settings">Add one in Settings</Link>}>
        Backups go to S3-compatible storage (AWS, Cloudflare R2, Backblaze B2, MinIO) or to a folder on this server.
      </EmptyState>
    );
  }
  const name = (id: string | null) => destinations.data?.find((d) => d.id === id)?.name ?? "deleted destination";

  return (
    <div className="grid gap-8 xl:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
      <div className="min-w-0">
        <SectionTitle>Backups</SectionTitle>
        <Card className="!p-0">
          {backups.data?.length ? (
            <ul className="divide-y divide-line">
              {backups.data.map((b) => (
                <li key={b.id} className="flex flex-wrap items-center justify-between gap-3 px-6 py-4">
                  <div className="min-w-0">
                    <StatusDot status={b.status} />
                    <p className="mt-1 truncate font-mono text-sm">{b.object_key}</p>
                    <p className="text-sm text-muted">
                      {timeAgo(b.created_at)}, {b.trigger === "schedule" ? "scheduled" : "by hand"}, to {name(b.destination_id)}
                      {b.size_bytes != null && `, ${bytes(b.size_bytes)}`}
                    </p>
                    {b.error && <p className="text-sm text-err">{b.error}</p>}
                    {b.restore_status && (
                      <p className={`text-sm ${b.restore_status === "failed" ? "text-err" : ""}`}>
                        Restore {b.restore_status}
                        {b.restored_at && ` ${timeAgo(b.restored_at)}`}
                        {b.restore_error && `: ${b.restore_error}`}
                      </p>
                    )}
                  </div>
                  {b.status === "succeeded" && (
                    <Button variant="secondary" className="h-8" onClick={() => setRestoring(b)}>
                      Restore
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-6 py-4 text-muted">No backups yet.</p>
          )}
        </Card>
      </div>
      <div className="grid content-start gap-8">
        {service.engine === "postgres" && !service.replica_of && destinations.data && (
          <Pitr service={service} destinations={destinations.data} />
        )}
        <Card>
          <h3 className="mb-4 text-lg font-semibold">Back up now</h3>
          <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
            <Field label="Destination" className="flex-1">
              <Select value={destination} onChange={(e) => setDestination(e.target.value)}>
                {destinations.data?.map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.name}
                  </option>
                ))}
              </Select>
            </Field>
            <Button busy={backupNow.isPending} disabled={!destination} onClick={() => backupNow.mutate()}>
              Back up
            </Button>
          </div>
        </Card>
        {schedule.isSuccess && destinations.data && (
          <ScheduleForm service={service} current={schedule.data} destinations={destinations.data} />
        )}
      </div>
      <Modal open={restoring !== null} title="Restore this backup?" onClose={() => setRestoring(null)}>
        <div className="space-y-4">
          <p>
            The data in <strong>{service.name}</strong> is replaced with <code className="break-all">{restoring?.object_key}</code>.
            {service.engine === "redis"
              ? " Redis is stopped while its data file is swapped, then started again."
              : " Tables and collections in the dump are dropped and recreated."}{" "}
            Take a fresh backup first if you may want the current data back.
          </p>
          <div className="flex justify-end gap-3">
            <Button variant="secondary" onClick={() => setRestoring(null)}>
              Cancel
            </Button>
            <Button variant="danger" busy={restore.isPending} onClick={() => restoring && restore.mutate(restoring.id)}>
              Restore
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}

function ScheduleForm({
  service,
  current,
  destinations,
}: {
  service: ServiceDetail;
  current: Schedule | null;
  destinations: Destination[];
}) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const [form, setForm] = useState<Schedule>(
    current ?? {
      destination_id: destinations[0]?.id ?? "",
      cron: "0 3 * * *",
      keep: 7,
    },
  );
  const save = useMutation({
    mutationFn: (value: Schedule | null) => api.put(`/services/${service.id}/backup-schedule`, value),
    onSuccess: (_, value) => {
      toast(value ? "Schedule saved" : "Schedule removed");
      queryClient.invalidateQueries({ queryKey: ["schedule", service.id] });
    },
  });
  return (
    <Card>
      <h3 className="mb-1 text-lg font-semibold">Schedule</h3>
      <p className="mb-4 text-sm text-muted">
        {current ? `Runs on "${current.cron}" (UTC), keeping ${current.keep}.` : "No scheduled backups."}
      </p>
      <div className="grid gap-4">
        <Field label="Destination">
          <Select value={form.destination_id} onChange={(e) => setForm({ ...form, destination_id: e.target.value })}>
            {destinations.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
              </option>
            ))}
          </Select>
        </Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="When (cron, UTC)" hint="0 3 * * * is 03:00 daily">
            <Input className="font-mono" value={form.cron} onChange={(e) => setForm({ ...form, cron: e.target.value })} />
          </Field>
          <Field label="Keep" hint="Older backups are deleted">
            <Input type="number" min={1} max={365} value={form.keep} onChange={(e) => setForm({ ...form, keep: Number(e.target.value) })} />
          </Field>
        </div>
        <ErrorText error={save.error} />
        <div className="flex gap-3">
          <Button busy={save.isPending} onClick={() => save.mutate(form)}>
            Save schedule
          </Button>
          {current && (
            <Button variant="secondary" onClick={() => save.mutate(null)}>
              Turn off
            </Button>
          )}
        </div>
      </div>
    </Card>
  );
}

/** Local date-time for a datetime-local input. */
const toLocalInput = (d: Date) => new Date(d.getTime() - d.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);

/** Continuous archiving with wal-g, and restoring any moment into a new database. Postgres only. */
function Pitr({ service, destinations }: { service: ServiceDetail; destinations: Destination[] }) {
  const toast = useToast();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const s3 = destinations.filter((d) => d.kind === "s3");
  const status = useQuery({
    queryKey: ["pitr", service.id],
    queryFn: () => api.get<PitrStatus>(`/services/${service.id}/pitr`),
    refetchInterval: 30_000,
  });
  const [choice, setChoice] = useState(service.pitr_destination_id ?? s3[0]?.id ?? "");
  const [target, setTarget] = useState(() => toLocalInput(new Date(Date.now() - 5 * 60_000)));
  const configure = useMutation({
    mutationFn: (destination_id: string | null) => api.put(`/services/${service.id}/pitr`, { destination_id }),
    onSuccess: (_, id) => {
      toast(id ? "Archiving on; the database is redeploying" : "Archiving off; the database is redeploying");
      queryClient.invalidateQueries({ queryKey: ["service", service.id] });
      queryClient.invalidateQueries({ queryKey: ["pitr", service.id] });
    },
    onError: (e) => toast(e.message, true),
  });
  const restore = useMutation({
    mutationFn: () =>
      api.post<{ id: string; name: string }>(`/services/${service.id}/pitr/restore`, { target: new Date(target).toISOString() }),
    onSuccess: (created) => {
      toast(`Restoring into ${created.name}`);
      navigate(`/services/${created.id}`);
    },
    onError: (e) => toast(e.message, true),
  });

  const p = status.data;
  const on = Boolean(service.pitr_destination_id);
  return (
    <Card>
      <h3 className="mb-1 text-lg font-semibold">Point-in-time recovery</h3>
      <p className="mb-4 text-sm text-muted">
        Every change is archived to S3 within a minute, with a full base backup each night (the last 7 are kept). Restoring picks any moment
        since the oldest base backup and builds a new database from it; this one stays as it is. To put that moment back here, use{" "}
        <em>Replace its data</em> under Copies and replicas.
      </p>
      {!on ? (
        s3.length === 0 ? (
          <p className="text-sm">
            Needs an S3 destination. <Link to="/settings">Add one in Settings</Link>.
          </p>
        ) : (
          <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
            <Field label="Archive to" className="flex-1">
              <Select value={choice} onChange={(e) => setChoice(e.target.value)}>
                {s3.map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.name}
                  </option>
                ))}
              </Select>
            </Field>
            <Button busy={configure.isPending} disabled={!choice} onClick={() => configure.mutate(choice)}>
              Turn on
            </Button>
          </div>
        )
      ) : (
        <div className="grid gap-4">
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            <dt className="text-muted">Archiving to</dt>
            <dd>{destinations.find((d) => d.id === service.pitr_destination_id)?.name}</dd>
            <dt className="text-muted">Last change archived</dt>
            <dd>{p?.archived_at ? timeAgo(p.archived_at) : "Not yet"}</dd>
            <dt className="text-muted">Last base backup</dt>
            <dd>{p?.base_backup_at ? timeAgo(p.base_backup_at) : "Not yet, within 10 minutes"}</dd>
            <dt className="text-muted">Restorable from</dt>
            <dd>{p?.earliest ? new Date(p.earliest).toLocaleString() : "After the first base backup"}</dd>
          </dl>
          {p && p.failed_count > 0 && (
            <p className="text-sm text-err">
              {p.failed_count} archive attempt{p.failed_count === 1 ? "" : "s"} failed
              {p.failed_at && `, last ${timeAgo(p.failed_at)}`}. Check the destination's credentials and the database's logs.
            </p>
          )}
          {p?.error && <p className="text-sm text-err">{p.error}</p>}
          {p?.earliest && (
            <div className="flex flex-col gap-3 border-t border-line pt-4 sm:flex-row sm:items-end">
              <Field label="Restore to (your time)" className="flex-1">
                <Input
                  type="datetime-local"
                  value={target}
                  min={toLocalInput(new Date(p.earliest))}
                  max={toLocalInput(new Date())}
                  onChange={(e) => setTarget(e.target.value)}
                />
              </Field>
              <Button variant="secondary" busy={restore.isPending} disabled={!target} onClick={() => restore.mutate()}>
                Restore into a new database
              </Button>
            </div>
          )}
          <div>
            <Button variant="tertiary" className="-ml-3.5 h-8" busy={configure.isPending} onClick={() => configure.mutate(null)}>
              Turn off archiving
            </Button>
          </div>
        </div>
      )}
    </Card>
  );
}
