import { useState, type FormEvent } from "react";
import { useSearchParams } from "react-router";
import { Button, cx, Input, Segmented, Select } from "../../ui";
import Logs from "./Logs";
import Overview from "./Overview";
import Requests from "./Requests";

type View = "requests" | "logs" | "overview";

const RANGES = [
  { id: "1h", hours: 1, label: "Last hour" },
  { id: "24h", hours: 24, label: "Last 24 hours" },
  { id: "7d", hours: 24 * 7, label: "Last 7 days" },
  { id: "30d", hours: 24 * 30, label: "Last 30 days" },
] as const;

/** What every view is narrowed to. */
export interface Scope {
  project: string;
  service: string | null;
  /** `production`, `preview`, or both when empty. */
  env: string;
  hours: number;
  /** A fixed window picked on the volume strip, instead of the last `hours`. */
  from: string | null;
  to: string | null;
  /** Following along: a relative range that isn't paused. */
  live: boolean;
}

/** The scope's time window as query values. */
export const windowOf = (s: Scope) => (s.from ? { from: s.from, to: s.to } : { hours: s.hours });

/** Builds `?a=1&b=2` from the values that are set. */
export function query(values: Record<string, string | number | null | undefined>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value !== null && value !== undefined && value !== "") params.set(key, String(value));
  }
  return params.size ? `?${params}` : "";
}

/** Filters live in the URL, so a link shows someone else exactly this view. */
export function useFilters() {
  const [params, setParams] = useSearchParams();
  return {
    get: (name: string) => params.get(name) ?? "",
    set: (patch: Record<string, string | null>) =>
      setParams(
        (current) => {
          const next = new URLSearchParams(current);
          for (const [key, value] of Object.entries(patch)) {
            if (value) next.set(key, value);
            else next.delete(key);
          }
          return next;
        },
        { replace: true },
      ),
  };
}

export const clock = (iso: string) =>
  new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/** A time short enough for the range: the hour for today, the date before that. */
export const stamp = (iso: string, long: boolean) =>
  long ? new Date(iso).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : clock(iso);

/** A range's ends: minutes are enough. */
export const minutes = (iso: string, long: boolean) =>
  new Date(iso).toLocaleString([], long ? { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" } : { hour: "2-digit", minute: "2-digit" });

/**
 * Requests, logs and errors of a project, or of one service when `serviceId`
 * is given: the same view on the project's and the service's page.
 */
export default function Observability({
  projectId,
  services,
  serviceId,
}: {
  projectId: string;
  services?: { id: string; name: string }[];
  serviceId?: string;
}) {
  const { get, set } = useFilters();
  const [paused, setPaused] = useState(false);
  const view: View = get("view") === "logs" || get("view") === "overview" ? (get("view") as View) : "requests";
  const range = RANGES.find((r) => r.id === get("range")) ?? RANGES[0];
  const from = get("from") || null;
  const scope: Scope = {
    project: projectId,
    service: serviceId ?? (get("service") || null),
    env: get("env"),
    hours: range.hours,
    from,
    to: get("to") || null,
    // An open request holds the list still, so the row being read stays put.
    live: !from && !paused && !get("request"),
  };

  return (
    <>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <Segmented<View>
          label="Show"
          value={view}
          onChange={(v) => set({ view: v === "requests" ? null : v, request: null })}
          options={[
            { value: "requests", label: "Requests" },
            { value: "logs", label: "Logs" },
            { value: "overview", label: "Overview" },
          ]}
        />
        {view !== "overview" && (
          <Following scope={scope} held={Boolean(get("request"))} onPause={setPaused} onBackToLive={() => set({ from: null, to: null })} />
        )}
      </div>
      <div className="mb-3 flex flex-wrap gap-2">
        <Select
          value={range.id}
          onChange={(e) => set({ range: e.target.value === "1h" ? null : e.target.value, from: null, to: null })}
          aria-label="How far back"
          className="min-w-0 flex-1 basis-36 sm:flex-none sm:basis-auto sm:w-auto"
        >
          {RANGES.map((r) => (
            <option key={r.id} value={r.id}>
              {r.label}
            </option>
          ))}
        </Select>
        {!serviceId && services && services.length > 1 && (
          <Select value={scope.service ?? ""} onChange={(e) => set({ service: e.target.value || null })} aria-label="Service" className="min-w-0 flex-1 basis-36 sm:flex-none sm:basis-auto sm:w-auto">
            <option value="">All services</option>
            {services.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </Select>
        )}
        {view !== "overview" && (
          <Select value={scope.env} onChange={(e) => set({ env: e.target.value || null })} aria-label="Deployments" className="min-w-0 flex-1 basis-36 sm:flex-none sm:basis-auto sm:w-auto">
            <option value="">Production and previews</option>
            <option value="production">Production only</option>
            <option value="preview">Previews only</option>
          </Select>
        )}
      </div>
      {view === "requests" && <Requests scope={scope} showService={!serviceId} />}
      {view === "logs" && <Logs scope={scope} showService={!serviceId} />}
      {view === "overview" && <Overview scope={scope} />}
    </>
  );
}

/** Whether the list follows new arrivals, and how to stop or go back to that. */
function Following({
  scope,
  held,
  onPause,
  onBackToLive,
}: {
  scope: Scope;
  /** A request is open, which holds the list. */
  held: boolean;
  onPause: (paused: boolean) => void;
  onBackToLive: () => void;
}) {
  if (scope.from) {
    return (
      <div className="flex items-center gap-3 text-sm">
        <span className="text-muted">
          {minutes(scope.from, scope.hours > 24)} to {scope.to ? minutes(scope.to, scope.hours > 24) : "now"}
        </span>
        <Button variant="secondary" onClick={onBackToLive}>
          Back to live
        </Button>
      </div>
    );
  }
  if (held) {
    return (
      <span className="flex items-center gap-2 text-sm text-muted" aria-live="polite">
        <span className="size-2 rounded-full bg-off" aria-hidden />
        Paused while a request is open
      </span>
    );
  }
  return (
    <div className="flex items-center gap-3 text-sm">
      <span className="flex items-center gap-2 text-muted" aria-live="polite">
        <span className={cx("size-2 rounded-full", scope.live ? "pulse bg-ok" : "bg-off")} aria-hidden />
        {scope.live ? "Live" : "Paused"}
      </span>
      <Button variant="secondary" onClick={() => onPause(scope.live)}>
        {scope.live ? "Pause" : "Resume"}
      </Button>
    </div>
  );
}

/** A text filter applied on Enter, so each keystroke isn't a query. */
export function SearchBox({ value, placeholder, onSearch }: { value: string; placeholder: string; onSearch: (text: string) => void }) {
  const [text, setText] = useState(value);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSearch(text.trim());
  };
  return (
    <form onSubmit={submit} role="search" className="flex min-w-0 flex-1 basis-60 gap-2">
      <Input type="search" value={text} onChange={(e) => setText(e.target.value)} placeholder={placeholder} aria-label={placeholder} />
      <Button type="submit" variant="secondary">
        Search
      </Button>
    </form>
  );
}

/** A row of toggles, one picked, each with an optional count. */
export function Toggles<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string; count?: number; tone?: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="no-scrollbar inline-flex max-w-full overflow-x-auto rounded-control border border-line bg-subtle p-0.5" role="group" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          aria-pressed={o.value === value}
          className={cx(
            "flex shrink-0 items-baseline gap-1.5 rounded-[5px] px-2.5 py-1 text-sm font-medium",
            o.value === value ? "bg-surface text-fg shadow-[0_1px_2px_rgb(0_0_0/0.08)]" : "text-muted hover:text-fg",
          )}
        >
          {o.label}
          {o.count !== undefined && (
            <span className={cx("font-mono text-xs tabular-nums", o.count > 0 && o.tone ? o.tone : "text-faint")}>{o.count.toLocaleString()}</span>
          )}
        </button>
      ))}
    </div>
  );
}
