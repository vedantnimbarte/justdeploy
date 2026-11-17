// Building blocks. Bands are full-bleed and square; cards are 8px; buttons are 4px
// charcoal; depth comes from the neutral ladder and hairlines, never shadows.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from "react";
import { Link } from "react-router";
import type { Status } from "./api";

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

// ---------------------------------------------------------------- layout

type Tone = "white" | "soft" | "tint" | "warm" | "teal" | "deep";

const TONES: Record<Tone, string> = {
  white: "bg-canvas",
  soft: "bg-canvas-soft",
  tint: "bg-canvas-tint",
  warm: "bg-canvas-warm",
  teal: "bg-teal",
  deep: "bg-ink-deep text-white",
};

/** A full-width section on one rung of the ladder, content in the 1600px column. */
export function Band({
  tone = "warm",
  className,
  children,
}: {
  tone?: Tone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className={cx(TONES[tone], "px-4 py-8 md:px-10 md:py-12 xl:px-[72px]")}>
      <div className={cx("mx-auto max-w-[1600px]", className)}>{children}</div>
    </section>
  );
}

/** The opening band of a page: serif headline, the gold rule, a standfirst. */
export function PageHeader({
  title,
  eyebrow,
  lead,
  actions,
  children,
}: {
  title: ReactNode;
  eyebrow?: ReactNode;
  lead?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <Band tone="warm">
      <div className="flex flex-col gap-6 lg:flex-row lg:items-end lg:justify-between">
        <div className="min-w-0">
          {eyebrow && <div className="mb-3 text-sm">{eyebrow}</div>}
          <h1 className="font-serif text-[32px] font-normal break-words md:text-[40px]">{title}</h1>
          <div className="mt-[18px] h-1 w-10 bg-gold" aria-hidden />
          {lead && <div className="mt-5 max-w-3xl text-lg leading-[1.4]">{lead}</div>}
          {children}
        </div>
        {actions && <div className="flex flex-wrap gap-3">{actions}</div>}
      </div>
    </Band>
  );
}

export function SectionTitle({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="mb-[18px] flex items-center justify-between gap-4">
      <h2 className="text-xl font-extrabold md:text-2xl">{children}</h2>
      {action}
    </div>
  );
}

export function Card({
  className,
  children,
  tone = "white",
}: {
  className?: string;
  children: ReactNode;
  tone?: "white" | "warm" | "soft";
}) {
  const bg = tone === "white" ? "bg-canvas" : tone === "warm" ? "bg-canvas-warm" : "bg-canvas-soft";
  return <div className={cx(bg, "rounded-md border border-hairline p-6", className)}>{children}</div>;
}

export function EmptyState({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="rounded-md bg-canvas-soft p-8 text-center md:p-12">
      <h3 className="text-lg font-extrabold">{title}</h3>
      {children && <div className="mx-auto mt-2 max-w-xl">{children}</div>}
      {action && <div className="mt-6 flex justify-center gap-3">{action}</div>}
    </div>
  );
}

// ---------------------------------------------------------------- controls

type Variant = "primary" | "secondary" | "tertiary" | "inverse" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-primary text-white hover:bg-primary-pressed active:bg-primary-pressed",
  secondary: "border border-ink text-ink hover:bg-ink/5",
  tertiary: "bg-ink/8 text-ink hover:bg-ink/12",
  inverse: "bg-canvas text-ink hover:bg-canvas-tint",
  danger: "border border-error text-error hover:bg-error/5",
};

export function Button({
  variant = "primary",
  className,
  busy,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; busy?: boolean }) {
  return (
    <button
      type="button"
      {...rest}
      disabled={rest.disabled || busy}
      className={cx(
        "inline-flex items-center justify-center gap-2 rounded-xs px-4 py-[13px] text-sm leading-[1.2] font-medium whitespace-nowrap",
        "disabled:cursor-not-allowed disabled:border disabled:border-hairline disabled:bg-canvas-soft disabled:text-disabled",
        VARIANTS[variant],
        className,
      )}
    >
      {busy ? "Working…" : children}
    </button>
  );
}

export function ButtonLink({ to, variant = "primary", children }: { to: string; variant?: Variant; children: ReactNode }) {
  return (
    <Link
      to={to}
      className={cx(
        "inline-flex items-center justify-center rounded-xs px-4 py-[13px] text-sm leading-[1.2] font-medium whitespace-nowrap",
        VARIANTS[variant],
        variant === "primary" ? "!text-white" : "!text-ink",
      )}
    >
      {children}
    </Link>
  );
}

const fieldClass =
  "w-full rounded-sm border border-field bg-canvas px-[18px] py-3 text-base text-ink placeholder:text-disabled disabled:bg-canvas-soft disabled:text-disabled";

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={cx(fieldClass, props.className)} />;
}

export function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...props} className={cx(fieldClass, "pr-10", props.className)} />;
}

export function Textarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea {...props} className={cx(fieldClass, "font-mono text-sm", props.className)} />;
}

export function Field({
  label,
  hint,
  children,
  className,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={cx("block", className)}>
      <span className="mb-2 block text-sm font-semibold">{label}</span>
      {children}
      {hint && <span className="mt-1.5 block text-sm text-ink/70">{hint}</span>}
    </label>
  );
}

export function Checkbox({
  label,
  hint,
  checked,
  onChange,
  disabled,
}: {
  label: string;
  hint?: ReactNode;
  checked: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3">
      <input
        type="checkbox"
        className="mt-1 size-4 accent-primary"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span className="block font-semibold">{label}</span>
        {hint && <span className="block text-sm text-ink/70">{hint}</span>}
      </span>
    </label>
  );
}

export function ErrorText({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-sm text-error">
      {error instanceof Error ? error.message : String(error)}
    </p>
  );
}

// ---------------------------------------------------------------- status

const STATUS: Record<string, { dot: string; label: string }> = {
  running: { dot: "bg-forest", label: "Running" },
  deploying: { dot: "bg-gold animate-pulse", label: "Deploying" },
  building: { dot: "bg-gold animate-pulse", label: "Building" },
  queued: { dot: "bg-gold", label: "Queued" },
  crashed: { dot: "bg-error", label: "Crashed" },
  failed: { dot: "bg-error", label: "Failed" },
  stopped: { dot: "bg-disabled", label: "Stopped" },
  idle: { dot: "bg-disabled", label: "Not deployed" },
  deleting: { dot: "bg-disabled animate-pulse", label: "Deleting" },
  superseded: { dot: "bg-hairline border border-disabled", label: "Superseded" },
  removed: { dot: "bg-hairline border border-disabled", label: "Removed" },
  succeeded: { dot: "bg-forest", label: "Succeeded" },
};

export function StatusDot({ status, className }: { status: Status | string; className?: string }) {
  const s = STATUS[status] ?? { dot: "bg-disabled", label: status };
  return (
    <span className={cx("inline-flex items-center gap-2 text-sm", className)}>
      <span className={cx("size-2 shrink-0 rounded-full", s.dot)} aria-hidden />
      {s.label}
    </span>
  );
}

export function Badge({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-xs border border-hairline bg-canvas-soft px-2 py-0.5 text-xs font-medium">
      {children}
    </span>
  );
}

// ---------------------------------------------------------------- copy, tabs, modal, toast

export function CopyButton({ value, label = "Copy" }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="tertiary"
      className="!py-2"
      onClick={async () => {
        await navigator.clipboard.writeText(value);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "Copied" : label}
    </Button>
  );
}

export function CodeLine({ value, secret }: { value: string; secret?: boolean }) {
  const [shown, setShown] = useState(!secret);
  return (
    <div className="flex min-w-0 items-center gap-2">
      <code className="min-w-0 flex-1 truncate rounded-sm border border-hairline bg-canvas-soft px-3 py-2 text-sm">
        {shown ? value : "•".repeat(Math.min(value.length, 32))}
      </code>
      {secret && (
        <Button variant="tertiary" className="!py-2" onClick={() => setShown(!shown)}>
          {shown ? "Hide" : "Show"}
        </Button>
      )}
      <CopyButton value={value} />
    </div>
  );
}

/** Light sub-navigation: an ink underline marks the active item. */
export function Tabs<T extends string>({
  tabs,
  active,
  onChange,
}: {
  tabs: { id: T; label: string }[];
  active: T;
  onChange: (id: T) => void;
}) {
  return (
    <nav className="no-scrollbar overflow-x-auto border-b border-hairline bg-canvas px-4 md:px-10 xl:px-[72px]">
      <div className="mx-auto flex h-16 max-w-[1600px] gap-6" role="tablist">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            role="tab"
            aria-selected={tab.id === active}
            onClick={() => onChange(tab.id)}
            className={cx(
              "relative shrink-0 text-base",
              tab.id === active ? "font-semibold" : "text-ink/80 hover:text-ink",
            )}
          >
            {tab.label}
            {tab.id === active && <span className="absolute inset-x-0 bottom-0 h-[3px] bg-ink" aria-hidden />}
          </button>
        ))}
      </div>
    </nav>
  );
}

export function Modal({
  open,
  title,
  onClose,
  children,
  wide,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-end justify-center bg-ink/40 p-4 sm:items-center"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cx(
          "max-h-[90vh] w-full overflow-y-auto rounded-md border border-hairline bg-canvas p-6",
          wide ? "max-w-2xl" : "max-w-lg",
        )}
      >
        <div className="mb-5 flex items-start justify-between gap-4">
          <h2 className="text-xl font-extrabold">{title}</h2>
          <button onClick={onClose} className="text-2xl leading-none text-ink/70 hover:text-ink" aria-label="Close">
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** Asks the user to type a name before something destructive happens. */
export function ConfirmDelete({
  open,
  name,
  what,
  consequence,
  onClose,
  onConfirm,
  busy,
}: {
  open: boolean;
  name: string;
  what: string;
  consequence: ReactNode;
  onClose: () => void;
  onConfirm: () => void;
  busy?: boolean;
}) {
  const [typed, setTyped] = useState("");
  return (
    <Modal open={open} title={`Delete ${what}`} onClose={onClose}>
      <div className="space-y-4">
        <div>{consequence}</div>
        <Field label={`Type ${name} to confirm`}>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus />
        </Field>
        <div className="flex justify-end gap-3">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="danger" disabled={typed !== name} busy={busy} onClick={onConfirm}>
            Delete {what}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

type Toast = { id: number; text: string; error?: boolean };
const ToastContext = createContext<(text: string, error?: boolean) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((text: string, error?: boolean) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t, { id, text, error }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 4000);
  }, []);
  return (
    <ToastContext.Provider value={push}>
      {children}
      <div className="pointer-events-none fixed right-4 bottom-4 left-4 z-[60] flex flex-col items-end gap-2" aria-live="polite">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={cx(
              "pointer-events-auto max-w-md rounded-md border bg-canvas px-5 py-3",
              t.error ? "border-error text-error" : "border-hairline",
            )}
          >
            {t.text}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export const useToast = () => useContext(ToastContext);
