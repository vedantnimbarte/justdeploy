import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, ROLE_HINT, ROLE_LABEL, timeAgo, type ApiToken } from "../api";
import { useMe } from "../me";
import { Band, Button, Card, CodeLine, ErrorText, Field, Input, PageHeader, SectionTitle, useToast } from "../ui";

export default function AccountPage() {
  const { me } = useMe();
  return (
    <>
      <PageHeader
        title="Account"
        lead={me ? `${me.email}, ${ROLE_LABEL[me.role].toLowerCase()}. ${ROLE_HINT[me.role]}` : undefined}
      />
      <Band className="grid max-w-4xl gap-6">
        <TwoFactor />
        <Tokens />
        <PasswordForm />
      </Band>
    </>
  );
}

type Setup = { secret: string; otpauth_url: string; qr_svg: string };

function TwoFactor() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const { me } = useMe();
  const [setup, setSetup] = useState<Setup | null>(null);
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["me"] });

  const start = useMutation({ mutationFn: () => api.post<Setup>("/auth/two-factor"), onSuccess: setSetup });
  const confirm = useMutation({
    mutationFn: () => api.post("/auth/two-factor/confirm", { code }),
    onSuccess: () => {
      toast("Two-factor sign-in is on");
      setSetup(null);
      setCode("");
      refresh();
    },
  });
  const disable = useMutation({
    mutationFn: () => api.del("/auth/two-factor", { password }),
    onSuccess: () => {
      toast("Two-factor sign-in is off");
      setPassword("");
      refresh();
    },
  });

  return (
    <Card>
      <SectionTitle>Two-factor sign-in</SectionTitle>
      {me?.two_factor ? (
        <form
          onSubmit={(e: FormEvent) => (e.preventDefault(), disable.mutate())}
          className="grid gap-4"
        >
          <p>
            <strong>On.</strong> Signing in asks for a code from your authenticator app. If you lose your phone, an owner can turn it
            off for you under Team.
          </p>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
            <Field label="Password, to turn it off" className="sm:w-72">
              <Input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} required />
            </Field>
            <Button type="submit" variant="danger" busy={disable.isPending}>
              Turn off
            </Button>
          </div>
          <ErrorText error={disable.error} />
        </form>
      ) : setup ? (
        <form onSubmit={(e: FormEvent) => (e.preventDefault(), confirm.mutate())} className="grid gap-6 md:grid-cols-[auto_minmax(0,1fr)]">
          <div
            className="size-[200px] overflow-hidden rounded-panel border border-line bg-white [&>svg]:size-full"
            role="img"
            aria-label="QR code to scan with your authenticator app"
            // The server renders the SVG from the setup link; nothing user-supplied is in it.
            dangerouslySetInnerHTML={{ __html: setup.qr_svg }}
          />
          <ol className="grid content-start gap-4">
            <li>
              <p className="font-medium">1. Scan the code with your authenticator app</p>
              <p className="mt-1 text-sm text-muted">1Password, Google Authenticator, Authy or any TOTP app. Can't scan? Enter this key:</p>
              <div className="mt-2 max-w-md">
                <CodeLine value={setup.secret} />
              </div>
            </li>
            <li>
              <p className="font-medium">2. Enter the 6-digit code it shows</p>
              <div className="mt-2 flex flex-col gap-2 sm:flex-row">
                <Input
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  className="font-mono tracking-[0.3em] sm:w-40"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  required
                  aria-label="Code"
                />
                <Button type="submit" busy={confirm.isPending}>
                  Turn on
                </Button>
                <Button variant="tertiary" onClick={() => setSetup(null)}>
                  Cancel
                </Button>
              </div>
              <div className="mt-2">
                <ErrorText error={confirm.error} />
              </div>
            </li>
          </ol>
        </form>
      ) : (
        <div className="flex flex-wrap items-center justify-between gap-4">
          <p className="max-w-[60ch]">Off. With it on, signing in also asks for a code from an app on your phone, so a leaked password isn't enough.</p>
          <Button busy={start.isPending} onClick={() => start.mutate()}>
            Set up
          </Button>
          <ErrorText error={start.error} />
        </div>
      )}
    </Card>
  );
}

function Tokens() {
  const toast = useToast();
  const queryClient = useQueryClient();
  const tokens = useQuery({ queryKey: ["tokens"], queryFn: () => api.get<ApiToken[]>("/auth/tokens") });
  const [name, setName] = useState("");
  const [created, setCreated] = useState<{ name: string; value: string } | null>(null);
  const create = useMutation({
    mutationFn: () => api.post<{ token: ApiToken; value: string }>("/auth/tokens", { name }),
    onSuccess: (r) => {
      setCreated({ name: r.token.name, value: r.value });
      setName("");
      queryClient.invalidateQueries({ queryKey: ["tokens"] });
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.del(`/auth/tokens/${id}`),
    onSuccess: () => {
      toast("Token deleted; anything using it stops working");
      queryClient.invalidateQueries({ queryKey: ["tokens"] });
    },
    onError: (e) => toast(e.message, true),
  });

  return (
    <Card>
      <SectionTitle>API tokens</SectionTitle>
      <p className="mb-5 max-w-[68ch] text-muted">
        For scripts and CI. Send it as <code>Authorization: Bearer jd_…</code>. A token can do whatever you can, so keep one per place it's
        used and delete it when that place goes away.
      </p>
      {created && (
        <div className="mb-5 rounded-panel border border-ok/40 bg-ok/5 p-4">
          <p className="mb-2 text-sm font-medium">
            {created.name} is ready. Copy it now: it won't be shown again.
          </p>
          <CodeLine value={created.value} />
        </div>
      )}
      {tokens.data && tokens.data.length > 0 && (
        <ul className="mb-5 divide-y divide-line rounded-panel border border-line">
          {tokens.data.map((t) => (
            <li key={t.id} className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
              <div className="min-w-0">
                <p className="font-semibold">{t.name}</p>
                <p className="text-sm text-muted">
                  Created {timeAgo(t.created_at)}, {t.last_used_at ? `last used ${timeAgo(t.last_used_at)}` : "never used"}
                </p>
              </div>
              <Button variant="tertiary" className="h-8" onClick={() => remove.mutate(t.id)}>
                Delete
              </Button>
            </li>
          ))}
        </ul>
      )}
      <form onSubmit={(e: FormEvent) => (e.preventDefault(), create.mutate())} className="flex flex-col gap-2 sm:flex-row">
        <Input aria-label="Token name" placeholder="github-actions" value={name} onChange={(e) => setName(e.target.value)} required className="sm:max-w-xs" />
        <Button type="submit" busy={create.isPending}>
          Create token
        </Button>
      </form>
      <div className="mt-2">
        <ErrorText error={create.error} />
      </div>
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
  return (
    <Card>
      <SectionTitle>Password</SectionTitle>
      <form onSubmit={(e: FormEvent) => (e.preventDefault(), change.mutate())} className="grid gap-5 md:grid-cols-2">
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
