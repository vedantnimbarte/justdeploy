import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type FormEvent } from "react";
import { api, timeAgo, type Destination, type NotificationChannel, type Settings, type SmtpSettings } from "../api";
import { DnsPanel, useDnsCheck } from "../DnsSetup";
import { useMe } from "../me";
import { GitHubCard } from "./GitHub";
import { Band, Button, Card, Checkbox, ErrorText, Field, Input, PageHeader, SectionTitle, Select, useToast } from "../ui";

export default function SettingsPage() {
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api.get<Settings>("/settings") });
  const { me, can } = useMe();
  if (me && !can("admin")) {
    return (
      <>
        <PageHeader title="Settings" lead="Domains, TLS, GitHub, notifications and backups for this server." />
        <Band className="max-w-4xl">
          <Card>
            <p>Only admins change server settings. Your own password, two-factor sign-in and API tokens are under Account.</p>
          </Card>
        </Band>
      </>
    );
  }
  return (
    <>
      <PageHeader title="Settings" lead="Domains, TLS, GitHub, notifications and backups for this server." />
      <Band className="grid max-w-4xl gap-6">
        {settings.data && <DomainSettings settings={settings.data} />}
        <GitHubCard />
        <EmailServer />
        <Notifications />
        <Destinations />
        {settings.data && (
          <p className="text-sm text-muted">
            justdeploy {settings.data.version}. Webhooks reach this server at {settings.data.public_url}.
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
  // Check the domain being typed, once typing pauses, so its DNS can be set up before saving.
  const [typed, setTyped] = useState(domain);
  useEffect(() => {
    const t = setTimeout(() => setTyped(domain), 600);
    return () => clearTimeout(t);
  }, [domain]);
  const candidate = typed === "localhost" || typed.includes(".") ? typed : settings.base_domain;
  const checkable = candidate !== "localhost";
  const check = useDnsCheck(`*.${candidate}`, checkable);
  const changed = domain !== settings.base_domain;
  const unready = changed && domain !== "localhost" && check.data?.status !== "connected";
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
              Services get <code>service-project.{domain || "example.com"}</code>, which needs one wildcard DNS record. Use{" "}
              <code>localhost</code> on your own machine.
            </>
          }
        >
          <Input value={domain} onChange={(e) => setDomain(e.target.value.toLowerCase())} required />
        </Field>
        {checkable && (
          <div className="overflow-hidden rounded-panel border border-line">
            <DnsPanel key={candidate} host={`*.${candidate}`} tls={settings.tls} />
          </div>
        )}
        <div className="rounded-control bg-subtle p-4">
          {settings.tls ? (
            <p>
              <strong>TLS is on.</strong> Traefik gets Let's Encrypt certificates for every hostname, registered to {settings.acme_email}.
            </p>
          ) : (
            <p>
              <strong>TLS is off.</strong> Set <code>JD_ACME_EMAIL</code> and start with the TLS compose file to get Let's Encrypt
              certificates. See the README.
            </p>
          )}
        </div>
        <ErrorText error={save.error} />
        {unready && (
          <p className="text-sm text-warn" role="status">
            <code>*.{domain}</code> doesn't point here yet. Saving moves every public service onto it now, and they won't load until it
            does.
          </p>
        )}
        <div>
          <Button type="submit" busy={save.isPending}>
            {unready ? "Save anyway" : "Save"}
          </Button>
        </div>
      </form>
    </Card>
  );
}

const CHANNEL_KINDS = {
  slack: {
    label: "Slack",
    hint: "An incoming webhook URL: Slack app settings, Incoming Webhooks.",
    placeholder: "https://hooks.slack.com/services/…",
  },
  discord: {
    label: "Discord",
    hint: "Channel settings, Integrations, Webhooks, Copy Webhook URL.",
    placeholder: "https://discord.com/api/webhooks/…",
  },
  webhook: { label: "Any URL", hint: "Gets a JSON POST with event, text and url.", placeholder: "https://example.com/hooks/justdeploy" },
  email: {
    label: "Email",
    hint: "One or more addresses, separated by commas. Sent through the email server above.",
    placeholder: "ops@example.com, oncall@example.com",
  },
} as const;

const SECURITY = {
  starttls: { label: "STARTTLS (port 587)", port: 587 },
  tls: { label: "TLS (port 465)", port: 465 },
  none: { label: "None, for a relay on a trusted network", port: 25 },
} as const;

/** The SMTP server email notifications go through. */
function EmailServer() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const current = useQuery({ queryKey: ["smtp"], queryFn: () => api.get<SmtpSettings | null>("/settings/smtp") });
  const blank = {
    host: "",
    port: 587,
    security: "starttls" as SmtpSettings["security"],
    username: "",
    password: "",
    from: "",
    test_to: "",
  };
  const [form, setForm] = useState(blank);
  const [editing, setEditing] = useState(false);
  useEffect(() => {
    const c = current.data;
    if (c) setForm((f) => ({ ...f, host: c.host, port: c.port, security: c.security, username: c.username, from: c.from }));
  }, [current.data]);
  const set =
    <K extends keyof typeof form>(key: K) =>
    (value: (typeof form)[K]) =>
      setForm((f) => ({ ...f, [key]: value }));
  const save = useMutation({
    mutationFn: () => api.put("/settings/smtp", { ...form, password: form.password || null }),
    onSuccess: () => {
      toast("Saved; the test message was accepted");
      setEditing(false);
      setForm((f) => ({ ...f, password: "" }));
      queryClient.invalidateQueries({ queryKey: ["smtp"] });
    },
  });
  const remove = useMutation({
    mutationFn: () => api.del("/settings/smtp"),
    onSuccess: () => {
      setForm(blank);
      queryClient.invalidateQueries({ queryKey: ["smtp"] });
    },
    onError: (e) => toast(e.message, true),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate();
  };
  const c = current.data;
  const open = editing || c === null;

  return (
    <Card>
      <SectionTitle>Email</SectionTitle>
      {c && !editing ? (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="max-w-[68ch]">
            Sending as <strong>{c.from}</strong> through {c.host}:{c.port}
            {c.username ? `, signed in as ${c.username}` : ""}.
          </p>
          <div className="flex gap-1">
            <Button variant="secondary" onClick={() => setEditing(true)}>
              Change
            </Button>
            <Button variant="tertiary" busy={remove.isPending} onClick={() => remove.mutate()}>
              Remove
            </Button>
          </div>
        </div>
      ) : (
        open && (
          <form onSubmit={submit} className="grid gap-5">
            <p className="max-w-[68ch] text-muted">
              An SMTP server for email notifications: Amazon SES, Postmark, Resend, Mailgun, your own. It's saved once a test message to the
              address below is accepted.
            </p>
            <div className="grid gap-5 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
              <Field label="Server">
                <Input value={form.host} onChange={(e) => set("host")(e.target.value)} placeholder="smtp.example.com" required />
              </Field>
              <Field label="Port">
                <Input type="number" min={1} max={65535} value={form.port} onChange={(e) => set("port")(Number(e.target.value))} required />
              </Field>
            </div>
            <Field label="Encryption">
              <Select
                value={form.security}
                onChange={(e) => {
                  const security = e.target.value as SmtpSettings["security"];
                  setForm((f) => ({ ...f, security, port: SECURITY[security].port }));
                }}
              >
                {Object.entries(SECURITY).map(([id, s]) => (
                  <option key={id} value={id}>
                    {s.label}
                  </option>
                ))}
              </Select>
            </Field>
            <div className="grid gap-5 md:grid-cols-2">
              <Field label="Username" hint="Leave empty if the server doesn't ask for one.">
                <Input autoComplete="off" value={form.username} onChange={(e) => set("username")(e.target.value)} />
              </Field>
              <Field label="Password" hint={c?.has_password ? "Leave empty to keep the saved one. Stored encrypted." : "Stored encrypted."}>
                <Input
                  type="password"
                  autoComplete="new-password"
                  value={form.password}
                  onChange={(e) => set("password")(e.target.value)}
                />
              </Field>
            </div>
            <div className="grid gap-5 md:grid-cols-2">
              <Field label="Send as">
                <Input
                  value={form.from}
                  onChange={(e) => set("from")(e.target.value)}
                  placeholder="justdeploy <alerts@example.com>"
                  required
                />
              </Field>
              <Field label="Send a test to">
                <Input
                  type="email"
                  value={form.test_to}
                  onChange={(e) => set("test_to")(e.target.value)}
                  placeholder="you@example.com"
                  required
                />
              </Field>
            </div>
            <ErrorText error={save.error} />
            <div className="flex gap-2">
              <Button type="submit" busy={save.isPending}>
                Send a test and save
              </Button>
              {editing && (
                <Button variant="secondary" onClick={() => setEditing(false)}>
                  Cancel
                </Button>
              )}
            </div>
          </form>
        )
      )}
    </Card>
  );
}

function Notifications() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const channels = useQuery({ queryKey: ["channels"], queryFn: () => api.get<NotificationChannel[]>("/notification-channels") });
  const empty = { name: "", kind: "slack" as NotificationChannel["kind"], url: "", notify_success: false };
  const [form, setForm] = useState(empty);
  const create = useMutation({
    mutationFn: () => api.post("/notification-channels", form),
    onSuccess: () => {
      toast("Channel added; a test message was sent");
      setForm(empty);
      queryClient.invalidateQueries({ queryKey: ["channels"] });
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.del(`/notification-channels/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["channels"] }),
    onError: (e) => toast(e.message, true),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate();
  };
  const kind = CHANNEL_KINDS[form.kind];

  return (
    <Card>
      <SectionTitle>Notifications</SectionTitle>
      {channels.data?.length ? (
        <ul className="mb-6 divide-y divide-line rounded-panel border border-line">
          {channels.data.map((c) => (
            <li key={c.id} className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
              <div className="min-w-0">
                <p className="font-semibold">{c.name}</p>
                <p className="truncate text-sm text-muted">
                  {CHANNEL_KINDS[c.kind].label} at {c.target}, {c.notify_success ? "failures and successful deploys" : "failures only"}
                </p>
              </div>
              <Button variant="tertiary" className="h-8" onClick={() => remove.mutate(c.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mb-6 max-w-[68ch]">
          Get a message when a deploy, scheduled run or backup fails, a service crashes or stops answering, or a domain stops pointing here.
          None set up yet.
        </p>
      )}
      <form onSubmit={submit} className="grid gap-5 border-t border-line pt-6">
        <h3 className="text-lg font-semibold">Add a channel</h3>
        <div className="grid gap-5 md:grid-cols-2">
          <Field label="Name">
            <Input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="ops" required />
          </Field>
          <Field label="Send to">
            <Select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value as NotificationChannel["kind"] })}>
              {Object.entries(CHANNEL_KINDS).map(([id, k]) => (
                <option key={id} value={id}>
                  {k.label}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <Field label={form.kind === "email" ? "Addresses" : "Webhook URL"} hint={`${kind.hint} Stored encrypted.`}>
          <Input
            type={form.kind === "email" ? "text" : "url"}
            autoComplete="off"
            value={form.url}
            onChange={(e) => setForm({ ...form, url: e.target.value })}
            placeholder={kind.placeholder}
            required
          />
        </Field>
        <Checkbox
          label="Also tell me when deploys succeed"
          checked={form.notify_success}
          onChange={(v) => setForm({ ...form, notify_success: v })}
        />
        <ErrorText error={create.error} />
        <div>
          <Button type="submit" busy={create.isPending}>
            Send a test and add
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
        <ul className="mb-6 divide-y divide-line rounded-panel border border-line">
          {destinations.data.map((d) => (
            <li key={d.id} className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
              <div className="min-w-0">
                <p className="font-semibold">{d.name}</p>
                <p className="truncate text-sm text-muted">
                  {d.summary}, added {timeAgo(d.created_at)}
                </p>
              </div>
              <Button variant="tertiary" className="h-8" onClick={() => remove.mutate(d.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mb-6">None yet. Add one, then schedule backups on any database's Backups tab.</p>
      )}
      <form onSubmit={submit} className="grid gap-5 border-t border-line pt-6">
        <h3 className="text-lg font-semibold">Add a destination</h3>
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
