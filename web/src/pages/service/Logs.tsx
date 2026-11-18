import { useInfiniteQuery } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { api, type LogLine, type ServiceDetail } from "../../api";
import { Button, ErrorText, Input, SectionTitle, Segmented, Select } from "../../ui";
import { LogPanel } from "./LogPanel";

type Mode = "live" | "search";

export default function Logs({ service }: { service: ServiceDetail }) {
  const [mode, setMode] = useState<Mode>("live");
  return (
    <>
      <SectionTitle
        action={
          <Segmented<Mode>
            label="Logs"
            value={mode}
            onChange={setMode}
            options={[
              { value: "live", label: "Live" },
              { value: "search", label: "Search" },
            ]}
          />
        }
      >
        Runtime output
      </SectionTitle>
      {mode === "live" ? (
        <>
          <p className="mb-5 max-w-3xl">What the running container prints, followed live. Build output lives under Deployments.</p>
          {/* Remount after a redeploy so the stream follows the new container. */}
          <LogPanel
            key={service.current_deployment_id ?? "none"}
            url={`/api/v1/services/${service.id}/logs?tail=500`}
            title={`${service.name} output`}
            height="h-[36rem]"
          />
        </>
      ) : (
        <Search service={service} />
      )}
    </>
  );
}

const RANGES = [
  { hours: 1, label: "Last hour" },
  { hours: 24, label: "Last 24 hours" },
  { hours: 24 * 7, label: "Last 7 days" },
  { hours: 24 * 30, label: "Last 30 days" },
];

function Search({ service }: { service: ServiceDetail }) {
  const [text, setText] = useState("");
  const [hours, setHours] = useState(24);
  const [query, setQuery] = useState<{ q: string; hours: number } | null>(null);
  const results = useInfiniteQuery({
    queryKey: ["log-search", service.id, query],
    enabled: query !== null,
    initialPageParam: null as number | null,
    queryFn: ({ pageParam }) =>
      api.get<LogLine[]>(
        `/services/${service.id}/logs/search?q=${encodeURIComponent(query!.q)}&hours=${query!.hours}${pageParam ? `&before=${pageParam}` : ""}`,
      ),
    // A full page means there may be more further back.
    getNextPageParam: (last) => (last.length === 200 ? last.at(-1)?.id : undefined),
  });
  const lines = results.data?.pages.flat() ?? [];
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setQuery({ q: text, hours });
  };

  return (
    <>
      <p className="mb-5 max-w-3xl">
        Everything the service printed is kept for a while (7 days unless the server says otherwise). Search it by any text; leave it empty
        to see everything in the range, newest first.
      </p>
      <form onSubmit={submit} className="mb-4 flex flex-wrap gap-2">
        <Input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="error, a request id, a user's email…"
          aria-label="Text to find"
          className="min-w-0 flex-1 basis-64"
        />
        <Select value={hours} onChange={(e) => setHours(Number(e.target.value))} aria-label="How far back" className="w-auto">
          {RANGES.map((r) => (
            <option key={r.hours} value={r.hours}>
              {r.label}
            </option>
          ))}
        </Select>
        <Button type="submit" busy={results.isFetching && !results.isFetchingNextPage}>
          Search
        </Button>
      </form>
      <ErrorText error={results.error} />
      {query && results.data && (
        <div className="overflow-hidden rounded-panel bg-console text-white">
          <div className="border-b border-white/10 px-4 py-3 text-sm opacity-70">
            {lines.length === 0
              ? "Nothing matched"
              : `${lines.length}${results.hasNextPage ? "+" : ""} line${lines.length === 1 ? "" : "s"}, newest first`}
          </div>
          {lines.length > 0 && (
            <div className="max-h-[36rem] overflow-auto px-4 py-3 font-mono text-[13px] leading-5">
              {lines.map((l) => (
                <div key={l.id} className="flex gap-4">
                  <time className="shrink-0 opacity-60" dateTime={l.ts}>
                    {new Date(l.ts).toLocaleString()}
                  </time>
                  <span className="min-w-0 whitespace-pre-wrap break-all">{l.line}</span>
                </div>
              ))}
              {results.hasNextPage && (
                <button
                  className="mt-3 text-sm underline opacity-80 hover:opacity-100"
                  onClick={() => results.fetchNextPage()}
                  disabled={results.isFetchingNextPage}
                >
                  {results.isFetchingNextPage ? "Loading…" : "Load older lines"}
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </>
  );
}
