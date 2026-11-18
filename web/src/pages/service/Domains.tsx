import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, type ServiceDetail, type Settings } from "../../api";
import { DnsPanel } from "../../DnsSetup";
import { Button, Card, EmptyState, ErrorText, Input, SectionTitle, useToast } from "../../ui";

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
      toast("Domain added. Set its DNS record to connect it.");
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

  const tls = settings.data?.tls ?? false;
  // A CNAME onto the generated hostname only works when that hostname is real.
  const cnameTarget = settings.data && settings.data.base_domain !== "localhost" ? (service.generated_host ?? undefined) : undefined;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    add.mutate();
  };

  return (
    <div className="grid max-w-5xl gap-4">
      <SectionTitle>Domains</SectionTitle>
      <Card className="!p-0">
        <form onSubmit={submit} className="border-b border-line px-5 py-5 md:px-6">
          <label htmlFor="new-domain" className="mb-1.5 block text-sm font-medium">
            Add a domain you own
          </label>
          <div className="flex flex-col gap-2 sm:flex-row">
            <Input
              id="new-domain"
              className="sm:max-w-md"
              value={hostname}
              onChange={(e) => setHostname(e.target.value.trim().toLowerCase())}
              placeholder="app.example.com"
              required
            />
            <Button type="submit" busy={add.isPending}>
              Add domain
            </Button>
          </div>
          <div className="mt-2">
            <ErrorText error={add.error} />
          </div>
        </form>

        <ul className="divide-y divide-line">
          {service.generated_host && (
            <li className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-4 md:px-6">
              <a href={`${tls ? "https" : "http"}://${service.generated_host}`} target="_blank" rel="noreferrer" className="min-w-0 flex-1 break-all">
                {service.generated_host}
              </a>
              <span className="text-sm text-muted">Generated from the base domain</span>
            </li>
          )}
          {service.domains.map((d) => (
            <li key={d.id}>
              <DnsPanel
                host={d.hostname}
                cnameTarget={cnameTarget}
                tls={tls}
                actions={
                  <Button variant="tertiary" className="h-8" busy={remove.isPending && remove.variables === d.id} onClick={() => remove.mutate(d.id)}>
                    Remove
                  </Button>
                }
              />
            </li>
          ))}
        </ul>
      </Card>

      {service.domains.length === 0 && (
        <ol className="grid gap-x-8 gap-y-3 px-1 text-sm text-muted md:grid-cols-3">
          <li>
            <span className="font-semibold text-fg">1. Add it here.</span> The service redeploys to answer on the new name.
          </li>
          <li>
            <span className="font-semibold text-fg">2. Set one DNS record</span> at your domain provider. We show you exactly which.
          </li>
          <li>
            <span className="font-semibold text-fg">3. Wait for Connected.</span> justdeploy checks DNS for you{tls ? ", then HTTPS is set up on the first visit" : ""}.
          </li>
        </ol>
      )}
    </div>
  );
}
