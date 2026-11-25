import { keepPreviousData, useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Area, AreaChart, CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from "recharts";
import { api, timeAgo, type ObservabilityOverview } from "../../api";
import { Card, cx, EmptyState, ErrorText } from "../../ui";
import { Chart } from "../service/Metrics";
import { query, useFilters, type Scope } from "./Observability";

const GRID = "var(--jd-line)";
const axis = { stroke: "var(--jd-muted)", fontSize: 12, tickLine: false, axisLine: false };

export default function Overview({ scope }: { scope: Scope }) {
  const { set } = useFilters();
  const overview = useQuery({
    queryKey: ["observability", scope.project, scope.service, scope.hours],
    queryFn: () =>
      api.get<ObservabilityOverview>(`/projects/${scope.project}/observability${query({ service: scope.service, hours: scope.hours })}`),
    refetchInterval: 30_000,
    placeholderData: keepPreviousData,
  });
  if (overview.isPending) return <p className="py-6 text-muted">Loading…</p>;
  if (overview.isError) return <ErrorText error={overview.error} />;
  const { buckets, failing, errors, step } = overview.data;
  const long = scope.hours > 24;
  const tick = (iso: string) =>
    long ? new Date(iso).toLocaleDateString([], { month: "short", day: "numeric" }) : new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const label = (iso: unknown) => new Date(String(iso)).toLocaleString();
  const sum = (key: "requests" | "errors" | "error_logs") => buckets.reduce((n, b) => n + b[key], 0);
  const requests = sum("requests");
  const failedCount = sum("errors");
  const errorLines = sum("error_logs");
  const p95 = buckets.map((b) => b.p95_ms).filter((v): v is number => v != null);
  const per = step === 1 ? "minute" : `${step} minutes`;

  // Opening a route or message shows its requests or lines over the same range.
  const range = scope.hours === 1 ? null : scope.hours === 24 ? "24h" : scope.hours === 24 * 7 ? "7d" : "30d";

  return (
    <div className="grid gap-6">
      <dl className="grid grid-cols-2 gap-x-6 gap-y-4 sm:flex sm:flex-wrap sm:gap-x-12">
        <Figure label="Requests" value={requests.toLocaleString()} />
        <Figure
          label="Server errors"
          value={failedCount.toLocaleString()}
          note={requests ? `${((failedCount / requests) * 100).toFixed(1)}% of requests` : undefined}
          bad={failedCount > 0}
        />
        <Figure label="Slowest p95" value={p95.length ? `${Math.round(Math.max(...p95))} ms` : "–"} />
        <Figure label="Error lines" value={errorLines.toLocaleString()} bad={errorLines > 0} />
      </dl>

      <div className="grid gap-6 lg:grid-cols-3">
        <div className="lg:col-span-2">
          <Chart title="Requests" latest={`per ${per}`}>
            <AreaChart data={buckets}>
              <CartesianGrid stroke={GRID} vertical={false} />
              <XAxis dataKey="t" tickFormatter={tick} {...axis} minTickGap={48} />
              <YAxis {...axis} width={40} allowDecimals={false} />
              <Tooltip labelFormatter={label} formatter={(v, name) => [v, name === "errors" ? "Server errors" : "Requests"]} />
              <Area type="monotone" dataKey="requests" stroke="var(--jd-muted)" fill="var(--jd-subtle)" strokeWidth={1.5} isAnimationActive={false} />
              <Area type="monotone" dataKey="errors" stroke="var(--jd-err)" fill="var(--jd-err)" fillOpacity={0.18} strokeWidth={1.5} isAnimationActive={false} />
            </AreaChart>
          </Chart>
        </div>
        <Chart title="Latency" latest="p50 and p95">
          <LineChart data={buckets}>
            <CartesianGrid stroke={GRID} vertical={false} />
            <XAxis dataKey="t" tickFormatter={tick} {...axis} minTickGap={48} />
            <YAxis tickFormatter={(v) => `${v} ms`} {...axis} width={60} />
            <Tooltip labelFormatter={label} formatter={(v, name) => [`${Math.round(Number(v))} ms`, name === "p95_ms" ? "p95" : "p50"]} />
            <Line type="monotone" dataKey="p50_ms" stroke="var(--jd-faint)" dot={{ r: 2, strokeWidth: 0, fill: "var(--jd-faint)" }} strokeWidth={1.5} connectNulls isAnimationActive={false} />
            <Line type="monotone" dataKey="p95_ms" stroke="var(--jd-fg)" dot={{ r: 2, strokeWidth: 0, fill: "var(--jd-fg)" }} strokeWidth={1.5} connectNulls isAnimationActive={false} />
          </LineChart>
        </Chart>
      </div>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <h3 className="mb-1 text-lg font-semibold">Failing routes</h3>
          <p className="mb-3 text-sm text-muted">Routes answered with a server error most often. Open one to see those requests.</p>
          {failing.length === 0 ? (
            <EmptyState title="No server errors">No request in this range was answered with a 5xx.</EmptyState>
          ) : (
            <Ranked
              items={failing.map((f) => ({
                key: `${f.method} ${f.path} ${f.service}`,
                main: (
                  <span className="font-mono text-[13px]">
                    <span className="text-muted">{f.method}</span> {f.path}
                  </span>
                ),
                sub: `${f.service}, last seen ${timeAgo(f.last)}`,
                count: f.errors,
                onOpen: () => set({ view: null, status: "5xx", method: f.method, q: f.path, range, from: null, to: null }),
              }))}
            />
          )}
        </Card>
        <Card>
          <h3 className="mb-1 text-lg font-semibold">Top errors</h3>
          <p className="mb-3 text-sm text-muted">Lines logged at error level, grouped by message. Open one to see every time it was logged.</p>
          {errors.length === 0 ? (
            <EmptyState title="No error lines">No service logged at error level in this range.</EmptyState>
          ) : (
            <Ranked
              items={errors.map((e) => ({
                key: `${e.message} ${e.service}`,
                main: <span className="line-clamp-2 font-mono text-[13px] break-all">{e.message}</span>,
                sub: `${e.service}, last seen ${timeAgo(e.last)}`,
                count: e.count,
                onOpen: () => set({ view: "logs", level: "error", q: e.message.slice(0, 120), range, from: null, to: null }),
              }))}
            />
          )}
        </Card>
      </div>
    </div>
  );
}

function Figure({ label, value, note, bad }: { label: string; value: string; note?: string; bad?: boolean }) {
  return (
    <div className="flex flex-col-reverse">
      <dt className="text-sm text-muted">
        {label}
        {note && <span className="block text-faint">{note}</span>}
      </dt>
      <dd className={cx("font-mono text-2xl font-medium tabular-nums", bad && "text-err")}>{value}</dd>
    </div>
  );
}

/** A ranked list where each item opens what it counts. */
function Ranked({ items }: { items: { key: string; main: ReactNode; sub: string; count: number; onOpen: () => void }[] }) {
  return (
    <ol className="-mx-2">
      {items.map((item) => (
        <li key={item.key}>
          <button onClick={item.onOpen} className="flex w-full items-start gap-4 rounded-control px-2 py-2 text-left hover:bg-subtle">
            <span className="min-w-0 flex-1">
              <span className="block truncate">{item.main}</span>
              <span className="block text-xs text-muted">{item.sub}</span>
            </span>
            <span className="shrink-0 font-mono text-[13px] text-err tabular-nums">{item.count.toLocaleString()}</span>
          </button>
        </li>
      ))}
    </ol>
  );
}
