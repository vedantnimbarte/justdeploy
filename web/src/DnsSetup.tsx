// Connecting a hostname: the record to set, a live check that it reaches this
// server, then the certificate. Used for custom domains and the base wildcard.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import { api, type DnsCheck } from "./api";
import { Button, CopyButton, cx } from "./ui";

const RECHECK_MS = 15_000;

const STATE: Record<DnsCheck["status"] | "checking", { dot: string; label: string; pulse?: boolean }> = {
  checking: { dot: "bg-off", label: "Checking", pulse: true },
  connected: { dot: "bg-ok", label: "Connected" },
  missing: { dot: "bg-warn", label: "Waiting for DNS", pulse: true },
  elsewhere: { dot: "bg-err", label: "Points elsewhere" },
  unknown: { dot: "bg-off", label: "Can't verify" },
};

export function useDnsCheck(host: string, enabled = true) {
  return useQuery({
    queryKey: ["dns", host],
    enabled,
    queryFn: () => api.get<DnsCheck>(`/dns-check?host=${encodeURIComponent(host)}`),
    refetchInterval: (q) => (q.state.data?.status === "connected" ? false : RECHECK_MS),
  });
}

/**
 * The record that points `host` here: a CNAME onto `cnameTarget` for a
 * subdomain when one is given, otherwise an A record at this server's IPv4.
 */
function recordFor(host: string, ip: string | undefined, cnameTarget?: string) {
  const labels = host.split(".");
  // ponytail: a two-label apex check reads example.co.uk as a subdomain; a public-suffix list fixes it if that bites.
  const apex = labels.length <= 2;
  const name = apex ? "@" : labels.slice(0, -2).join(".");
  return cnameTarget && !apex
    ? { type: "CNAME", name, value: cnameTarget }
    : { type: "A", name, value: ip ?? "" };
}

/**
 * One hostname as a row with its connection state, opening onto the steps
 * that connect it. Opens by itself until the hostname is connected.
 */
export function DnsPanel({
  host,
  cnameTarget,
  tls,
  actions,
}: {
  host: string;
  cnameTarget?: string;
  tls: boolean;
  actions?: ReactNode;
}) {
  const check = useDnsCheck(host);
  const status = check.data?.status ?? "checking";
  const [open, setOpen] = useState<boolean | null>(null);
  const expanded = open ?? (check.data !== undefined && status !== "connected");
  const state = STATE[status];
  const wildcard = host.startsWith("*.");
  const bodyId = `dns-${host}`;

  return (
    <div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-4 md:px-6">
        {wildcard ? (
          <code className="min-w-0 flex-1 text-[14px] break-all">{host}</code>
        ) : (
          <a href={`${tls ? "https" : "http"}://${host}`} target="_blank" rel="noreferrer" className="min-w-0 flex-1 break-all">
            {host}
          </a>
        )}
        <span className="inline-flex items-center gap-2 text-sm">
          <span className={cx("size-2 shrink-0 rounded-full", state.dot, state.pulse && "pulse")} aria-hidden />
          {state.label}
        </span>
        <div className="flex gap-1">
          <Button variant="tertiary" className="h-8" aria-expanded={expanded} aria-controls={bodyId} onClick={() => setOpen(!expanded)}>
            {expanded ? "Hide setup" : "Show setup"}
          </Button>
          {actions}
        </div>
      </div>
      {expanded && (
        <div id={bodyId} className="border-t border-line bg-subtle/60 px-5 py-6 md:px-6">
          <Steps host={host} check={check} cnameTarget={cnameTarget} tls={tls} />
        </div>
      )}
    </div>
  );
}

function Steps({
  host,
  check,
  cnameTarget,
  tls,
}: {
  host: string;
  check: ReturnType<typeof useDnsCheck>;
  cnameTarget?: string;
  tls: boolean;
}) {
  const data = check.data;
  const connected = data?.status === "connected";
  const ip = data?.expected.find((a) => !a.includes(":"));
  const record = recordFor(host, ip, cnameTarget);
  const zone = host.split(".").slice(-2).join(".");
  const wildcard = host.startsWith("*.");

  return (
    <ol className="grid max-w-3xl">
      <Step n={1} state={connected ? "done" : "current"} title={`Add this record where you manage DNS for ${zone}`}>
        <Record type={record.type} name={record.name} value={record.value} />
        <div className="mt-3 space-y-1 text-[13px] text-muted">
          {record.type === "CNAME" && ip && (
            <p>
              Can't add a CNAME? An <strong className="font-medium text-fg">A</strong> record pointing at <code>{ip}</code> works too.
            </p>
          )}
          {!record.value && <p>Use this server's public IPv4 address. justdeploy couldn't work it out from JD_PUBLIC_URL.</p>}
          <p>On Cloudflare, set the record to DNS only (grey cloud). The proxy hides this server from the check and from certificate issuing.</p>
        </div>
      </Step>

      <Step n={2} state={connected ? "done" : "current"} title={connected ? "DNS points here" : "Wait for DNS to reach this server"}>
        <CheckResult host={host} check={check} />
      </Step>

      <Step n={3} state={connected ? "current" : "todo"} title={wildcard ? "Open your services" : tls ? "Visit it over HTTPS" : "Visit it"} last>
        <p className="text-muted">
          {tls
            ? `${wildcard ? "Each service gets" : "It gets"} a Let's Encrypt certificate on its first visit after DNS connects, which takes a few seconds. Port 80 has to be open to the internet for it.`
            : "TLS is off on this install, so everything is served over plain HTTP."}
        </p>
        {connected && !wildcard && (
          <p className="mt-2">
            <a href={`${tls ? "https" : "http"}://${host}`} target="_blank" rel="noreferrer">
              Open {host}
            </a>
          </p>
        )}
      </Step>
    </ol>
  );
}

/** A numbered step on a rail. Done steps turn green, the current one is ink. */
function Step({
  n,
  state,
  title,
  last,
  children,
}: {
  n: number;
  state: "done" | "current" | "todo";
  title: string;
  last?: boolean;
  children: ReactNode;
}) {
  return (
    <li className="relative grid grid-cols-[28px_1fr] gap-x-4">
      {!last && <span className={cx("absolute top-8 bottom-1 left-[13.5px] w-px", state === "done" ? "bg-ok/50" : "bg-line-strong")} aria-hidden />}
      <span
        className={cx(
          "z-[1] flex size-7 items-center justify-center rounded-full text-[13px] font-semibold",
          state === "done" && "bg-ok text-white dark:text-page",
          state === "current" && "bg-fg text-on-fg",
          state === "todo" && "border border-line-strong bg-surface text-muted",
        )}
      >
        {state === "done" ? (
          <svg viewBox="0 0 16 16" className="size-3.5" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" aria-label="Done">
            <path d="M3.5 8.5l3 3 6-7" />
          </svg>
        ) : (
          n
        )}
      </span>
      <div className={cx("min-w-0", !last && "pb-7")}>
        <h4 className={cx("pt-1 font-semibold", state === "todo" && "text-muted")}>{title}</h4>
        <div className="mt-3">{children}</div>
      </div>
    </li>
  );
}

/** The DNS record laid out the way registrars ask for it, each value copyable. */
function Record({ type, name, value }: { type: string; name: string; value: string }) {
  const cells = [
    { label: "Type", value: type, copy: false },
    { label: "Name", value: name, copy: true },
    { label: "Value", value: value || "Your server's IPv4", copy: !!value },
  ];
  return (
    <div className="grid rounded-panel border border-line bg-surface md:grid-cols-[88px_minmax(0,0.8fr)_minmax(0,1.6fr)]">
      {cells.map((c, i) => (
        <div key={c.label} className={cx("min-w-0 px-4 py-3", i > 0 && "border-t border-line md:border-t-0 md:border-l")}>
          <div className="mb-1 text-[13px] text-muted">{c.label}</div>
          <div className="flex min-h-9 min-w-0 items-center justify-between gap-2">
            <code className={cx("min-w-0 text-[14px] break-all", c.label === "Value" && !value && "font-sans text-muted")}>{c.value}</code>
            {c.copy && <CopyButton value={c.value} />}
          </div>
        </div>
      ))}
    </div>
  );
}

function CheckResult({ host, check }: { host: string; check: ReturnType<typeof useDnsCheck> }) {
  const data = check.data;
  // Re-render so "checked 20s ago" stays honest between checks.
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 5_000);
    return () => clearInterval(t);
  }, []);
  const ago = Math.round((Date.now() - check.dataUpdatedAt) / 1000);
  const name = <code className="text-fg">{host}</code>;
  const ips = (list: string[]) => <code className="text-fg">{list.join(", ")}</code>;

  let message: ReactNode = "Looking up DNS…";
  if (check.error) message = check.error.message;
  else if (data?.status === "connected") message = <>{name} resolves to {ips(data.found)}, which is this server.</>;
  else if (data?.status === "missing")
    message = <>No record for {name} yet. New records usually show up within minutes; some providers take a few hours.</>;
  else if (data?.status === "elsewhere")
    message = (
      <>
        {name} resolves to {ips(data.found)}, but this server is {ips(data.expected)}. Change the record, and delete any older A or AAAA
        records for the same name.
      </>
    );
  else if (data?.status === "unknown")
    message = (
      <>
        {name} resolves to {ips(data.found)}. justdeploy can't tell whether that's this server: set JD_PUBLIC_URL to the address people reach
        it on.
      </>
    );

  return (
    <div className="rounded-panel border border-line bg-surface px-4 py-3">
      <p className="text-muted" aria-live="polite">
        {message}
      </p>
      {data?.status !== "connected" && (
        <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2">
          <Button variant="secondary" className="h-8" busy={check.isFetching} onClick={() => check.refetch()}>
            Check now
          </Button>
          {check.dataUpdatedAt > 0 && (
            <span className="text-[13px] text-muted">
              Checked {ago < 5 ? "just now" : `${ago}s ago`}. Checking again every {RECHECK_MS / 1000} seconds.
            </span>
          )}
        </div>
      )}
    </div>
  );
}
