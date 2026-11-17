import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Area, AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api, bytes, type Sample, type ServiceDetail } from "../../api";
import { Card, EmptyState, SectionTitle, Tabs } from "../../ui";

const INK = "#312d2a";
const TEAL = "#006b8f";
const GRID = "#e9e7e4";

const RANGES = [
  { id: "60", label: "1 hour" },
  { id: "360", label: "6 hours" },
  { id: "1440", label: "24 hours" },
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
        <h3 className="text-lg font-extrabold">{title}</h3>
        <span className="font-mono text-sm">{latest}</span>
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
  const axis = { stroke: "#5f5a55", fontSize: 12, tickLine: false, axisLine: false };

  return (
    <div className="grid gap-8">
      <div className="-mx-4 md:-mx-10 xl:-mx-[72px]">
        <Tabs tabs={RANGES.map((r) => ({ id: r.id, label: r.label }))} active={range} onChange={setRange} />
      </div>
      <SectionTitle>Resource use</SectionTitle>
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
              <Area type="monotone" dataKey="cpu_percent" stroke={INK} fill="#f1efed" strokeWidth={2} isAnimationActive={false} />
            </AreaChart>
          </Chart>
          <Chart title="Memory" latest={bytes(last?.memory_bytes)}>
            <AreaChart data={data}>
              <CartesianGrid stroke={GRID} vertical={false} />
              <XAxis dataKey="ts" tickFormatter={time} {...axis} minTickGap={40} />
              <YAxis tickFormatter={(v) => bytes(Number(v))} {...axis} width={64} />
              <Tooltip labelFormatter={(v) => time(String(v))} formatter={(v) => [bytes(Number(v)), "Memory"]} />
              <Area type="monotone" dataKey="memory_bytes" stroke={INK} fill="#f1efed" strokeWidth={2} isAnimationActive={false} />
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
              <Line type="monotone" dataKey="net_rx_rate" stroke={INK} dot={false} strokeWidth={2} isAnimationActive={false} />
              <Line type="monotone" dataKey="net_tx_rate" stroke={TEAL} dot={false} strokeWidth={2} isAnimationActive={false} />
            </LineChart>
          </Chart>
        </div>
      )}
    </div>
  );
}
