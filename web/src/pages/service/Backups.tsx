import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Link } from "react-router";
import { api, bytes, timeAgo, type Backup, type Destination, type Schedule, type ServiceDetail } from "../../api";
import { Button, Card, EmptyState, ErrorText, Field, Input, Modal, SectionTitle, Select, StatusDot, useToast } from "../../ui";

export default function Backups({ service }: { service: ServiceDetail }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const destinations = useQuery({ queryKey: ["destinations"], queryFn: () => api.get<Destination[]>("/backup-destinations") });
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
    mutationFn: () => api.post(`/services/${service.id}/backups`, { destination_id: destination }),
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
        Backups go to S3-compatible storage (AWS, Cloudflare R2, Backblaze B2, MinIO) or to a folder on this
        server.
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
            <ul className="divide-y divide-divider">
              {backups.data.map((b) => (
                <li key={b.id} className="flex flex-wrap items-center justify-between gap-3 px-6 py-4">
                  <div className="min-w-0">
                    <StatusDot status={b.status} />
                    <p className="mt-1 truncate font-mono text-sm">{b.object_key}</p>
                    <p className="text-sm text-ink/70">
                      {timeAgo(b.created_at)} · {b.trigger} · {name(b.destination_id)}
                      {b.size_bytes != null && ` · ${bytes(b.size_bytes)}`}
                    </p>
                    {b.error && <p className="text-sm text-error">{b.error}</p>}
                    {b.restore_status && (
                      <p className={`text-sm ${b.restore_status === "failed" ? "text-error" : ""}`}>
                        Restore {b.restore_status}
                        {b.restored_at && ` ${timeAgo(b.restored_at)}`}
                        {b.restore_error && `: ${b.restore_error}`}
                      </p>
                    )}
                  </div>
                  {b.status === "succeeded" && (
                    <Button variant="secondary" className="!py-2" onClick={() => setRestoring(b)}>
                      Restore
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-6 py-4 text-ink/70">No backups yet.</p>
          )}
        </Card>
      </div>
      <div className="grid content-start gap-8">
        <Card>
          <h3 className="mb-4 text-lg font-extrabold">Back up now</h3>
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
    current ?? { destination_id: destinations[0]?.id ?? "", cron: "0 3 * * *", keep: 7 },
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
      <h3 className="mb-1 text-lg font-extrabold">Schedule</h3>
      <p className="mb-4 text-sm text-ink/70">
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
