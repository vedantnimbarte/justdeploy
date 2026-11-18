import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";
import { api, kindLabel, type Service } from "../api";
import { cx, StatusDot } from "../ui";

type Edge = { from: string; to: string; keys: string[] };

const CARD_W = 232;
const CARD_H = 96;
const GAP_X = 80;
const GAP_Y = 28;
const PAD = 28;

/**
 * Columns by depth: what nothing depends on sits left (frontends), what only
 * gets depended on sits right (databases), so references read left to right.
 */
export function layout(services: Service[], edges: Edge[]) {
  const out = new Map<string, string[]>();
  edges.forEach((e) => out.set(e.from, [...(out.get(e.from) ?? []), e.to]));
  const depth = new Map<string, number>();
  const visit = (id: string, seen: Set<string>): number => {
    const known = depth.get(id);
    if (known !== undefined) return known;
    if (seen.has(id)) return 0; // a cycle: stop here
    seen.add(id);
    const d = Math.max(-1, ...(out.get(id) ?? []).map((to) => visit(to, seen))) + 1;
    depth.set(id, d);
    return d;
  };
  services.forEach((s) => visit(s.id, new Set()));
  const deepest = Math.max(0, ...depth.values());
  const columns: Service[][] = Array.from({ length: deepest + 1 }, () => []);
  const order = { app: 0, image: 1, database: 2 } as const;
  [...services]
    .sort((a, b) => order[a.kind] - order[b.kind] || a.name.localeCompare(b.name))
    .forEach((s) => columns[deepest - (depth.get(s.id) ?? 0)]?.push(s));
  const at = new Map<string, { x: number; y: number }>();
  const tallest = Math.max(...columns.map((c) => c.length));
  columns.forEach((column, c) => {
    // Shorter columns are centred against the tallest.
    const offset = ((tallest - column.length) * (CARD_H + GAP_Y)) / 2;
    column.forEach((s, r) => at.set(s.id, { x: PAD + c * (CARD_W + GAP_X), y: PAD + offset + r * (CARD_H + GAP_Y) }));
  });
  return {
    at,
    width: PAD * 2 + columns.length * CARD_W + (columns.length - 1) * GAP_X,
    height: PAD * 2 + tallest * CARD_H + (tallest - 1) * GAP_Y,
  };
}

/** The project's services as cards, joined where one references another. */
export function Canvas({ projectId, services }: { projectId: string; services: Service[] }) {
  const graph = useQuery({ queryKey: ["graph", projectId], queryFn: () => api.get<Edge[]>(`/projects/${projectId}/graph`) });
  const [hover, setHover] = useState<string | null>(null);
  const edges = graph.data ?? [];
  const { at, width, height } = layout(services, edges);
  const touches = (e: Edge) => hover !== null && (e.from === hover || e.to === hover);

  return (
    <div className="thin-scrollbar overflow-x-auto rounded-panel border border-line bg-subtle [background-image:radial-gradient(var(--jd-line-strong)_1px,transparent_1px)] [background-size:20px_20px]">
      <div className="relative" style={{ width, height, minWidth: "100%" }}>
        <svg className="pointer-events-none absolute inset-0" width={width} height={height} aria-hidden>
          <defs>
            <marker id="jd-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
              <path d="M0 0L8 4L0 8z" fill="var(--jd-muted)" />
            </marker>
          </defs>
          {edges.map((e) => {
            const a = at.get(e.from);
            const b = at.get(e.to);
            if (!a || !b) return null;
            const forward = b.x > a.x;
            const x1 = a.x + (forward ? CARD_W : 0);
            const x2 = b.x + (forward ? 0 : CARD_W);
            const y1 = a.y + CARD_H / 2;
            const y2 = b.y + CARD_H / 2;
            const bend = forward ? Math.max(40, (x2 - x1) / 2) : -60;
            return (
              <path
                key={`${e.from}-${e.to}`}
                d={`M${x1} ${y1} C${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2 - 1} ${y2}`}
                fill="none"
                stroke={touches(e) ? "var(--jd-fg)" : "var(--jd-muted)"}
                strokeWidth={touches(e) ? 2 : 1.25}
                markerEnd="url(#jd-arrow)"
              >
                <title>{e.keys.join(", ")}</title>
              </path>
            );
          })}
        </svg>
        {services.map((s) => {
          const p = at.get(s.id);
          if (!p) return null;
          return (
            <Link
              key={s.id}
              to={`/services/${s.id}`}
              onMouseEnter={() => setHover(s.id)}
              onMouseLeave={() => setHover(null)}
              onFocus={() => setHover(s.id)}
              onBlur={() => setHover(null)}
              className={cx(
                "absolute flex flex-col justify-between rounded-panel border bg-surface px-4 py-3 no-underline shadow-[0_1px_2px_rgb(0_0_0/0.06)]",
                s.status === "crashed" || s.status === "failed" ? "border-err/60" : "border-line hover:border-line-strong",
              )}
              style={{ left: p.x, top: p.y, width: CARD_W, height: CARD_H }}
            >
              <div className="flex items-baseline justify-between gap-2">
                <span className="truncate font-semibold">{s.name}</span>
                <span className="shrink-0 text-[12px] text-muted">{kindLabel(s)}</span>
              </div>
              <StatusDot status={s.status} className="!text-[13px]" />
              <span className="truncate font-mono text-[12px] text-muted">
                {s.url
                  ? s.url.replace(/^https?:\/\//, "")
                  : s.cron_schedule
                    ? `runs on ${s.cron_schedule}`
                    : s.port
                      ? `${s.name}:${s.port}`
                      : "worker, no port"}
              </span>
            </Link>
          );
        })}
      </div>
    </div>
  );
}
