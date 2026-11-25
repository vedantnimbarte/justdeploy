import { keepPreviousData, useInfiniteQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api, type Level, type LogLine } from "../../api";
import { Button, cx, EmptyState, ErrorText } from "../../ui";
import { query, SearchBox, stamp, Toggles, useFilters, windowOf, type Scope } from "./Observability";
import TracePanel, { Split } from "./TraceView";

/** Each level's word and tone; only warnings and errors get colour. */
const LEVELS: Record<Level, { word: string; tone: string; edge: string }> = {
  0: { word: "debug", tone: "text-faint", edge: "border-l-transparent" },
  1: { word: "info", tone: "text-faint", edge: "border-l-transparent" },
  2: { word: "warn", tone: "text-warn", edge: "border-l-warn" },
  3: { word: "error", tone: "text-err", edge: "border-l-err" },
};

export default function Logs({ scope, showService }: { scope: Scope; showService: boolean }) {
  const { get, set } = useFilters();
  const level = get("level");
  const q = get("q");
  const open = get("request") || null;
  const filters = { service: scope.service, env: scope.env, ...windowOf(scope), level, q };
  const results = useInfiniteQuery({
    queryKey: ["logs", scope.project, filters],
    initialPageParam: null as number | null,
    queryFn: ({ pageParam }) => api.get<LogLine[]>(`/projects/${scope.project}/logs${query({ ...filters, before: pageParam })}`),
    getNextPageParam: (last) => (last.length === 200 ? last.at(-1)?.id : undefined),
    refetchInterval: scope.live ? 5_000 : false,
    placeholderData: keepPreviousData,
  });
  const lines = results.data?.pages.flat() ?? [];

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Toggles
          label="Level"
          value={level}
          onChange={(l) => set({ level: l || null })}
          options={[
            { value: "", label: "All levels" },
            { value: "error", label: "Errors" },
            { value: "warn", label: "Warnings" },
            { value: "info", label: "Info" },
            { value: "debug", label: "Debug" },
          ]}
        />
        <SearchBox key={q} value={q} placeholder="Text in the line" onSearch={(text) => set({ q: text || null })} />
      </div>
      <ErrorText error={results.error} />
      <Split panel={open && <TracePanel id={open} onClose={() => set({ request: null })} />}>
        {results.isPending ? (
          <p className="py-6 text-muted">Loading logs…</p>
        ) : lines.length === 0 ? (
          <EmptyState
            title={level || q || scope.from ? "No lines match" : "No logs yet"}
            action={
              (level || q || scope.from) && (
                <Button variant="secondary" onClick={() => set({ level: null, q: null, from: null, to: null })}>
                  Clear filters
                </Button>
              )
            }
          >
            {level || q || scope.from
              ? "Nothing printed in this range matches the level or text you picked."
              : "What this project's services print is kept here for a while (7 days unless the server says otherwise), with each line's level."}
          </EmptyState>
        ) : (
          <div className="overflow-hidden rounded-panel border border-line bg-surface">
            <ol aria-label="Log lines, newest first" className={cx("divide-y divide-line", results.isPlaceholderData && "opacity-60")}>
              {lines.map((l) => (
                <LogRow
                  key={l.id}
                  line={l}
                  showService={showService}
                  long={scope.hours > 24}
                  open={l.trace_id === open}
                  onTrace={(trace) => set({ request: trace })}
                />
              ))}
            </ol>
            {results.hasNextPage && (
              <div className="border-t border-line px-3 py-2">
                <Button variant="tertiary" onClick={() => results.fetchNextPage()} busy={results.isFetchingNextPage}>
                  Load older lines
                </Button>
              </div>
            )}
          </div>
        )}
      </Split>
    </>
  );
}

/** One kept line. A JSON line shows its message and opens to its fields. */
export function LogRow({
  line,
  showService,
  long,
  open,
  onTrace,
}: {
  line: LogLine;
  showService: boolean;
  long?: boolean;
  open?: boolean;
  onTrace?: (trace: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const level = LEVELS[line.level] ?? LEVELS[1];
  const text = message(line);
  return (
    <li className={cx("border-l-2 px-3 py-1.5 font-mono text-[13px] leading-5", level.edge, open && "bg-subtle")}>
      <div className="flex flex-wrap gap-x-3 sm:flex-nowrap">
        <time dateTime={line.ts} className="shrink-0 text-faint tabular-nums" title={new Date(line.ts).toLocaleString()}>
          {stamp(line.ts, Boolean(long))}
        </time>
        <span className={cx("w-10 shrink-0", level.tone)}>{level.word}</span>
        {showService && <span className="shrink-0 text-muted">{line.service}</span>}
        <span className="w-full min-w-0 sm:w-auto sm:flex-1">
          {line.fields ? (
            <button className="text-left break-all whitespace-pre-wrap hover:underline" onClick={() => setExpanded(!expanded)} aria-expanded={expanded}>
              {text}
            </button>
          ) : (
            <span className="break-all whitespace-pre-wrap">{text}</span>
          )}
        </span>
        {line.trace_id && onTrace && (
          <button className="shrink-0 font-sans text-xs text-muted underline decoration-line-strong underline-offset-2 hover:text-fg" onClick={() => onTrace(line.trace_id!)}>
            Request
          </button>
        )}
      </div>
      {expanded && line.fields && (
        <pre className="mt-1.5 mb-1 overflow-x-auto rounded-control bg-subtle p-3 text-muted">{JSON.stringify(line.fields, null, 2)}</pre>
      )}
    </li>
  );
}

/** A JSON line's message, or the whole line when it has none. */
function message(line: LogLine): string {
  const f = line.fields ?? {};
  const msg = f.msg ?? f.message;
  return typeof msg === "string" ? msg : line.line;
}
