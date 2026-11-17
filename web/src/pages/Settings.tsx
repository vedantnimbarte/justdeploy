import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type FormEvent } from "react";
import { api, timeAgo, type Destination, type Settings } from "../api";
import { Band, Button, Card, ErrorText, Field, Input, PageHeader, SectionTitle, Select, useToast } from "../ui";

export default function SettingsPage() {
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api.get<Settings>("/settings") });
  return (
    <>
      <PageHeader title="Settings" lead="Domains, TLS, backups and your account." />
      <Band tone="soft" className="grid max-w-4xl gap-8">
        {settings.data && <DomainSettings settings={settings.data} />}
        <Destinations />
        <PasswordForm />
        {settings.data && (
          <p className="text-sm text-ink/70">
            justdeploy {settings.data.version} · webhooks reach this server at {settings.data.public_url}
          </p>
        )}
      </Band>
    </>
  );
}

function DomainSettings({ settings }: { settings: Settings }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const [domain, setDomain] = useState(settings.base_domain);
  useEffect(() => setDomain(settings.base_domain), [settings.base_domain]);
  const save = useMutation({
    mutationFn: () => api.put("/settings", { base_domain: domain }),
    onSuccess: () => {
      toast("Saved. Public services are redeploying onto the new hostnames.");
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate();
  };
  return (
    <Card>
      <SectionTitle>Domain and TLS</SectionTitle>
      <form onSubmit={submit} className="grid gap-5">
        <Field
          label="Base domain"
          hint={
            <>
              Services get <code>service-project.{domain || "example.com"}</code>. Point a wildcard DNS record{" "}
              <code>*.{domain || "example.com"}</code> at this server. Use <code>localhost</code> on your own machine.
            </>
          }
        >
          <Input value={domain} onChange={(e) => setDomain(e.target.value.toLowerCase())} required />
        </Field>
        <div className="rounded-sm bg-canvas-soft p-4">
          {settings.tls ? (
            <p>
              <strong>TLS is on.</strong> Traefik gets Let's Encrypt certificates for every hostname, registered to{" "}
              {settings.acme_email}.
            </p>
          ) : (
            <p>
              <strong>TLS is off.</strong> Set <code>JD_ACME_EMAIL</code> and start with the TLS compose file to get
              Let's Encrypt certificates. See the README.
            </p>
          )}
        </div>
        <ErrorText error={save.error} />
        <div>
          <Button type="submit" busy={save.isPending}>
            Save
          </Button>
        </div>
      </form>
    </Card>
  );
}

function Destinations() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const destinations = useQuery({ queryKey: ["destinations"], queryFn: () => api.get<Destination[]>("/backup-destinations") });
  const empty = {
    name: "",
    kind: "s3" as "s3" | "local",
    endpoint: "",
    region: "us-east-1",
    bucket: "",
    access_key_id: "",
    secret_access_key: "",
    prefix: "justdeploy",
    path: "",
  };
  const [form, setForm] = useState(empty);
  const set = (key: keyof typeof form) => (value: string) => setForm((f) => ({ ...f, [key]: value }));

  const create = useMutation({
    mutationFn: () =>
      api.post(
        "/backup-destinations",
        form.kind === "local"
          ? { name: form.name, kind: "local", path: form.path }
          : {
              name: form.name,
              kind: "s3",
              endpoint: form.endpoint,
              region: form.region,
              bucket: form.bucket,
              access_key_id: form.access_key_id,
              secret_access_key: form.secret_access_key,
              prefix: form.prefix,
            },
      ),
    onSuccess: () => {
      toast("Destination added; a test write succeeded");
      setForm(empty);
      queryClient.invalidateQueries({ queryKey: ["destinations"] });
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.del(`/backup-destinations/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["destinations"] }),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate();
  };

  return (
    <Card>
      <SectionTitle>Backup destinations</SectionTitle>
      {destinations.data?.length ? (
        <ul className="mb-6 divide-y divide-divider rounded-md border border-hairline">
          {destinations.data.map((d) => (
            <li key={d.id} className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
              <div className="min-w-0">
                <p className="font-semibold">{d.name}</p>
                <p className="truncate text-sm text-ink/70">
                  {d.summary} · added {timeAgo(d.created_at)}
                </p>
              </div>
              <Button variant="tertiary" className="!py-2" onClick={() => remove.mutate(d.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mb-6">None yet. Add one, then schedule backups on any database's Backups tab.</p>
      )}
      <form onSubmit={submit} className="grid gap-5 border-t border-divider pt-6">
        <h3 className="text-lg font-extrabold">Add a destination</h3>
        <div className="grid gap-5 md:grid-cols-2">
          <Field label="Name">
            <Input value={form.name} onChange={(e) => set("name")(e.target.value)} placeholder="offsite" required />
          </Field>
          <Field label="Type">
            <Select value={form.kind} onChange={(e) => set("kind")(e.target.value)}>
              <option value="s3">S3-compatible storage</option>
              <option value="local">Folder on this server</option>
            </Select>
          </Field>
        </div>
        {form.kind === "s3" ? (
          <>
            <div className="grid gap-5 md:grid-cols-2">
              <Field label="Endpoint" hint="Empty for AWS. For R2, B2 or MinIO, their S3 endpoint URL.">
                <Input value={form.endpoint} onChange={(e) => set("endpoint")(e.target.value)} placeholder="https://…" />
              </Field>
              <Field label="Region">
                <Input value={form.region} onChange={(e) => set("region")(e.target.value)} />
              </Field>
              <Field label="Bucket">
                <Input value={form.bucket} onChange={(e) => set("bucket")(e.target.value)} required />
              </Field>
              <Field label="Key prefix">
                <Input value={form.prefix} onChange={(e) => set("prefix")(e.target.value)} />
              </Field>
              <Field label="Access key ID">
                <Input value={form.access_key_id} onChange={(e) => set("access_key_id")(e.target.value)} autoComplete="off" required />
              </Field>
              <Field label="Secret access key" hint="Stored encrypted.">
                <Input
                  type="password"
                  value={form.secret_access_key}
                  onChange={(e) => set("secret_access_key")(e.target.value)}
                  autoComplete="off"
                  required
                />
              </Field>
            </div>
          </>
        ) : (
          <Field label="Folder" hint="Created under the backups volume (JD_BACKUP_DIR). Copy it off the server too.">
            <Input value={form.path} onChange={(e) => set("path")(e.target.value)} placeholder="nightly" required />
          </Field>
        )}
        <ErrorText error={create.error} />
        <div>
          <Button type="submit" busy={create.isPending}>
            Test and add
          </Button>
        </div>
      </form>
    </Card>
  );
}

function PasswordForm() {
  const toast = useToast();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const change = useMutation({
    mutationFn: () => api.put("/auth/password", { current, new: next }),
    onSuccess: () => {
      toast("Password changed");
      setCurrent("");
      setNext("");
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    change.mutate();
  };
  return (
    <Card>
      <SectionTitle>Admin password</SectionTitle>
      <form onSubmit={submit} className="grid gap-5 md:grid-cols-2">
        <Field label="Current password">
          <Input type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
        </Field>
        <Field label="New password" hint="At least 10 characters.">
          <Input type="password" autoComplete="new-password" minLength={10} value={next} onChange={(e) => setNext(e.target.value)} required />
        </Field>
        <div className="md:col-span-2">
          <ErrorText error={change.error} />
          <Button type="submit" busy={change.isPending} className="mt-2">
            Change password
          </Button>
        </div>
      </form>
    </Card>
  );
}
