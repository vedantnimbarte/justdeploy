import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Area, AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api, bytes, type Sample, type ServiceDetail } from "../../api";
import { Card, EmptyState, Segmented, SectionTitle } from "../../ui";

// One series per chart uses slot 1; network's in/out use slots 3 and 4, as on Analytics.
const LINE = "var(--jd-series-1)";
const NET_IN = "var(--jd-series-3)";
const NET_OUT = "var(--jd-series-4)";
const GRID = "var(--jd-line)";

const RANGES = [
  { id: "60", label: "1h" },
  { id: "360", label: "6h" },
  { id: "1440", label: "24h" },
] as const;

const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function Chart({
  title,
  latest,
  children,
}: {
  title: string;
  latest: string;
  children: React.ReactElement;
}) {
  return (
    <Card>
      <div className="mb-4 flex items-baseline justify-between gap-3">
        <h3 className="text-lg font-semibold">{title}</h3>
        <span className="text-sm tabular-nums text-muted">{latest}</span>
      </div>
      <div className="h-56">
        <ResponsiveContainer width="100%" height="100%">
          {children}
        </ResponsiveContainer>
      </div>
    </Card>
  );
}

export default function Metrics({ service }: { service: ServiceDetail }) {
  const [range, setRange] = useState<(typeof RANGES)[number]["id"]>("60");
  const samples = useQuery({
    queryKey: ["metrics", service.id, range],
    queryFn: () => api.get<Sample[]>(`/services/${service.id}/metrics?minutes=${range}`),
    refetchInterval: 30_000,
  });
  const data = samples.data ?? [];
  const last = data[data.length - 1];
  const axis = { stroke: "var(--jd-muted)", fontSize: 12, tickLine: false, axisLine: false };

  return (
    <div className="grid gap-6">
      <SectionTitle
        action={
          <Segmented
            label="Chart range"
            value={range}
            onChange={setRange}
            options={RANGES.map((r) => ({ value: r.id, label: r.label }))}
          />
        }
      >
        Resource use
      </SectionTitle>
      {data.length === 0 ? (
        <EmptyState title="No samples yet">
          Samples are taken every 30 seconds while the service runs, and kept for 24 hours.
        </EmptyState>
      ) : (
        <div className="grid gap-8 xl:grid-cols-3">
          <Chart title="CPU" latest={`${last?.cpu_percent.toFixed(1)}%`}>
            <AreaChart data={data}>
              <CartesianGrid stroke={GRID} vertical={false} />
              <XAxis dataKey="ts" tickFormatter={time} {...axis} minTickGap={40} />
              <YAxis tickFormatter={(v) => `${v}%`} {...axis} width={44} />
              <Tooltip labelFormatter={(v) => time(String(v))} formatter={(v) => [`${Number(v).toFixed(1)}%`, "CPU"]} />
              <Area type="monotone" dataKey="cpu_percent" stroke={LINE} fill="var(--jd-track)" strokeWidth={2} isAnimationActive={false} />
            </AreaChart>
          </Chart>
          <Chart title="Memory" latest={bytes(last?.memory_bytes)}>
            <AreaChart data={data}>
              <CartesianGrid stroke={GRID} vertical={false} />
              <XAxis dataKey="ts" tickFormatter={time} {...axis} minTickGap={40} />
              <YAxis tickFormatter={(v) => bytes(Number(v))} {...axis} width={64} />
              <Tooltip labelFormatter={(v) => time(String(v))} formatter={(v) => [bytes(Number(v)), "Memory"]} />
              <Area type="monotone" dataKey="memory_bytes" stroke={LINE} fill="var(--jd-track)" strokeWidth={2} isAnimationActive={false} />
            </AreaChart>
          </Chart>
          <Chart title="Network" latest={`↓ ${bytes(last?.net_rx_rate)}/s  ↑ ${bytes(last?.net_tx_rate)}/s`}>
            <LineChart data={data}>
              <CartesianGrid stroke={GRID} vertical={false} />
              <XAxis dataKey="ts" tickFormatter={time} {...axis} minTickGap={40} />
              <YAxis tickFormatter={(v) => `${bytes(Number(v))}/s`} {...axis} width={72} />
              <Tooltip
                labelFormatter={(v) => time(String(v))}
                formatter={(v, name) => [`${bytes(Number(v))}/s`, name === "net_rx_rate" ? "In" : "Out"]}
              />
              <Line type="monotone" dataKey="net_rx_rate" stroke={NET_IN} dot={false} strokeWidth={2} isAnimationActive={false} />
              <Line type="monotone" dataKey="net_tx_rate" stroke={NET_OUT} dot={false} strokeWidth={2} isAnimationActive={false} />
            </LineChart>
          </Chart>
        </div>
      )}
    </div>
  );
}
