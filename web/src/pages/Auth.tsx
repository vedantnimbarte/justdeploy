import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { api, ApiError, ROLE_HINT, ROLE_LABEL, type Invite } from "../api";
import { Logo } from "../Layout";
import { Button, ErrorText, Field, Input } from "../ui";

function Screen({
  title,
  lead,
  children,
  onSubmit,
}: {
  title: string;
  lead: ReactNode;
  children: ReactNode;
  onSubmit: (e: FormEvent) => void;
}) {
  return (
    <div className="flex min-h-screen flex-col items-center px-5 py-12 sm:justify-center">
      <Logo className="mb-10 h-7" />
      <form onSubmit={onSubmit} className="w-full max-w-sm rounded-panel border border-line bg-surface p-7">
        <h1 className="title-wide text-2xl">{title}</h1>
        <p className="mt-2 text-sm text-muted">{lead}</p>
        <div className="mt-7 space-y-4">{children}</div>
      </form>
    </div>
  );
}

/** First-run setup (creates the owner) and sign-in, which asks for a code when the account has one. */
export default function AuthPage({ mode }: { mode: "setup" | "login" }) {
  const queryClient = useQueryClient();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(false);
  const [needsCode, setNeedsCode] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const setup = mode === "setup";

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.post(setup ? "/auth/setup" : "/auth/login", { email, password, code: needsCode ? code : undefined });
      await queryClient.invalidateQueries({ queryKey: ["auth-status"] });
    } catch (err) {
      // 401 here means the password was right and the account wants its code.
      if (err instanceof ApiError && err.status === 401 && !needsCode) setNeedsCode(true);
      else setError(err);
    } finally {
      setBusy(false);
    }
  };

  if (needsCode) {
    return (
      <Screen
        title="Enter your code"
        lead={
          recovery
            ? "Enter one of the recovery codes you saved when you turned on two-factor sign-in. Each works once."
            : "Open your authenticator app and enter the 6-digit code for justdeploy."
        }
        onSubmit={submit}
      >
        <Field label={recovery ? "Recovery code" : "Code"}>
          <Input
            key={recovery ? "recovery" : "code"}
            inputMode={recovery ? "text" : "numeric"}
            autoComplete={recovery ? "off" : "one-time-code"}
            autoCapitalize="off"
            spellCheck={false}
            placeholder={recovery ? "abcde-12345" : undefined}
            autoFocus
            required
            className="font-mono text-lg tracking-[0.3em]"
            value={code}
            onChange={(e) => setCode(e.target.value)}
          />
        </Field>
        <button type="button" className="text-left text-sm text-muted hover:text-fg" onClick={() => (setRecovery(!recovery), setCode(""))}>
          {recovery ? "Use the authenticator app instead" : "Lost your phone? Use a recovery code"}
        </button>
        <ErrorText error={error} />
        <Button type="submit" busy={busy} className="w-full">
          Sign in
        </Button>
        <button
          type="button"
          className="w-full text-sm text-muted hover:text-fg"
          onClick={() => (setNeedsCode(false), setCode(""), setError(null))}
        >
          Use a different account
        </button>
      </Screen>
    );
  }

  return (
    <Screen
      title={setup ? "Set up this server" : "Sign in"}
      lead={
        setup
          ? "Create the owner account. You can invite your team afterwards, and reset this password from the server's command line."
          : "Use your justdeploy account. No account yet? Ask an owner for an invite link."
      }
      onSubmit={submit}
    >
      <Field label="Email">
        <Input type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
      </Field>
      <Field label="Password" hint={setup ? "At least 10 characters." : undefined}>
        <Input
          type="password"
          autoComplete={setup ? "new-password" : "current-password"}
          required
          minLength={setup ? 10 : undefined}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </Field>
      <ErrorText error={error} />
      <Button type="submit" busy={busy} className="w-full">
        {setup ? "Create owner account" : "Sign in"}
      </Button>
    </Screen>
  );
}

/** Where an invite link lands: set a password and you're in. */
export function InvitePage() {
  const { token = "" } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const invite = useQuery({ queryKey: ["invite", token], queryFn: () => api.get<Invite>(`/auth/invites/${token}`), retry: false });
  const [password, setPassword] = useState("");
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.post(`/auth/invites/${token}`, { password });
      await queryClient.invalidateQueries();
      navigate("/projects", { replace: true });
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  if (invite.isPending) return null;
  if (invite.isError) {
    return (
      <Screen title="This invite doesn't work" lead={invite.error.message} onSubmit={(e) => e.preventDefault()}>
        <Button className="w-full" onClick={() => navigate("/", { replace: true })}>
          Go to sign in
        </Button>
      </Screen>
    );
  }
  return (
    <Screen
      title="Join justdeploy"
      lead={
        <>
          You're invited as <strong className="text-fg">{ROLE_LABEL[invite.data.role].toLowerCase()}</strong>. {ROLE_HINT[invite.data.role]}
        </>
      }
      onSubmit={submit}
    >
      <Field label="Email">
        <Input type="email" value={invite.data.email} disabled />
      </Field>
      <Field label="Choose a password" hint="At least 10 characters.">
        <Input
          type="password"
          autoComplete="new-password"
          minLength={10}
          required
          autoFocus
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </Field>
      <ErrorText error={error} />
      <Button type="submit" busy={busy} className="w-full">
        Create account
      </Button>
    </Screen>
  );
}
