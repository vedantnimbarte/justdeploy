import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useEffect, useState, type PointerEvent } from "react";
import { api, micros, type RequestRow, type RequestVolume } from "../../api";
import { Button, cx, EmptyState, ErrorText, Select } from "../../ui";
import { clock, minutes, query, SearchBox, stamp, Toggles, useFilters, windowOf, type Scope } from "./Observability";
import TracePanel, { Split } from "./TraceView";

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/** The coloured edge a row gets: only for answers that went wrong. */
const edge = (status: number | null) =>
  status != null && status >= 500 ? "border-l-err" : status != null && status >= 400 ? "border-l-warn" : "border-l-transparent";

const statusTone = (status: number | null) =>
  status == null ? "text-faint" : status >= 500 ? "text-err" : status >= 400 ? "text-warn" : status >= 300 ? "text-faint" : "text-muted";

export default function Requests({ scope, showService }: { scope: Scope; showService: boolean }) {
  const { get, set } = useFilters();
  const status = get("status");
  const method = get("method");
  const q = get("q");
  const open = get("request") || null;
  const shared = { service: scope.service, env: scope.env, ...windowOf(scope), method, q };
  const follow = scope.live ? 5_000 : false;

  const list = useInfiniteQuery({
    queryKey: ["requests", scope.project, shared, status],
    initialPageParam: null as number | null,
    queryFn: ({ pageParam }) =>
      api.get<RequestRow[]>(`/projects/${scope.project}/requests${query({ ...shared, status, before: pageParam })}`),
    getNextPageParam: (last) => (last.length === 200 ? last.at(-1)?.id : undefined),
    refetchInterval: follow,
    placeholderData: keepPreviousData,
  });
  const volume = useQuery({
    queryKey: ["request-volume", scope.project, shared],
    queryFn: () => api.get<RequestVolume>(`/projects/${scope.project}/requests/volume${query(shared)}`),
    refetchInterval: follow,
    placeholderData: keepPreviousData,
  });
  const rows = list.data?.pages.flat() ?? [];
  const totals = (volume.data?.buckets ?? []).reduce(
    (t, b) => ({ ok: t.ok + b.ok, redirect: t.redirect + b.redirect, client: t.client + b.client, server: t.server + b.server }),
    { ok: 0, redirect: 0, client: 0, server: 0 },
  );
  const slowest = Math.max(1, ...rows.map((r) => r.duration_us));

  // ↑ and ↓ (or k and j) move through the list while a request is open; Escape closes it.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof Element && e.target.closest("input, select, textarea");
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
      const at = rows.findIndex((r) => r.trace_id === open);
      const step = e.key === "ArrowDown" || e.key === "j" ? 1 : e.key === "ArrowUp" || e.key === "k" ? -1 : 0;
      if (e.key === "Escape") set({ request: null });
      else if (step && rows[at + step]) set({ request: rows[at + step]!.trace_id });
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, rows, set]);
  useEffect(() => {
    if (open) document.querySelector(`[data-trace="${open}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open]);

  const filtered = Boolean(status || method || q || scope.from);
  const at = rows.findIndex((r) => r.trace_id === open);

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Toggles
          label="Status"
          value={status}
          onChange={(s) => set({ status: s || null })}
          options={[
            { value: "", label: "All", count: totals.ok + totals.redirect + totals.client + totals.server },
            { value: "2xx", label: "2xx", count: totals.ok },
            { value: "3xx", label: "3xx", count: totals.redirect },
            { value: "4xx", label: "4xx", count: totals.client, tone: "text-warn" },
            { value: "5xx", label: "5xx", count: totals.server, tone: "text-err" },
          ]}
        />
        <Select value={method} onChange={(e) => set({ method: e.target.value || null })} aria-label="Method" className="min-w-0 flex-1 basis-36 sm:flex-none sm:basis-auto sm:w-auto">
          <option value="">Any method</option>
          {METHODS.map((m) => (
            <option key={m}>{m}</option>
          ))}
        </Select>
        <SearchBox key={q} value={q} placeholder="Path or host" onSearch={(text) => set({ q: text || null })} />
      </div>
      {volume.data && (
        <VolumeStrip volume={volume.data} status={status} long={scope.hours > 24} onPick={(from, to) => set({ from, to })} />
      )}
      <ErrorText error={list.error} />
      <Split
        panel={
          open && (
            <TracePanel
              id={open}
              onClose={() => set({ request: null })}
              onNewer={at > 0 ? () => set({ request: rows[at - 1]!.trace_id }) : undefined}
              onOlder={at >= 0 && at < rows.length - 1 ? () => set({ request: rows[at + 1]!.trace_id }) : undefined}
            />
          )
        }
      >
        {list.isPending ? (
          <p className="py-6 text-muted">Loading requests…</p>
        ) : rows.length === 0 ? (
          filtered ? (
            <EmptyState
              title="No requests match"
              action={
                <Button variant="secondary" onClick={() => set({ status: null, method: null, q: null, from: null, to: null })}>
                  Clear filters
                </Button>
              }
            >
              Nothing in this range matches the status, method or text you picked.
            </EmptyState>
          ) : (
            <EmptyState title="No requests yet">
              Requests to this project's public services appear here a few seconds after they're answered, with their status and how long
              they took.
            </EmptyState>
          )
        ) : (
          <div className="@container overflow-hidden rounded-panel border border-line bg-surface">
            <div
              className={cx(
                "hidden border-b border-line px-3 py-2 text-xs font-medium text-muted @2xl:grid",
                showService
                  ? "grid-cols-[5.75rem_2.5rem_minmax(0,1fr)_minmax(0,8rem)_9rem]"
                  : "grid-cols-[5.75rem_2.5rem_minmax(0,1fr)_9rem]",
                "gap-x-3 border-l-2 border-l-transparent",
              )}
              aria-hidden
            >
              <span>Time</span>
              <span>Status</span>
              <span>Request</span>
              {showService && <span>Service</span>}
              <span className="text-right">Duration</span>
            </div>
            <ol aria-label="Requests, newest first" className={cx("divide-y divide-line", list.isPlaceholderData && "opacity-60")}>
              {rows.map((r) => (
                <li key={r.id}>
                  <Row row={r} selected={r.trace_id === open} showService={showService} long={scope.hours > 24} slowest={slowest} onOpen={() => set({ request: r.trace_id })} />
                </li>
              ))}
            </ol>
            {list.hasNextPage && (
              <div className="border-t border-line px-3 py-2">
                <Button variant="tertiary" onClick={() => list.fetchNextPage()} busy={list.isFetchingNextPage}>
                  Load older requests
                </Button>
              </div>
            )}
          </div>
        )}
      </Split>
    </>
  );
}

function Row({
  row: r,
  selected,
  showService,
  long,
  slowest,
  onOpen,
}: {
  row: RequestRow;
  selected: boolean;
  showService: boolean;
  long: boolean;
  slowest: number;
  onOpen: () => void;
}) {
  // Durations span orders of magnitude, so the bar is on a log scale from 0.1 ms.
  const share = Math.max(0.04, Math.min(1, Math.log10(Math.max(r.duration_us, 100) / 100) / Math.max(0.5, Math.log10(slowest / 100))));
  return (
    <button
      onClick={onOpen}
      data-trace={r.trace_id}
      aria-current={selected || undefined}
      className={cx(
        "grid w-full grid-cols-[2.5rem_minmax(0,1fr)_5.5rem] items-center gap-x-3 gap-y-0.5 border-l-2 px-3 py-2 text-left text-sm focus-visible:-outline-offset-2",
        showService
          ? "@2xl:grid-cols-[5.75rem_2.5rem_minmax(0,1fr)_minmax(0,8rem)_9rem]"
          : "@2xl:grid-cols-[5.75rem_2.5rem_minmax(0,1fr)_9rem]",
        edge(r.status),
        selected ? "bg-subtle" : "hover:bg-subtle/60",
      )}
    >
      <time dateTime={r.ts} className="hidden font-mono text-[13px] text-faint tabular-nums @2xl:block" title={new Date(r.ts).toLocaleString()}>
        {stamp(r.ts, long)}
      </time>
      <span className={cx("font-mono text-[13px] font-medium tabular-nums", statusTone(r.status))}>{r.status ?? "–"}</span>
      <span className="min-w-0 truncate font-mono text-[13px]" title={`${r.method} ${r.host}${r.path}`}>
        <span className="text-muted">{r.method}</span> {r.path}
        <span className="hidden text-faint @2xl:inline"> {r.host}</span>
      </span>
      {showService && (
        <span className="hidden min-w-0 truncate @2xl:block">
          {r.service}
          {r.pr != null && <span className="text-muted"> PR {r.pr}</span>}
        </span>
      )}
      <span className="flex items-center gap-2">
        <span aria-hidden className="hidden h-1 flex-1 overflow-hidden rounded-full bg-subtle @2xl:block">
          <span className="block h-full rounded-full bg-line-strong" style={{ width: `${share * 100}%` }} />
        </span>
        <span className="ml-auto font-mono text-[13px] tabular-nums">{micros(r.duration_us)}</span>
      </span>
      {/* On phones the time and service move under the request. */}
      <span className="col-span-2 col-start-2 flex gap-3 text-xs text-faint @2xl:hidden">
        <time dateTime={r.ts}>{clock(r.ts)}</time>
        {showService && <span className="truncate">{r.service}</span>}
      </span>
    </button>
  );
}

/**
 * Requests over the range as stacked bars: answered fine in grey, 4xx in
 * amber, 5xx in red. Dragging across bars (or clicking one) narrows the range.
 */
function VolumeStrip({
  volume,
  status,
  long,
  onPick,
}: {
  volume: RequestVolume;
  status: string;
  long: boolean;
  onPick: (from: string, to: string) => void;
}) {
  const { buckets, step_secs } = volume;
  const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
  const totals = buckets.map((b) => b.ok + b.redirect + b.client + b.server);
  const max = Math.max(1, ...totals);
  const sum = totals.reduce((a, b) => a + b, 0);
  const failed = buckets.reduce((n, b) => n + b.server, 0);
  if (buckets.length === 0) return null;

  const indexAt = (e: PointerEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    return Math.max(0, Math.min(buckets.length - 1, Math.floor(((e.clientX - box.left) / box.width) * buckets.length)));
  };
  const finish = () => {
    if (!drag) return;
    const lo = Math.min(drag.from, drag.to);
    const hi = Math.max(drag.from, drag.to);
    const end = new Date(new Date(buckets[hi]!.t).getTime() + step_secs * 1000);
    setDrag(null);
    onPick(buckets[lo]!.t, end.toISOString());
  };
  // A segment is dimmed when the status filter leaves it out.
  const dim = (cls: string) => status !== "" && status !== cls;
  const segments = (b: (typeof buckets)[number]) => [
    { n: b.server, cls: "5xx", color: "bg-err" },
    { n: b.client, cls: "4xx", color: "bg-warn" },
    { n: b.redirect, cls: "3xx", color: "bg-faint/50" },
    { n: b.ok, cls: "2xx", color: "bg-line-strong" },
  ];

  return (
    <figure className="mb-4">
      <div
        role="img"
        aria-label={`${sum.toLocaleString()} requests in this range, ${failed.toLocaleString()} answered with a server error. Drag across the bars to zoom in.`}
        className="relative flex h-14 cursor-crosshair touch-none items-end gap-px select-none"
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          const i = indexAt(e);
          setDrag({ from: i, to: i });
        }}
        onPointerMove={(e) => drag && setDrag({ ...drag, to: indexAt(e) })}
        onPointerUp={finish}
        onPointerCancel={() => setDrag(null)}
      >
        {buckets.map((b, i) => (
          <div
            key={b.t}
            className="flex h-full min-w-0 flex-1 flex-col justify-end"
            title={`${stamp(b.t, long)}: ${totals[i]} requests${b.client ? `, ${b.client} 4xx` : ""}${b.server ? `, ${b.server} 5xx` : ""}`}
          >
            {totals[i] === 0 ? (
              <div className="h-px bg-line" />
            ) : (
              segments(b).map(
                (s) =>
                  s.n > 0 && (
                    <div
                      key={s.cls}
                      className={cx(s.color, dim(s.cls) && "opacity-25", "first:rounded-t-[2px]")}
                      style={{ height: `${Math.max(2, (s.n / max) * 100)}%` }}
                    />
                  ),
              )
            )}
          </div>
        ))}
        {drag && (
          <div
            aria-hidden
            className="pointer-events-none absolute inset-y-0 border-x border-fg/40 bg-fg/8"
            style={{
              left: `${(Math.min(drag.from, drag.to) / buckets.length) * 100}%`,
              width: `${((Math.abs(drag.to - drag.from) + 1) / buckets.length) * 100}%`,
            }}
          />
        )}
      </div>
      <figcaption className="mt-1.5 flex justify-between gap-4 text-xs text-faint">
        <time dateTime={buckets[0]!.t}>{minutes(buckets[0]!.t, long)}</time>
        <span className="hidden sm:inline">Drag across the bars to zoom in</span>
        <span>{minutes(buckets.at(-1)!.t, long)}</span>
      </figcaption>
    </figure>
  );
}
