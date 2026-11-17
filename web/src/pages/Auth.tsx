import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api } from "../api";
import { Logo } from "../Layout";
import { Button, ErrorText, Field, Input } from "../ui";

/** First-run setup (creates the single admin) and sign-in share one screen. */
export default function AuthPage({ mode }: { mode: "setup" | "login" }) {
  const queryClient = useQueryClient();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const setup = mode === "setup";

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.post(setup ? "/auth/setup" : "/auth/login", { email, password });
      await queryClient.invalidateQueries({ queryKey: ["auth-status"] });
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-screen flex-col items-center px-5 py-12 sm:justify-center">
      <Logo className="mb-10 h-7" />
      <form onSubmit={submit} className="w-full max-w-sm rounded-panel border border-line bg-surface p-7">
        <h1 className="title-wide text-2xl">{setup ? "Set up this server" : "Sign in"}</h1>
        <p className="mt-2 text-sm text-muted">
          {setup
            ? "Create the admin account. There is only one; you can reset its password from the server's command line."
            : "Use the admin account for this server."}
        </p>
        <div className="mt-7 space-y-4">
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
            {setup ? "Create admin account" : "Sign in"}
          </Button>
        </div>
      </form>
    </div>
  );
}
