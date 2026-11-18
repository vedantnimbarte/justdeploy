import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { useParams } from "react-router";
import { api, type StatusPageData } from "../api";
import { cx } from "../ui";
import { UptimeRow } from "./project/Uptime";

const OVERALL = {
  operational: { dot: "bg-ok", text: "All systems operational" },
  degraded: { dot: "bg-warn", text: "Some systems are down" },
  down: { dot: "bg-err", text: "All systems are down" },
};

/** The public status page of a project, at /status/<project> or its own hostname. No sign-in. */
export default function StatusPage({ name: fromHost }: { name?: string }) {
  const params = useParams();
  const name = fromHost ?? params.name ?? "";
  const page = useQuery({
    queryKey: ["status-page", name],
    queryFn: () => api.get<StatusPageData>(`/status/${encodeURIComponent(name)}`),
    refetchInterval: 60_000,
  });
  useEffect(() => {
    document.title = page.data ? `${page.data.title} status` : "Status";
  }, [page.data]);

  if (page.isPending) return null;
  if (page.isError) {
    return (
      <main className="mx-auto max-w-3xl px-5 py-20">
        <h1 className="text-2xl font-semibold">No status page here</h1>
        <p className="mt-2 text-muted">This page isn't published, or the address is wrong.</p>
      </main>
    );
  }
  const p = page.data;
  const overall = OVERALL[p.overall];
  return (
    <main className="mx-auto max-w-3xl px-5 py-12 md:py-20">
      <h1 className="text-3xl font-semibold tracking-tight">{p.title}</h1>
      <p className="mt-6 flex items-center gap-3 rounded-panel border border-line bg-surface px-5 py-4 text-lg font-medium">
        <span className={cx("size-3 rounded-full", overall.dot)} aria-hidden />
        {overall.text}
      </p>
      <ul className="mt-8 divide-y divide-line rounded-panel border border-line bg-surface">
        {p.services.map((s) => (
          <li key={s.id}>
            <UptimeRow s={s} showUrl={false} />
          </li>
        ))}
      </ul>
      <p className="mt-6 text-sm text-muted">Checked every minute. Refreshes by itself.</p>
    </main>
  );
}
