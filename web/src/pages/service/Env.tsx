import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import { api, type EnvVar, type ServiceDetail } from "../../api";
import { Button, Card, Checkbox, ErrorText, Input, Modal, SectionTitle, Textarea, useToast } from "../../ui";

/** Parses KEY=value lines, ignoring comments, blank lines and `export`. */
export function parseDotenv(text: string): EnvVar[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#") && line.includes("="))
    .map((line) => {
      const [rawKey, ...rest] = line.replace(/^export\s+/, "").split("=");
      let value = rest.join("=").trim();
      if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
        value = value.slice(1, -1);
      }
      return { key: (rawKey ?? "").trim(), value };
    });
}

export default function Env({ service }: { service: ServiceDetail }) {
  const shared = useQuery({
    queryKey: ["shared-env", service.project.id],
    queryFn: () => api.get<EnvVar[]>(`/projects/${service.project.id}/env`),
  });
  return (
    <EnvEditor
      url={`/services/${service.id}/env`}
      title="Environment variables"
      intro={
        <>
          Encrypted at rest, and available at build time too. Saving re-runs the current image with the new values; use{" "}
          <strong>Deploy latest</strong> to rebuild when a build step reads them.
          {service.kind !== "database" && service.port ? ` PORT is set to ${service.port} unless you set it here.` : ""} Point at
          another service with a reference: <code>{"${{db.URL}}"}</code> is a database's connection URL,{" "}
          <code>{"${{api.HOST}}"}</code> and <code>{"${{api.PORT}}"}</code> another service's address, and{" "}
          <code>{"${{api.NAME}}"}</code> any of its variables.
        </>
      }
      footer={
        shared.data && shared.data.length > 0 ? (
          <p className="mt-4 text-sm text-muted">
            Also gets the project's shared variables: {shared.data.map((v) => v.key).join(", ")}. A variable here with the same name
            wins.
          </p>
        ) : null
      }
    />
  );
}

/** A list of variables stored at `url`, edited as a whole: a service's, or a project's shared ones. */
export function EnvEditor({
  url,
  title,
  intro,
  footer,
}: {
  url: string;
  title: string;
  intro: ReactNode;
  footer?: ReactNode;
}) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const env = useQuery({
    queryKey: ["env", url],
    queryFn: () => api.get<EnvVar[]>(url),
  });
  const [rows, setRows] = useState<EnvVar[]>([]);
  const [show, setShow] = useState(false);
  const [importing, setImporting] = useState(false);
  const [pasted, setPasted] = useState("");

  useEffect(() => {
    if (env.data) setRows(env.data);
  }, [env.data]);

  const save = useMutation({
    mutationFn: () =>
      api.put<{ redeploying: boolean }>(
        url,
        rows.filter((r) => r.key.trim()),
      ),
    onSuccess: (result) => {
      toast(result.redeploying ? "Saved. Redeploying with the new values." : "Saved");
      queryClient.invalidateQueries({ queryKey: ["env", url] });
    },
  });

  const update = (i: number, field: keyof EnvVar, value: string) =>
    setRows((r) => r.map((row, j) => (j === i ? { ...row, [field]: value } : row)));
  const dirty = JSON.stringify(rows) !== JSON.stringify(env.data ?? []);

  return (
    <div className="max-w-5xl">
      <SectionTitle
        action={
          <Button variant="tertiary" onClick={() => setImporting(true)}>
            Paste .env
          </Button>
        }
      >
        {title}
      </SectionTitle>
      <p className="mb-6 max-w-3xl">{intro}</p>
      <Card>
        {env.isPending ? (
          <p>Loading…</p>
        ) : (
          <div className="space-y-3">
            {rows.length === 0 && <p className="text-muted">No variables yet.</p>}
            {rows.map((row, i) => (
              <div key={i} className="grid gap-2 md:grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto]">
                <Input
                  aria-label="Name"
                  className="font-mono text-sm"
                  value={row.key}
                  placeholder="NAME"
                  onChange={(e) => update(i, "key", e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, "_"))}
                />
                <Input
                  aria-label={`Value of ${row.key}`}
                  className="font-mono text-sm"
                  type={show ? "text" : "password"}
                  autoComplete="off"
                  value={row.value}
                  onChange={(e) => update(i, "value", e.target.value)}
                />
                <Button variant="tertiary" onClick={() => setRows((r) => r.filter((_, j) => j !== i))}>
                  Remove
                </Button>
              </div>
            ))}
            <div className="flex flex-wrap items-center justify-between gap-4 pt-3">
              <div className="flex gap-3">
                <Button variant="secondary" onClick={() => setRows((r) => [...r, { key: "", value: "" }])}>
                  Add variable
                </Button>
                <Button busy={save.isPending} disabled={!dirty} onClick={() => save.mutate()}>
                  Save
                </Button>
              </div>
              <Checkbox label="Show values" checked={show} onChange={setShow} />
            </div>
            <ErrorText error={save.error} />
          </div>
        )}
      </Card>
      {footer}
      <Modal open={importing} title="Paste a .env file" onClose={() => setImporting(false)} wide>
        <div className="space-y-4">
          <p>Variables with the same name are replaced; others are added.</p>
          <Textarea rows={10} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={"KEY=value\nOTHER=value"} />
          <div className="flex justify-end gap-3">
            <Button variant="secondary" onClick={() => setImporting(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => {
                const incoming = parseDotenv(pasted);
                setRows((current) => {
                  const map = new Map(current.map((r) => [r.key, r.value]));
                  incoming.forEach((r) => map.set(r.key, r.value));
                  return [...map.entries()].map(([key, value]) => ({ key, value }));
                });
                setPasted("");
                setImporting(false);
              }}
            >
              Add to the list
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
