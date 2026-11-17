import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";
import type { ServiceDetail } from "../../api";
import { Button, SectionTitle } from "../../ui";

/** A shell in the live container. Input goes up as binary frames; a text frame resizes. */
export default function Terminal({ service }: { service: ServiceDetail }) {
  const [session, setSession] = useState(0);
  const [open, setOpen] = useState(false);

  return (
    <>
      <SectionTitle
        action={
          open ? (
            <Button variant="secondary" onClick={() => setOpen(false)}>
              Close
            </Button>
          ) : (
            <Button
              disabled={service.status !== "running"}
              onClick={() => {
                setSession((s) => s + 1);
                setOpen(true);
              }}
            >
              Open shell
            </Button>
          )
        }
      >
        Terminal
      </SectionTitle>
      <p className="mb-5 max-w-3xl">
        A shell inside the running container, as its own user. Changes to its filesystem are lost on the
        next deploy unless they are on a volume. Every session is written to the audit log.
      </p>
      {open && <Session key={session} serviceId={service.id} />}
    </>
  );
}

function Session({ serviceId }: { serviceId: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [state, setState] = useState("Connecting…");

  useEffect(() => {
    const term = new XTerm({
      fontFamily: '"JetBrains Mono", ui-monospace, monospace',
      fontSize: 13,
      cursorBlink: true,
      theme: { background: "#0f1115", foreground: "#e3e6ea", cursor: "#f5c26b" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current!);
    fit.fit();

    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${scheme}://${window.location.host}/api/v1/services/${serviceId}/terminal`);
    ws.binaryType = "arraybuffer";
    const encoder = new TextEncoder();
    const resize = () => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ cols: term.cols, rows: term.rows }));
    };
    ws.onopen = () => {
      setState("Connected");
      resize();
      term.focus();
    };
    ws.onmessage = (e) => term.write(e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : String(e.data));
    ws.onclose = () => {
      setState("Closed");
      term.write("\r\n[session closed]\r\n");
    };
    const input = term.onData((data) => ws.readyState === WebSocket.OPEN && ws.send(encoder.encode(data)));
    const resized = term.onResize(resize);
    const onWindow = () => fit.fit();
    window.addEventListener("resize", onWindow);

    return () => {
      window.removeEventListener("resize", onWindow);
      input.dispose();
      resized.dispose();
      ws.onclose = null;
      ws.close();
      term.dispose();
    };
  }, [serviceId]);

  return (
    <div className="overflow-hidden rounded-panel bg-console">
      <div className="border-b border-white/10 px-4 py-3 text-sm text-white" role="status">
        {state}
      </div>
      <div ref={host} className="h-[28rem] p-3" />
    </div>
  );
}
