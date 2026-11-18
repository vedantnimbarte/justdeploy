import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Area, AreaChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api, bytes, type Project, type UsagePoint } from "../../api";
import { Card, EmptyState, ErrorText, Segmented, SectionTitle } from "../../ui";

type Range = "day" | "week" | "quarter";
type Metric = "cpu" | "memory" | "disk";

// The four validated chart slots, in fixed order; more services fold into Other.
const SLOTS = ["var(--jd-series-1)", "var(--jd-series-2)", "var(--jd-series-3)", "var(--jd-series-4)"];
const OTHER = "var(--jd-off)";
const OTHER_ID = "other";

const tick = (range: Range) => (iso: string) => {
  const d = new Date(iso);
  return range === "day"
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString([], { day: "numeric", month: "short" });
};

const format: Record<Metric, (v: number) => string> = {
  cpu: (v) => `${v.toFixed(v < 10 ? 1 : 0)}%`,
  memory: (v) => bytes(v),
  disk: (v) => bytes(v),
};

export default function Usage({ project }: { project: Project }) {
  const [range, setRange] = useState<Range>("day");
  const usage = useQuery({
    queryKey: ["usage", project.id, range],
    queryFn: () => api.get<UsagePoint[]>(`/projects/${project.id}/usage?range=${range}`),
    refetchInterval: range === "day" ? 60_000 : false,
  });

  // Colour follows the service (its place in the project), never its rank in a range.
  const series = project.services.slice(0, SLOTS.length).map((s, i) => ({ id: s.id, name: s.name, color: SLOTS[i]! }));
  const folded = new Set(project.services.slice(SLOTS.length).map((s) => s.id));
  if (folded.size) series.push({ id: OTHER_ID, name: `${folded.size} more`, color: OTHER });
  const key = (serviceId: string) => (folded.has(serviceId) ? OTHER_ID : serviceId);

  const rows = pivot(usage.data ?? [], key);
  const totals = summary(usage.data ?? [], project);

  return (
    <div className="grid max-w-6xl gap-6">
      <SectionTitle
        action={
          <Segmented<Range>
            label="Range"
            value={range}
            onChange={setRange}
            options={[
              { value: "day", label: "Day" },
              { value: "week", label: "Week" },
              { value: "quarter", label: "90 days" },
            ]}
          />
        }
      >
        Usage
      </SectionTitle>
      <p className="-mt-3 max-w-[68ch] text-muted">
        Every copy of every service in {project.name}, added up. The last day is in 15-minute steps; longer ranges come from hourly averages
        kept for 90 days. Disk is each service's volumes.
      </p>
      {usage.isError && <ErrorText error={usage.error} />}
      {usage.data && usage.data.length === 0 ? (
        <EmptyState title="No usage recorded yet">Samples are taken every 30 seconds while services run.</EmptyState>
      ) : (
        <>
          {series.length > 1 && (
            <ul className="flex flex-wrap gap-x-5 gap-y-2 text-sm" aria-label="Services">
              {series.map((s) => (
                <li key={s.id} className="flex items-center gap-2">
                  <span className="size-2.5 rounded-[3px]" style={{ background: s.color }} aria-hidden />
                  {s.name}
                </li>
              ))}
            </ul>
          )}
          {(["cpu", "memory", "disk"] as Metric[]).map((metric) =>
            metric === "disk" && !rows.disk.some((r) => Object.values(r).some((v) => typeof v === "number" && v > 0)) ? (
              <Card key={metric}>
                <h3 className="text-lg font-semibold">Disk</h3>
                <p className="mt-1 text-sm text-muted">
                  No volume sizes yet. They're measured once an hour, on servers running Docker 29 or newer.
                </p>
              </Card>
            ) : (
              <StackChart
                key={metric}
                title={metric === "cpu" ? "CPU (100% is one core)" : metric === "memory" ? "Memory" : "Disk"}
                rows={rows[metric]}
                series={series}
                format={format[metric]}
                tick={tick(range)}
              />
            ),
          )}
          <Card className="!p-0">
            <table className="w-full text-sm">
              <caption className="px-5 pt-5 pb-3 text-left font-semibold md:px-6">By service, over the range</caption>
              <thead className="border-y border-line bg-subtle text-[13px] text-muted">
                <tr>
                  <th className="px-5 py-2 text-left font-medium md:px-6">Service</th>
                  <th className="px-3 py-2 text-right font-medium">Average CPU</th>
                  <th className="px-3 py-2 text-right font-medium">Average memory</th>
                  <th className="px-5 py-2 text-right font-medium md:px-6">Disk now</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-line tabular-nums">
                {totals.map((t) => (
                  <tr key={t.id}>
                    <td className="px-5 py-2.5 md:px-6">{t.name}</td>
                    <td className="px-3 py-2.5 text-right">{format.cpu(t.cpu)}</td>
                    <td className="px-3 py-2.5 text-right">{bytes(t.memory)}</td>
                    <td className="px-5 py-2.5 text-right md:px-6">{t.disk ? bytes(t.disk) : "–"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
        </>
      )}
    </div>
  );
}

type Row = { t: string } & Record<string, number | string>;

/** One row per moment, a column per series, for each metric. Gaps count as nothing running. */
function pivot(points: UsagePoint[], key: (id: string) => string): Record<Metric, Row[]> {
  const out: Record<Metric, Map<string, Row>> = {
    cpu: new Map(),
    memory: new Map(),
    disk: new Map(),
  };
  for (const p of points) {
    for (const metric of ["cpu", "memory", "disk"] as Metric[]) {
      const row: Row = out[metric].get(p.t) ?? ({ t: p.t } as Row);
      const k = key(p.service_id);
      row[k] = ((row[k] as number | undefined) ?? 0) + p[metric];
      out[metric].set(p.t, row);
    }
  }
  return {
    cpu: [...out.cpu.values()],
    memory: [...out.memory.values()],
    disk: [...out.disk.values()],
  };
}

function summary(points: UsagePoint[], project: Project) {
  return project.services.map((s) => {
    const mine = points.filter((p) => p.service_id === s.id);
    const avg = (f: (p: UsagePoint) => number) => (mine.length ? mine.reduce((a, p) => a + f(p), 0) / mine.length : 0);
    const last = mine[mine.length - 1];
    return {
      id: s.id,
      name: s.name,
      cpu: avg((p) => p.cpu),
      memory: avg((p) => p.memory),
      disk: last?.disk ?? 0,
    };
  });
}

function StackChart({
  title,
  rows,
  series,
  format,
  tick,
}: {
  title: string;
  rows: Row[];
  series: { id: string; name: string; color: string }[];
  format: (v: number) => string;
  tick: (iso: string) => string;
}) {
  const axis = {
    stroke: "var(--jd-muted)",
    fontSize: 12,
    tickLine: false,
    axisLine: false,
  };
  return (
    <Card>
      <h3 className="mb-4 text-lg font-semibold">{title}</h3>
      <div className="h-56">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={rows} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
            <CartesianGrid stroke="var(--jd-line)" vertical={false} />
            <XAxis dataKey="t" tickFormatter={tick} minTickGap={40} {...axis} />
            <YAxis tickFormatter={(v: number) => format(v)} width={64} {...axis} />
            <Tooltip
              cursor={{ stroke: "var(--jd-line-strong)" }}
              content={({ active, payload, label }) =>
                active && payload?.length ? (
                  <div className="rounded-panel border border-line bg-surface px-3 py-2 text-[13px] shadow-[0_8px_24px_-8px_rgb(0_0_0/0.25)]">
                    <p className="mb-1 text-muted">{new Date(String(label)).toLocaleString()}</p>
                    {[...payload].reverse().map((item) => (
                      <p key={String(item.dataKey)} className="flex items-center gap-2 tabular-nums">
                        <span className="size-2 rounded-[2px]" style={{ background: item.color }} aria-hidden />
                        <span className="text-muted">{series.find((s) => s.id === item.dataKey)?.name}</span>
                        <span className="ml-auto pl-3">{format(Number(item.value))}</span>
                      </p>
                    ))}
                  </div>
                ) : null
              }
            />
            {series.map((s) => (
              <Area
                key={s.id}
                dataKey={s.id}
                stackId="1"
                type="monotone"
                fill={s.color}
                fillOpacity={0.85}
                // A surface-coloured edge keeps stacked bands apart.
                stroke="var(--jd-surface)"
                strokeWidth={2}
                isAnimationActive={false}
              />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </Card>
  );
}
