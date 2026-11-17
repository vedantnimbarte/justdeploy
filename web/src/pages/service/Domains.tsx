import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, type ServiceDetail, type Settings } from "../../api";
import { Button, Card, EmptyState, ErrorText, Field, Input, SectionTitle, useToast } from "../../ui";

export default function Domains({ service }: { service: ServiceDetail }) {
  const toast = useToast();
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api.get<Settings>("/settings") });
  const [hostname, setHostname] = useState("");
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["service", service.id] });

  const add = useMutation({
    mutationFn: () => api.post(`/services/${service.id}/domains`, { hostname }),
    onSuccess: () => {
      setHostname("");
      toast("Domain added. Redeploying to route it.");
      refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.del(`/services/${service.id}/domains/${id}`),
    onSuccess: () => {
      toast("Domain removed");
      refresh();
    },
    onError: (e) => toast(e.message, true),
  });

  if (!service.public || !service.port) {
    return (
      <EmptyState title="This service is private">
        Make it public with a port under Settings, and it gets a hostname of its own. Then you can add
        custom domains here.
      </EmptyState>
    );
  }

  const scheme = settings.data?.tls ? "https" : "http";
  const submit = (e: FormEvent) => {
    e.preventDefault();
    add.mutate();
  };

  return (
    <div className="grid max-w-5xl gap-8">
      <div>
        <SectionTitle>Domains</SectionTitle>
        <Card className="!p-0">
          <ul className="divide-y divide-divider">
            {service.generated_host && (
              <li className="flex flex-wrap items-center justify-between gap-3 px-6 py-4">
                <a href={`${scheme}://${service.generated_host}`} target="_blank" rel="noreferrer" className="break-all">
                  {service.generated_host}
                </a>
                <span className="text-sm text-ink/70">Generated</span>
              </li>
            )}
            {service.domains.map((d) => (
              <li key={d.id} className="flex flex-wrap items-center justify-between gap-3 px-6 py-4">
                <a href={`${scheme}://${d.hostname}`} target="_blank" rel="noreferrer" className="break-all">
                  {d.hostname}
                </a>
                <Button variant="tertiary" className="!py-2" onClick={() => remove.mutate(d.id)}>
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        </Card>
      </div>
      <Card>
        <form onSubmit={submit} className="grid gap-5">
          <h3 className="text-lg font-extrabold">Add a custom domain</h3>
          <p>
            Point an <strong>A</strong> record for the domain at this server first.
            {settings.data?.tls
              ? " A Let's Encrypt certificate is issued on the first request, so port 80 must be reachable."
              : " TLS is off on this install, so the domain is served over plain HTTP."}
          </p>
          <div className="flex flex-col gap-3 md:flex-row md:items-end">
            <Field label="Hostname" className="flex-1">
              <Input value={hostname} onChange={(e) => setHostname(e.target.value.toLowerCase())} placeholder="app.example.com" required />
            </Field>
            <Button type="submit" busy={add.isPending}>
              Add domain
            </Button>
          </div>
          <ErrorText error={add.error} />
        </form>
      </Card>
    </div>
  );
}
