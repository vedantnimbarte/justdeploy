import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, micros, type Span, type TraceDetail } from "../../api";
import { cx, ErrorText } from "../../ui";
import { LogRow } from "./Logs";

/** The list, with the open request's panel beside it on wide screens. */
export function Split({ panel, children }: { panel: ReactNode; children: ReactNode }) {
  return (
    <div className={cx("grid items-start gap-4", Boolean(panel) && "lg:grid-cols-[minmax(0,1fr)_minmax(0,28rem)] xl:grid-cols-[minmax(0,1fr)_minmax(0,32rem)]")}>
      <div className="min-w-0">{children}</div>
      {panel}
    </div>
  );
}

/** A span with how deep it sits under the request. */
type Row = { span: Span; depth: number };

/**
 * Orders spans as a tree, parents before children. App spans whose parent was
 * Traefik's proxy span (not kept) hang under Traefik's request span.
 */
export function tree(spans: Span[]): Row[] {
  const ids = new Set(spans.map((s) => s.span_id));
  const edge = spans.find((s) => s.source === "edge");
  const children = new Map<string | null, Span[]>();
  for (const span of spans) {
    let parent = span.parent_span_id && ids.has(span.parent_span_id) ? span.parent_span_id : null;
    if (parent === null && edge && span !== edge) parent = edge.span_id;
    children.set(parent, [...(children.get(parent) ?? []), span]);
  }
  const rows: Row[] = [];
  const walk = (parent: string | null, depth: number) => {
    const kids = (children.get(parent) ?? []).sort((a, b) => a.start_ts.localeCompare(b.start_ts));
    for (const span of kids) {
      rows.push({ span, depth });
      walk(span.span_id, depth + 1);
    }
  };
  walk(null, 0);
  return rows;
}

const failed = (s: Span) => s.status === 2 || (s.http_status ?? 0) >= 500;

/** Microseconds since the epoch, keeping the precision Date drops. */
const startUs = (s: Span) => {
  const ms = Date.parse(s.start_ts);
  const fraction = /\.(\d+)/.exec(s.start_ts)?.[1] ?? "";
  return Math.floor(ms / 1000) * 1_000_000 + Number(fraction.padEnd(6, "0").slice(0, 6));
};

const statusTone = (status: number | null | undefined) =>
  status == null ? "text-muted" : status >= 500 ? "text-err" : status >= 400 ? "text-warn" : "text-ok";

/**
 * A request's details, its trace as a timeline, and what it logged. Beside the
 * list on wide screens; over everything on phones.
 */
export default function TracePanel({
  id,
  onClose,
  onNewer,
  onOlder,
}: {
  id: string;
  onClose: () => void;
  onNewer?: () => void;
  onOlder?: () => void;
}) {
  const trace = useQuery({
    queryKey: ["trace", id],
    queryFn: () => api.get<TraceDetail>(`/traces/${id}`),
    // App spans can arrive after Traefik's; look again while it's fresh.
    refetchInterval: 5_000,
  });
  const spans = trace.data?.spans;
  const rows = useMemo(() => tree(spans ?? []), [spans]);
  const [selected, setSelected] = useState<string | null>(null);
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => setSelected(null), [id]);
  // On phones the panel covers the page: put focus in it.
  useEffect(() => {
    if (window.matchMedia("(max-width: 1023px)").matches) close.current?.focus();
  }, []);

  const request = spans?.find((s) => s.source === "edge");
  const attrs = (request?.attrs ?? {}) as Record<string, unknown>;
  const text = (key: string) => (typeof attrs[key] === "string" || typeof attrs[key] === "number" ? String(attrs[key]) : null);
  const start = Math.min(...rows.map((r) => startUs(r.span)));
  const end = Math.max(...rows.map((r) => startUs(r.span) + r.span.duration_us));
  const total = Math.max(1, end - start);
  const chosen = rows.find((r) => r.span.span_id === selected)?.span ?? request ?? rows[0]?.span;
  const method = text("http.request.method");
  const path = text("url.path");

  return (
    <aside
      aria-label="Request details"
      className="fixed inset-0 z-40 overflow-y-auto bg-surface lg:sticky lg:inset-auto lg:top-4 lg:z-auto lg:max-h-[calc(100vh-2rem)] lg:rounded-panel lg:border lg:border-line"
    >
      <header className="sticky top-0 z-10 flex items-start gap-3 border-b border-line bg-surface px-5 py-4">
        <div className="min-w-0 flex-1">
          <h2 className="font-mono text-[15px] font-medium break-all">
            {request ? (
              <>
                <span className="text-muted">{method}</span> {path ?? request.name}
              </>
            ) : (
              "Request"
            )}
          </h2>
          {request && (
            <p className="mt-1 flex flex-wrap gap-x-4 text-sm text-muted">
              <span className={cx("font-mono font-medium", statusTone(request.http_status))}>{request.http_status ?? "–"}</span>
              <span className="font-mono">{micros(request.duration_us)}</span>
              <span>{request.service}</span>
              <time dateTime={request.start_ts}>{new Date(request.start_ts).toLocaleString()}</time>
            </p>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {(onNewer || onOlder) && (
            <>
              <NavButton label="Newer request" keys="ArrowUp" onClick={onNewer}>
                ↑
              </NavButton>
              <NavButton label="Older request" keys="ArrowDown" onClick={onOlder}>
                ↓
              </NavButton>
            </>
          )}
          <button
            ref={close}
            onClick={onClose}
            aria-label="Close request details"
            aria-keyshortcuts="Escape"
            className="grid size-8 place-items-center rounded-control text-xl leading-none text-muted hover:bg-subtle hover:text-fg"
          >
            ×
          </button>
        </div>
      </header>

      <div className="px-5 py-5">
        <ErrorText error={trace.error} />
        {trace.isPending && <p className="text-muted">Loading…</p>}
        {request && (
          <dl className="mb-6 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-sm">
            {[
              ["Host", text("server.address")],
              ["Query", text("url.query") || null],
              ["Client", text("client.address")],
              ["User agent", text("user_agent.original")],
              ["Trace", id],
            ]
              .filter(([, value]) => value)
              .map(([label, value]) => (
                <div key={label} className="contents">
                  <dt className="text-muted">{label}</dt>
                  <dd className="truncate font-mono text-[13px]" title={value!}>
                    {value}
                  </dd>
                </div>
              ))}
          </dl>
        )}
        {trace.data && rows.length === 0 && <p className="text-muted">This request's trace is no longer kept.</p>}
        {rows.length > 0 && (
          <section className="mb-6">
            <h3 className="mb-2 font-semibold">Timeline</h3>
            {rows.length === 1 && (
              <p className="mb-3 text-sm text-muted">
                Only the request itself is traced. With an OpenTelemetry SDK in the app, its database calls, outgoing requests and other
                work show here as well.
              </p>
            )}
            <ol className="overflow-hidden rounded-control border border-line">
              {rows.map(({ span, depth }) => {
                const left = ((startUs(span) - start) / total) * 100;
                const width = Math.max(0.6, (span.duration_us / total) * 100);
                const isChosen = chosen?.span_id === span.span_id;
                return (
                  <li key={span.span_id} className="border-b border-line last:border-0">
                    <button
                      onClick={() => setSelected(span.span_id)}
                      aria-pressed={isChosen}
                      className={cx("block w-full px-3 py-1.5 text-left text-sm focus-visible:-outline-offset-2", isChosen ? "bg-subtle" : "hover:bg-subtle/60")}
                    >
                      <span className="flex items-baseline justify-between gap-3">
                        <span className="min-w-0 truncate" style={{ paddingLeft: `${depth * 12}px` }} title={span.name}>
                          <span className="text-muted">{span.source === "edge" ? "traefik" : span.service}</span> {span.name}
                        </span>
                        <span className="shrink-0 font-mono text-[13px] text-muted tabular-nums">{micros(span.duration_us)}</span>
                      </span>
                      <span className="relative mt-1 block h-1.5 rounded-full bg-subtle">
                        <span
                          className={cx("absolute inset-y-0 rounded-full", failed(span) ? "bg-err" : span.source === "edge" ? "bg-fg/70" : "bg-muted")}
                          style={{ left: `${Math.min(left, 99.4)}%`, width: `${width}%` }}
                        />
                      </span>
                    </button>
                  </li>
                );
              })}
            </ol>
          </section>
        )}
        {chosen && Object.keys(chosen.attrs).length > 0 && (
          <section className="mb-6">
            <h3 className="mb-2 font-semibold">{chosen === request ? "Request attributes" : chosen.name}</h3>
            <dl className="grid gap-x-4 gap-y-1 font-mono text-[13px] sm:grid-cols-[minmax(0,11rem)_minmax(0,1fr)]">
              {Object.entries(chosen.attrs).map(([key, value]) => (
                <div key={key} className="contents">
                  <dt className="truncate text-muted" title={key}>
                    {key}
                  </dt>
                  <dd className="mb-1 break-all sm:mb-0">{typeof value === "string" ? value : JSON.stringify(value)}</dd>
                </div>
              ))}
            </dl>
          </section>
        )}
        {trace.data && trace.data.logs.length > 0 && (
          <section>
            <h3 className="mb-2 font-semibold">Logged during this request</h3>
            <ol className="overflow-hidden rounded-control border border-line">
              {trace.data.logs.map((l) => (
                <LogRow key={l.id} line={l} showService />
              ))}
            </ol>
          </section>
        )}
      </div>
    </aside>
  );
}

function NavButton({ label, keys, onClick, children }: { label: string; keys: string; onClick?: () => void; children: ReactNode }) {
  return (
    <button
      onClick={onClick}
      disabled={!onClick}
      aria-label={label}
      aria-keyshortcuts={keys}
      title={label}
      className="grid size-8 place-items-center rounded-control text-muted hover:bg-subtle hover:text-fg disabled:opacity-30 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}
