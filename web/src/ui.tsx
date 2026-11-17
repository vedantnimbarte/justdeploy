// Building blocks. Greys carry structure, actions are solid ink, colour means
// state. Controls are 6px, panels 10px; only menus and dialogs float.

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

export const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

// ---------------------------------------------------------------- layout

/** A block of page content. `tone` is accepted for older call sites and ignored. */
export function Band({ className, children }: { tone?: string; className?: string; children: ReactNode }) {
  return <section className={cx("px-5 pb-10 md:px-10", className)}>{children}</section>;
}

/** The title row at the top of a page: wide title, one line of context, actions. */
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
    <header className="px-5 pt-8 pb-8 md:px-10 md:pt-10">
      {eyebrow && <nav className="mb-3 text-sm text-muted">{eyebrow}</nav>}
      <div className="flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between">
        <div className="min-w-0">
          <h1 className="title-wide text-[28px] break-words md:text-[34px]">{title}</h1>
          {lead && <p className="mt-2 max-w-[68ch] text-muted">{lead}</p>}
        </div>
        {actions && <div className="flex shrink-0 flex-wrap gap-2">{actions}</div>}
      </div>
      {children}
    </header>
  );
}

export function SectionTitle({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="mb-4 flex items-center justify-between gap-4">
      <h2 className="text-lg font-semibold">{children}</h2>
      {action}
    </div>
  );
}

/** A surface on the page. `tone` "soft" sits one step down for nested blocks. */
export function Card({ className, children, tone = "white" }: { className?: string; children: ReactNode; tone?: string }) {
  return (
    <div
      className={cx(
        "rounded-panel border border-line p-5 md:p-6",
        tone === "white" ? "bg-surface" : "bg-subtle",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function EmptyState({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="rounded-panel border border-dashed border-line-strong px-6 py-12 text-center">
      <h3 className="font-semibold">{title}</h3>
      {children && <div className="mx-auto mt-2 max-w-[56ch] text-muted">{children}</div>}
      {action && <div className="mt-6 flex flex-wrap justify-center gap-2">{action}</div>}
    </div>
  );
}

// ---------------------------------------------------------------- controls

type Variant = "primary" | "secondary" | "tertiary" | "inverse" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-fg text-on-fg hover:opacity-90",
  secondary: "border border-line-strong bg-surface text-fg hover:bg-subtle",
  tertiary: "text-fg hover:bg-subtle",
  inverse: "bg-surface text-fg hover:bg-subtle",
  danger: "border border-err/40 bg-surface text-err hover:bg-err/8",
};

const buttonBase =
  "inline-flex h-9 items-center justify-center gap-2 rounded-control px-3.5 text-sm font-medium whitespace-nowrap no-underline transition-[background-color,opacity] duration-100";

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
        buttonBase,
        VARIANTS[variant],
        "disabled:cursor-not-allowed disabled:opacity-45",
        className,
      )}
    >
      {busy && <span className="size-3 animate-spin rounded-full border-2 border-current border-t-transparent" aria-hidden />}
      {children}
    </button>
  );
}

export function ButtonLink({ to, variant = "primary", children }: { to: string; variant?: Variant; children: ReactNode }) {
  return (
    <Link to={to} className={cx(buttonBase, VARIANTS[variant])}>
      {children}
    </Link>
  );
}

const fieldClass =
  "w-full rounded-control border border-line-strong bg-surface px-3 py-2 text-[15px] text-fg placeholder:text-faint focus:border-focus focus:outline-none focus:ring-2 focus:ring-focus/25 disabled:bg-subtle disabled:text-faint";

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={cx(fieldClass, "h-9", props.className)} />;
}

export function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...props} className={cx(fieldClass, "h-9 pr-8", props.className)} />;
}

export function Textarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea {...props} className={cx(fieldClass, "font-mono text-[13px] leading-6", props.className)} />;
}

export function Field({ label, hint, children, className }: { label: string; hint?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <label className={cx("block", className)}>
      <span className="mb-1.5 block text-sm font-medium">{label}</span>
      {children}
      {hint && <span className="mt-1.5 block text-[13px] text-muted">{hint}</span>}
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
        className="mt-1 size-4 accent-[var(--jd-fg)]"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span className="block font-medium">{label}</span>
        {hint && <span className="block text-[13px] text-muted">{hint}</span>}
      </span>
    </label>
  );
}

export function ErrorText({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-sm text-err">
      {error instanceof Error ? error.message : String(error)}
    </p>
  );
}

// ---------------------------------------------------------------- status

const STATUS: Record<string, { dot: string; label: string; pulse?: boolean }> = {
  running: { dot: "bg-ok", label: "Running" },
  deploying: { dot: "bg-warn", label: "Deploying", pulse: true },
  building: { dot: "bg-warn", label: "Building", pulse: true },
  queued: { dot: "bg-warn", label: "Queued" },
  crashed: { dot: "bg-err", label: "Crashed" },
  failed: { dot: "bg-err", label: "Failed" },
  stopped: { dot: "bg-off", label: "Stopped" },
  idle: { dot: "bg-off", label: "Not deployed" },
  deleting: { dot: "bg-off", label: "Deleting", pulse: true },
  superseded: { dot: "bg-line-strong", label: "Replaced" },
  removed: { dot: "bg-line-strong", label: "Removed" },
  succeeded: { dot: "bg-ok", label: "Succeeded" },
};

export function statusDot(status: string): { dot: string; pulse?: boolean } {
  return STATUS[status] ?? { dot: "bg-off" };
}

export function Dot({ status, className }: { status: Status | string; className?: string }) {
  const s = statusDot(status);
  return <span className={cx("inline-block size-2 shrink-0 rounded-full", s.dot, s.pulse && "pulse", className)} aria-hidden />;
}

/** A dot always travels with a word, so state never rests on colour alone. */
export function StatusDot({ status, className }: { status: Status | string; className?: string }) {
  return (
    <span className={cx("inline-flex items-center gap-2 text-sm", className)}>
      <Dot status={status} />
      {STATUS[status]?.label ?? status}
    </span>
  );
}

export function Badge({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-control bg-subtle px-2 py-0.5 text-xs font-medium text-muted">
      {children}
    </span>
  );
}

// ---------------------------------------------------------------- copy, tabs, modal, toast

export function CopyButton({ value, label = "Copy" }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="secondary"
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
      <code className="flex h-9 min-w-0 flex-1 items-center truncate rounded-control border border-line bg-subtle px-3 text-[13px]">
        {shown ? value : "•".repeat(Math.min(value.length, 32))}
      </code>
      {secret && (
        <Button variant="secondary" onClick={() => setShown(!shown)}>
          {shown ? "Hide" : "Show"}
        </Button>
      )}
      <CopyButton value={value} />
    </div>
  );
}

/** Underlined tabs on the page background. */
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
    <nav className="no-scrollbar mb-8 overflow-x-auto border-b border-line px-5 md:px-10">
      <div className="flex gap-6" role="tablist">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            role="tab"
            aria-selected={tab.id === active}
            onClick={() => onChange(tab.id)}
            className={cx(
              "relative -mb-px shrink-0 border-b-2 py-3 text-sm font-medium",
              tab.id === active ? "border-fg text-fg" : "border-transparent text-muted hover:text-fg",
            )}
          >
            {tab.label}
          </button>
        ))}
      </div>
    </nav>
  );
}

/** A small segmented control, for ranges and modes. */
export function Segmented<T extends string | number>({
  options,
  value,
  onChange,
  label,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  label: string;
}) {
  return (
    <div className="inline-flex rounded-control border border-line bg-subtle p-0.5" role="group" aria-label={label}>
      {options.map((o) => (
        <button
          key={String(o.value)}
          onClick={() => onChange(o.value)}
          aria-pressed={o.value === value}
          className={cx(
            "rounded-[5px] px-3 py-1 text-sm font-medium",
            o.value === value ? "bg-surface text-fg shadow-[0_1px_2px_rgb(0_0_0/0.08)]" : "text-muted hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
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
      className="fixed inset-0 z-50 flex items-end justify-center bg-black/40 p-4 backdrop-blur-[2px] sm:items-center"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cx(
          "max-h-[90vh] w-full overflow-y-auto rounded-panel border border-line bg-surface p-6 shadow-[0_24px_48px_-12px_rgb(0_0_0/0.25)]",
          wide ? "max-w-2xl" : "max-w-lg",
        )}
      >
        <div className="mb-5 flex items-start justify-between gap-4">
          <h2 className="text-lg font-semibold">{title}</h2>
          <button onClick={onClose} className="-mt-1 -mr-1 rounded-control p-1 text-xl leading-none text-muted hover:bg-subtle hover:text-fg" aria-label="Close">
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** Asks for the name to be typed before something destructive happens. */
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
        <div className="text-muted">{consequence}</div>
        <Field label={`Type ${name} to confirm`}>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus />
        </Field>
        <div className="flex justify-end gap-2">
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
              "pointer-events-auto flex max-w-md items-center gap-2.5 rounded-panel border bg-surface px-4 py-3 text-sm shadow-[0_12px_32px_-8px_rgb(0_0_0/0.25)]",
              t.error ? "border-err/40" : "border-line",
            )}
          >
            <span className={cx("size-2 shrink-0 rounded-full", t.error ? "bg-err" : "bg-ok")} aria-hidden />
            {t.text}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export const useToast = () => useContext(ToastContext);
