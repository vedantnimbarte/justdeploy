import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api } from "../api";
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
    <div className="flex min-h-screen flex-col bg-canvas-warm">
      <header className="bg-canvas px-4 md:px-10">
        <div className="mx-auto flex h-16 max-w-[1600px] items-center md:h-[72px]">
          <span className="text-2xl font-extrabold text-brand">justdeploy</span>
        </div>
      </header>
      <main className="flex flex-1 items-start justify-center px-4 py-12 md:items-center">
        <form onSubmit={submit} className="w-full max-w-md rounded-md border border-hairline bg-canvas p-6 md:p-8">
          <h1 className="font-serif text-[32px] font-normal">{setup ? "Set up justdeploy" : "Sign in"}</h1>
          <div className="mt-[18px] mb-6 h-1 w-10 bg-gold" aria-hidden />
          {setup && (
            <p className="mb-6">
              Create the admin account for this server. There is only one; you can reset its password
              from the command line if you lose it.
            </p>
          )}
          <div className="space-y-5">
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
      </main>
    </div>
  );
}
