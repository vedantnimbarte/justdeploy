import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { api, type GithubOverview } from "../api";
import { Band, Button, Card, ErrorText, Field, Input, SectionTitle, useToast } from "../ui";

/** Settings card: connect a GitHub App, then install it where your code lives. */
export function GitHubCard() {
  const queryClient = useQueryClient();
  const toast = useToast();
  const github = useQuery({ queryKey: ["github"], queryFn: () => api.get<GithubOverview>("/github") });
  const [org, setOrg] = useState("");

  // GitHub creates the app from a manifest the browser posts to it.
  const create = useMutation({
    mutationFn: () => api.post<{ action_url: string; manifest: unknown }>("/github/manifest", { org }),
    onSuccess: ({ action_url, manifest }) => {
      const form = document.createElement("form");
      form.method = "post";
      form.action = action_url;
      const field = document.createElement("input");
      field.type = "hidden";
      field.name = "manifest";
      field.value = JSON.stringify(manifest);
      form.append(field);
      document.body.append(form);
      form.submit();
    },
  });
  const forget = useMutation({
    mutationFn: () => api.del("/github/app"),
    onSuccess: () => {
      toast("Disconnected. Delete the app on GitHub too if you no longer need it.");
      queryClient.invalidateQueries({ queryKey: ["github"] });
    },
  });

  const app = github.data?.app;
  const installed = (github.data?.installations.length ?? 0) > 0;

  return (
    <Card>
      <SectionTitle>GitHub</SectionTitle>
      <p className="mb-5 max-w-[68ch] text-muted">
        A GitHub App of your own lets justdeploy clone private repositories without personal tokens, deploy on every push without
        setting up webhooks, and mark commits and pull requests with each deploy's result and preview link.
      </p>
      <ol className="grid gap-5">
        <li className="grid gap-3 md:grid-cols-[1.75rem_minmax(0,1fr)]">
          <Marker n={1} done={!!app} />
          <div>
            <p className="font-medium">{app ? "App created" : "Create the app on GitHub"}</p>
            {app ? (
              <p className="mt-1 text-sm text-muted">
                <a href={app.html_url} target="_blank" rel="noreferrer">
                  {app.slug}
                </a>{" "}
                on GitHub.{" "}
                <button className="underline decoration-line-strong underline-offset-[3px] hover:decoration-current" onClick={() => forget.mutate()}>
                  Disconnect
                </button>
              </p>
            ) : (
              <form onSubmit={(e: FormEvent) => (e.preventDefault(), create.mutate())} className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-end">
                <Field label="Organization" hint="Leave empty to create it on your personal account." className="sm:w-72">
                  <Input value={org} onChange={(e) => setOrg(e.target.value.trim())} placeholder="acme" />
                </Field>
                <Button type="submit" busy={create.isPending} className="sm:mb-[22px]">
                  Create GitHub App
                </Button>
              </form>
            )}
            <div className="mt-2">
              <ErrorText error={create.error} />
            </div>
          </div>
        </li>
        <li className="grid gap-3 md:grid-cols-[1.75rem_minmax(0,1fr)]">
          <Marker n={2} done={installed} />
          <div>
            <p className={app ? "font-medium" : "font-medium text-muted"}>Install it on the repositories to deploy</p>
            {app && (
              <div className="mt-1 text-sm text-muted">
                {installed ? (
                  <p>
                    Installed on {github.data?.installations.map((i) => i.account).join(", ")}.{" "}
                    <a href={app.install_url} target="_blank" rel="noreferrer">
                      Install on another account or change repositories
                    </a>
                  </p>
                ) : (
                  <p>
                    <a href={app.install_url}>Install {app.slug}</a> and choose the repositories. GitHub brings you back here.
                  </p>
                )}
                {github.data?.error && <p className="mt-1 text-err">GitHub couldn't be reached: {github.data.error}</p>}
              </div>
            )}
          </div>
        </li>
        <li className="grid gap-3 md:grid-cols-[1.75rem_minmax(0,1fr)]">
          <Marker n={3} done={false} />
          <div>
            <p className={installed ? "font-medium" : "font-medium text-muted"}>Pick a repository when you add a service</p>
            <p className="mt-1 text-sm text-muted">
              Existing services built from those repositories use the app too: no token or webhook needed.
            </p>
          </div>
        </li>
      </ol>
    </Card>
  );
}

function Marker({ n, done }: { n: number; done: boolean }) {
  return (
    <span
      className={
        done
          ? "flex size-7 items-center justify-center rounded-full bg-ok text-[13px] font-semibold text-white dark:text-page"
          : "flex size-7 items-center justify-center rounded-full border border-line-strong bg-surface text-[13px] font-semibold text-muted"
      }
      aria-label={done ? `Step ${n}, done` : `Step ${n}`}
    >
      {done ? "✓" : n}
    </span>
  );
}

/** GitHub sends the browser here after creating the app; trade the code for its keys. */
export function GitHubCreated() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const queryClient = useQueryClient();
  const sent = useRef(false);
  const finish = useMutation({
    mutationFn: () => api.post("/github/app", { code: params.get("code"), state: params.get("state") }),
    onSuccess: () => {
      toast("GitHub App created. Now install it on your repositories.");
      queryClient.invalidateQueries({ queryKey: ["github"] });
      navigate("/settings", { replace: true });
    },
  });
  useEffect(() => {
    if (!sent.current) {
      sent.current = true;
      finish.mutate();
    }
  }, [finish]);
  return (
    <Band className="pt-10">
      {finish.isError ? <ErrorText error={finish.error} /> : <p className="text-muted">Connecting the GitHub App…</p>}
    </Band>
  );
}

/** GitHub sends the browser here after an installation. */
export function GitHubInstalled() {
  const navigate = useNavigate();
  const toast = useToast();
  const queryClient = useQueryClient();
  useEffect(() => {
    toast("Installed. Pick its repositories when you add a service.");
    queryClient.invalidateQueries({ queryKey: ["github"] });
    navigate("/settings", { replace: true });
  }, [navigate, toast, queryClient]);
  return null;
}
