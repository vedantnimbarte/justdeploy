import { useEffect, useRef, useState } from "react";

/**
 * Follows a server-sent event stream of log lines in a dark panel. Each event
 * may carry several lines. A named `end` event closes the stream.
 */
export function LogPanel({ url, title, height = "h-[32rem]" }: { url: string; title: string; height?: string }) {
  const [lines, setLines] = useState<string[]>([]);
  const [state, setState] = useState<"live" | "done" | "reconnecting">("live");
  const box = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  useEffect(() => {
    setLines([]);
    setState("live");
    const source = new EventSource(url);
    source.onmessage = (event) => {
      setState("live");
      const incoming = String(event.data).split("\n");
      // Keep the panel bounded on very long builds.
      setLines((current) => [...current, ...incoming].slice(-5000));
    };
    source.addEventListener("end", () => {
      setState("done");
      source.close();
    });
    source.onerror = () => setState(source.readyState === EventSource.CLOSED ? "done" : "reconnecting");
    return () => source.close();
  }, [url]);

  useEffect(() => {
    const el = box.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  return (
    <div className="overflow-hidden rounded-md bg-ink-deep text-white">
      <div className="flex items-center justify-between border-b border-white/10 px-4 py-3 text-sm">
        <span className="font-semibold">{title}</span>
        <span className="opacity-70">
          {state === "live" ? "Live" : state === "done" ? `${lines.length} lines` : "Reconnecting…"}
        </span>
      </div>
      <div
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className={`${height} overflow-auto px-4 py-3 font-mono text-[13px] leading-5`}
      >
        {lines.length === 0 ? (
          <span className="opacity-60">Waiting for output…</span>
        ) : (
          lines.map((line, i) => (
            <div key={i} className={`whitespace-pre-wrap break-all ${line.startsWith("==>") ? "font-semibold text-gold-soft" : ""}`}>
              {line || " "}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
