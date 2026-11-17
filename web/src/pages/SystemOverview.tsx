// The system overview on the Analytics page: the server, all apps together,
// platform activity, Docker's disk usage, and anything that needs attention.
//
// Chart colours are the validated categorical slots (blue/orange for server vs
// apps, aqua/violet for in vs out); every series also carries a legend entry and
// an end label, so identity never rests on colour alone.

import { useQuery } from "@tanstack/react-query";
import { useState, type ReactElement, type ReactNode } from "react";
import { Link } from "react-router";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api, bytes, timeAgo } from "../api";
import { SectionTitle } from "../ui";

const SERVER = "#2a78d6";
const APPS = "#eb6834";
const NET_IN = "#1baf7a";
const NET_OUT = "#6250d6";
const GRID = "#e9e7e4";
const MUTED = "#5f5a55";
const TRACK = "#cde2fb";
const WARN = "#c98500";
const DANGER = "#d63b25";

interface Server {
  sampled_at: string;
  cpu_percent: number;
  cpu_cores: number;
  memory_used: number;
  memory_total: number;
  disk_used: number;
  disk_total: number;
  net_rx_rate: number;
  net_tx_rate: number;
  uptime_seconds: number;
}

interface Point {
  ts: string;
  cpu_percent: number;
  apps_cpu_percent: number | null;
  memory_used: number;
  apps_memory: number | null;
  net_rx_rate: number | null;
  net_tx_rate: number | null;
}

interface DiskCategory {
  count: number;
  size: number;
  reclaimable: number;
}

interface Overview {
  server: Server | null;
  series: Point[];
  top_services: { id: string; name: string; project: string; cpu_percent: number; memory_bytes: number }[];
  platform: {
    projects: number;
    services: number;
    running: number;
    deploying: number;
    crashed: number;
    failed: number;
    stopped: number;
    deploys_today: number;
    deploys_failed_today: number;
    build_success_rate: number | null;
    last_backup: { status: string; created_at: string; service: string } | null;
    failed_backups_24h: number;
  };
  docker_disk: {
    images: DiskCategory;
    containers: DiskCategory;
    volumes: DiskCategory;
    build_cache: DiskCategory;
    sampled_at: string;
  } | null;
  warnings: { level: "critical" | "warning"; text: string; link: string | null }[];
}

const RANGES = [
  { minutes: 60, label: "1h" },
  { minutes: 360, label: "6h" },
  { minutes: 1440, label: "24h" },
];

const pct = (part: number, whole: number) => (whole > 0 ? (part / whole) * 100 : 0);
const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function uptime(seconds: number): string {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`;
}

export default function SystemOverview() {
  const [minutes, setMinutes] = useState(60);
  const overview = useQuery({
    queryKey: ["overview", minutes],
    queryFn: () => api.get<Overview>(`/overview?minutes=${minutes}`),
    refetchInterval: 15_000,
    placeholderData: (previous) => previous,
  });
  const data = overview.data;

  return (
    <div className="grid gap-8">
      <SectionTitle
        action={
          <div className="flex gap-1 rounded-sm bg-canvas-tint p-1" role="group" aria-label="Chart range">
            {RANGES.map((r) => (
              <button
                key={r.minutes}
                onClick={() => setMinutes(r.minutes)}
                aria-pressed={minutes === r.minutes}
                className={`rounded-xs px-3 py-1.5 text-sm font-medium ${minutes === r.minutes ? "bg-canvas border border-hairline" : "text-ink/70 hover:text-ink"}`}
              >
                {r.label}
              </button>
            ))}
          </div>
        }
      >
        Server
      </SectionTitle>

      {data && data.warnings.length > 0 && <Warnings warnings={data.warnings} />}

      {!data ? (
        <p>Loading…</p>
      ) : !data.server ? (
        <p className="rounded-md bg-canvas-soft p-6">
          Collecting the first server samples. Figures appear within a minute.
        </p>
      ) : (
        <>
          <ServerTiles server={data.server} />
          <Charts series={data.series} server={data.server} />
        </>
      )}

      {data && (
        <div className="grid gap-8 xl:grid-cols-3">
          <Platform platform={data.platform} />
          <DockerDisk disk={data.docker_disk} />
          <TopServices services={data.top_services} />
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- warnings

function Warnings({ warnings }: { warnings: Overview["warnings"] }) {
  return (
    <ul className="divide-y divide-divider rounded-md border border-hairline bg-canvas" aria-label="Needs attention">
      {warnings.map((w, i) => (
        <li key={i} className="flex items-start gap-3 px-5 py-3">
          <span
            className="mt-0.5 inline-flex size-5 shrink-0 items-center justify-center rounded-full text-xs font-extrabold text-white"
            style={{ background: w.level === "critical" ? DANGER : WARN }}
            aria-hidden
          >
            !
          </span>
          <span className="flex-1">
            <span className="sr-only">{w.level === "critical" ? "Critical: " : "Warning: "}</span>
            {w.text}
          </span>
          {w.link && (
            <Link to={w.link} className="shrink-0 text-sm font-medium">
              Open
            </Link>
          )}
        </li>
      ))}
    </ul>
  );
}

// ---------------------------------------------------------------- tiles

function Tile({ label, value, sub, meter }: { label: string; value: ReactNode; sub?: ReactNode; meter?: number }) {
  return (
    <div className="rounded-md bg-canvas-warm p-5">
      <p className="text-sm">{label}</p>
      <p className="mt-1 text-2xl font-semibold">{value}</p>
      {meter !== undefined && <Meter percent={meter} />}
      {sub && <p className="mt-2 text-sm text-ink/70">{sub}</p>}
    </div>
  );
}

/** Fill carries severity; the track is a lighter step of the fill's ramp. */
function Meter({ percent }: { percent: number }) {
  const fill = percent >= 95 ? DANGER : percent >= 85 ? WARN : SERVER;
  return (
    <div
      className="mt-3 h-2 overflow-hidden rounded-full"
      style={{ background: TRACK }}
      role="meter"
      aria-valuenow={Math.round(percent)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div className="h-full rounded-full" style={{ width: `${Math.min(100, percent)}%`, background: fill }} />
    </div>
  );
}

function ServerTiles({ server: s }: { server: Server }) {
  const memory = pct(s.memory_used, s.memory_total);
  const disk = pct(s.disk_used, s.disk_total);
  return (
    <div className="grid grid-cols-2 gap-4 md:grid-cols-3 xl:grid-cols-5">
      <Tile label="CPU" value={`${s.cpu_percent.toFixed(1)}%`} meter={s.cpu_percent} sub={`${s.cpu_cores} cores`} />
      <Tile label="Memory" value={`${memory.toFixed(0)}%`} meter={memory} sub={`${bytes(s.memory_used)} of ${bytes(s.memory_total)}`} />
      <Tile label="Disk" value={`${disk.toFixed(0)}%`} meter={disk} sub={`${bytes(s.disk_total - s.disk_used)} free of ${bytes(s.disk_total)}`} />
      <Tile
        label="Network"
        value={`${bytes(s.net_rx_rate + s.net_tx_rate)}/s`}
        sub={`${bytes(s.net_rx_rate)}/s in · ${bytes(s.net_tx_rate)}/s out`}
      />
      <Tile label="Uptime" value={uptime(s.uptime_seconds)} sub={`Sampled ${timeAgo(s.sampled_at)}`} />
    </div>
  );
}

// ---------------------------------------------------------------- charts

interface SeriesDef {
  key: keyof Point;
  name: string;
  color: string;
}

/** Tooltip in ink: the coloured key beside each value carries the identity. */
function ChartTooltip({
  active,
  payload,
  label,
  series,
  format,
}: {
  active?: boolean;
  payload?: readonly { dataKey?: unknown; value?: unknown }[];
  label?: unknown;
  series: SeriesDef[];
  format: (v: number) => string;
}) {
  if (!active || !payload?.length) return null;
  return (
    <div className="rounded-md border border-hairline bg-canvas px-3 py-2 text-sm">
      <p className="mb-1 font-semibold">{time(String(label))}</p>
      {series.map((s) => {
        const item = payload.find((p) => p.dataKey === s.key);
        if (item?.value == null) return null;
        return (
          <p key={s.name} className="flex items-center gap-2">
            <span className="h-0.5 w-3 rounded-full" style={{ background: s.color }} aria-hidden />
            {s.name} <span className="ml-auto pl-3 tabular-nums">{format(Number(item.value))}</span>
          </p>
        );
      })}
    </div>
  );
}

/** A line chart with a legend above, end labels, a crosshair tooltip and one axis. */
function ChartCard({
  title,
  data,
  series,
  format,
  max,
}: {
  title: string;
  data: Point[];
  series: SeriesDef[];
  format: (v: number) => string;
  max?: number;
}) {
  const last = data.length - 1;
  // End labels only when the lines end apart; converging labels would overprint,
  // and the legend and tooltip already name every line.
  const ends = series.map((s) => Number(data[last]?.[s.key] ?? 0));
  const top = max ?? Math.max(1, ...data.flatMap((p) => series.map((s) => Number(p[s.key] ?? 0))));
  const apart = ends.length < 2 || Math.abs(ends[0]! - ends[1]!) / top > 0.12;
  const endLabel =
    (s: SeriesDef) =>
    ({ index, x, y }: { index?: number; x?: number | string; y?: number | string }): ReactElement =>
      apart && index === last && x != null && y != null ? (
        <text x={Number(x) + 6} y={Number(y) + 4} fontSize={12} fill={MUTED}>
          {s.name}
        </text>
      ) : (
        <g />
      );
  return (
    <div className="rounded-md border border-hairline bg-canvas p-5">
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h3 className="text-lg font-extrabold">{title}</h3>
        <div className="flex gap-4 text-sm" aria-label="Legend">
          {series.map((s) => (
            <span key={s.name} className="inline-flex items-center gap-1.5">
              <span className="h-0.5 w-4 rounded-full" style={{ background: s.color }} aria-hidden />
              {s.name}
            </span>
          ))}
        </div>
      </div>
      <div className="h-52">
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={data} margin={{ top: 8, right: 48, bottom: 0, left: 0 }}>
            <CartesianGrid stroke={GRID} vertical={false} />
            <XAxis dataKey="ts" tickFormatter={time} stroke={MUTED} fontSize={12} tickLine={false} axisLine={false} minTickGap={48} />
            <YAxis
              tickFormatter={(v) => format(Number(v))}
              stroke={MUTED}
              fontSize={12}
              tickLine={false}
              axisLine={false}
              width={68}
              domain={[0, max ?? "auto"]}
            />
            <Tooltip
              cursor={{ stroke: MUTED, strokeWidth: 1 }}
              content={(props) => (
                <ChartTooltip
                  active={props.active}
                  payload={props.payload as readonly { dataKey?: unknown; value?: unknown }[] | undefined}
                  label={props.label}
                  series={series}
                  format={format}
                />
              )}
            />
            {series.map((s) => (
              <Line
                key={s.name}
                type="monotone"
                dataKey={s.key}
                name={s.name}
                stroke={s.color}
                strokeWidth={2}
                dot={false}
                activeDot={{ r: 4, stroke: "#ffffff", strokeWidth: 2 }}
                isAnimationActive={false}
                connectNulls
                label={endLabel(s)}
              />
            ))}
          </LineChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}

function Charts({ series, server }: { series: Point[]; server: Server }) {
  if (series.length < 2) {
    return <p className="rounded-md bg-canvas-soft p-6">Charts fill in as samples arrive, one every 30 seconds.</p>;
  }
  return (
    <div className="grid gap-8 xl:grid-cols-3">
      <ChartCard
        title="CPU"
        data={series}
        format={(v) => `${v.toFixed(0)}%`}
        max={100}
        series={[
          { key: "cpu_percent", name: "Server", color: SERVER },
          { key: "apps_cpu_percent", name: "Apps", color: APPS },
        ]}
      />
      <ChartCard
        title="Memory"
        data={series}
        format={(v) => bytes(v)}
        max={server.memory_total}
        series={[
          { key: "memory_used", name: "Server", color: SERVER },
          { key: "apps_memory", name: "Apps", color: APPS },
        ]}
      />
      <ChartCard
        title="Network"
        data={series}
        format={(v) => `${bytes(v)}/s`}
        series={[
          { key: "net_rx_rate", name: "In", color: NET_IN },
          { key: "net_tx_rate", name: "Out", color: NET_OUT },
        ]}
      />
    </div>
  );
}

// ---------------------------------------------------------------- platform, disk, top services

function Panel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="rounded-md border border-hairline bg-canvas p-5">
      <h3 className="mb-4 text-lg font-extrabold">{title}</h3>
      {children}
    </div>
  );
}

function Platform({ platform: p }: { platform: Overview["platform"] }) {
  const problems = [
    p.crashed && `${p.crashed} crashed`,
    p.failed && `${p.failed} failed`,
    p.deploying && `${p.deploying} deploying`,
    p.stopped && `${p.stopped} stopped`,
  ].filter(Boolean);
  return (
    <Panel title="Activity">
      <div className="grid grid-cols-2 gap-4">
        <Tile
          label="Services running"
          value={`${p.running} of ${p.services}`}
          sub={problems.length ? problems.join(", ") : `${p.projects} projects`}
        />
        <Tile
          label="Deploys today"
          value={p.deploys_today.toLocaleString()}
          sub={p.deploys_failed_today ? `${p.deploys_failed_today} failed` : "none failed"}
        />
        <Tile
          label="Build success, 7 days"
          value={p.build_success_rate == null ? "—" : `${p.build_success_rate.toFixed(0)}%`}
        />
        <Tile
          label="Last backup"
          value={p.last_backup ? timeAgo(p.last_backup.created_at) : "None"}
          sub={
            p.last_backup
              ? `${p.last_backup.service}, ${p.last_backup.status}${p.failed_backups_24h ? ` · ${p.failed_backups_24h} failed in 24h` : ""}`
              : "No backups yet"
          }
        />
      </div>
    </Panel>
  );
}

function DockerDisk({ disk }: { disk: Overview["docker_disk"] }) {
  if (!disk) {
    return (
      <Panel title="Docker disk usage">
        <p>Measuring; this can take a minute on a busy server.</p>
      </Panel>
    );
  }
  const rows = [
    { name: "Images", ...disk.images },
    { name: "Build cache", ...disk.build_cache },
    { name: "Volumes", ...disk.volumes },
    { name: "Containers", ...disk.containers },
  ];
  const largest = Math.max(...rows.map((r) => r.size), 1);
  const total = rows.reduce((sum, r) => sum + r.size, 0);
  const reclaimable = rows.reduce((sum, r) => sum + r.reclaimable, 0);
  return (
    <Panel title="Docker disk usage">
      <p className="mb-4 text-sm text-ink/70">
        {bytes(total)} in total, {bytes(reclaimable)} reclaimable · measured {timeAgo(disk.sampled_at)}
      </p>
      <ul className="grid gap-4">
        {rows.map((r) => (
          <li key={r.name}>
            <div className="flex items-baseline justify-between gap-3 text-sm">
              <span className="font-semibold">
                {r.name} <span className="font-normal text-ink/70">({r.count})</span>
              </span>
              <span>
                {bytes(r.size)}
                <span className="text-ink/70"> · {bytes(r.reclaimable)} reclaimable</span>
              </span>
            </div>
            <div className="mt-1.5 h-2 rounded-full" style={{ background: TRACK }} title={`${r.name}: ${bytes(r.size)}`}>
              <div className="h-full rounded-full" style={{ width: `${pct(r.size, largest)}%`, background: SERVER }} />
            </div>
          </li>
        ))}
      </ul>
      {disk.build_cache.reclaimable > 1024 ** 3 && (
        <p className="mt-4 text-sm">
          Clear unused build cache on the server with <code className="text-xs">docker builder prune</code>.
        </p>
      )}
    </Panel>
  );
}

function TopServices({ services }: { services: Overview["top_services"] }) {
  return (
    <Panel title="Heaviest services">
      {services.length === 0 ? (
        <p>No running services yet.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead className="bg-canvas-tint text-xs">
            <tr>
              <th className="px-3 py-2 font-semibold">Service</th>
              <th className="px-3 py-2 text-right font-semibold">CPU</th>
              <th className="px-3 py-2 text-right font-semibold">Memory</th>
            </tr>
          </thead>
          <tbody>
            {services.map((s) => (
              <tr key={s.id} className="border-b border-hairline last:border-b-0">
                <td className="px-3 py-2.5">
                  <Link to={`/services/${s.id}?tab=metrics`} className="font-medium">
                    {s.name}
                  </Link>
                  <span className="text-ink/70"> · {s.project}</span>
                </td>
                <td className="px-3 py-2.5 text-right tabular-nums">{s.cpu_percent.toFixed(1)}%</td>
                <td className="px-3 py-2.5 text-right tabular-nums">{bytes(s.memory_bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
