import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import { Link, NavLink, Outlet, useLocation } from "react-router";
import { api, type Project } from "./api";
import { cx, Dot, Segmented } from "./ui";

/** The mark and the word as one lockup. Inverted on dark surfaces. */
export function Logo({ className }: { className?: string }) {
  return <img src="/logo-full.svg" alt="justdeploy" className={cx("block h-6 w-auto dark:invert", className)} />;
}

// ---------------------------------------------------------------- theme

type Theme = "system" | "light" | "dark";

function readTheme(): Theme {
  try {
    const stored = localStorage.getItem("jd-theme");
    return stored === "light" || stored === "dark" ? stored : "system";
  } catch {
    return "system";
  }
}

function applyTheme(theme: Theme) {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(readTheme);
  useEffect(() => {
    applyTheme(theme);
    try {
      if (theme === "system") localStorage.removeItem("jd-theme");
      else localStorage.setItem("jd-theme", theme);
    } catch {
      // Storage can be unavailable; the choice then lasts for this visit.
    }
    if (theme !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const follow = () => applyTheme("system");
    media.addEventListener("change", follow);
    return () => media.removeEventListener("change", follow);
  }, [theme]);
  return [theme, setTheme];
}

// ---------------------------------------------------------------- navigation

const icon = (d: ReactNode) => (
  <svg viewBox="0 0 20 20" className="size-[18px] shrink-0" fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    {d}
  </svg>
);

const NAV = [
  {
    to: "/projects",
    label: "Projects",
    icon: icon(<path d="M3 6.5 10 3l7 3.5-7 3.5-7-3.5Zm0 4 7 3.5 7-3.5M3 14.5 10 18l7-3.5" />),
  },
  {
    to: "/analytics",
    label: "Analytics",
    icon: icon(<path d="M3 16h14M5.5 13V9m4.5 4V5m4.5 8v-6" />),
  },
  {
    to: "/templates",
    label: "Templates",
    icon: icon(<path d="M3.5 3.5h5v5h-5zm8 0h5v5h-5zm-8 8h5v5h-5zm8 2.5h5M14 11.5v5" />),
  },
  {
    to: "/settings",
    label: "Settings",
    icon: icon(
      <path d="M10 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Zm6.5-2.5-.1-1.2 1.6-1.3-1.6-2.8-2 .7a6 6 0 0 0-2-1.2L12 2h-4l-.4 2.2a6 6 0 0 0-2 1.2l-2-.7L2 7.5l1.6 1.3L3.5 10l.1 1.2L2 12.5l1.6 2.8 2-.7a6 6 0 0 0 2 1.2L8 18h4l.4-2.2a6 6 0 0 0 2-1.2l2 .7 1.6-2.8-1.6-1.3.1-1.2Z" />,
    ),
  },
];

/** The worst state among a project's services, so trouble shows from anywhere. */
function projectStatus(project: Project): string {
  const states: string[] = project.services.map((s) => s.status);
  for (const s of ["crashed", "failed", "deploying", "running"]) {
    if (states.includes(s)) return s;
  }
  return states.length ? "stopped" : "idle";
}

function Sidebar({ onNavigate }: { onNavigate: () => void }) {
  const { pathname } = useLocation();
  const queryClient = useQueryClient();
  const [theme, setTheme] = useTheme();
  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => api.get<Project[]>("/projects"),
    refetchInterval: 10_000,
  });
  const me = useQuery({ queryKey: ["me"], queryFn: () => api.get<{ email: string }>("/auth/me") });
  const current = (to: string) => pathname.startsWith(to) || (to === "/projects" && pathname.startsWith("/services"));

  const signOut = async () => {
    await api.post("/auth/logout");
    await queryClient.invalidateQueries({ queryKey: ["auth-status"] });
  };

  return (
    <div className="flex h-full flex-col">
      <div className="hidden px-5 pt-6 pb-7 lg:block">
        <Link to="/projects" aria-label="justdeploy home" className="inline-block no-underline">
          <Logo />
        </Link>
      </div>

      <nav className="px-3 pt-3 lg:pt-0" aria-label="Sections">
        {NAV.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            onClick={onNavigate}
            className={cx(
              "flex items-center gap-3 rounded-control px-2.5 py-2 text-[15px] no-underline",
              current(item.to) ? "bg-subtle font-semibold text-fg" : "text-muted hover:bg-subtle hover:text-fg",
            )}
          >
            {item.icon}
            {item.label}
          </NavLink>
        ))}
      </nav>

      <div className="thin-scrollbar mt-8 min-h-0 flex-1 overflow-y-auto px-3">
        <p className="px-2.5 pb-2 text-[13px] font-medium text-muted">Your projects</p>
        <ul>
          {projects.data?.map((p) => {
            const active = pathname === `/projects/${p.id}`;
            return (
              <li key={p.id}>
                <Link
                  to={`/projects/${p.id}`}
                  onClick={onNavigate}
                  className={cx(
                    "flex items-center justify-between gap-3 rounded-control px-2.5 py-1.5 text-sm no-underline",
                    active ? "bg-subtle font-medium text-fg" : "text-muted hover:bg-subtle hover:text-fg",
                  )}
                >
                  <span className="truncate">{p.name}</span>
                  <Dot status={projectStatus(p)} />
                </Link>
              </li>
            );
          })}
          {projects.data?.length === 0 && <li className="px-2.5 text-sm text-faint">None yet</li>}
        </ul>
      </div>

      <div className="border-t border-line px-5 py-4">
        <Segmented<Theme>
          label="Theme"
          value={theme}
          onChange={setTheme}
          options={[
            { value: "system", label: "Auto" },
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
          ]}
        />
        <div className="mt-4 flex items-center justify-between gap-3 text-sm">
          <span className="truncate text-muted" title={me.data?.email}>
            {me.data?.email}
          </span>
          <button onClick={signOut} className="shrink-0 font-medium hover:underline">
            Sign out
          </button>
        </div>
      </div>
    </div>
  );
}

export default function Layout() {
  const [open, setOpen] = useState(false);
  const { pathname } = useLocation();
  useEffect(() => setOpen(false), [pathname]);

  return (
    <div className="min-h-screen lg:grid lg:grid-cols-[248px_minmax(0,1fr)]">
      {/* Phones and tablets: a top bar with a menu. */}
      <header className="sticky top-0 z-40 flex h-14 items-center justify-between border-b border-line bg-surface px-5 lg:hidden">
        <Link to="/projects" aria-label="justdeploy home" className="no-underline">
          <Logo className="h-5" />
        </Link>
        <button
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
          aria-controls="sidebar"
          className="rounded-control px-2.5 py-1.5 text-sm font-medium hover:bg-subtle"
        >
          {open ? "Close" : "Menu"}
        </button>
      </header>

      <aside
        id="sidebar"
        className={cx(
          "border-line bg-surface lg:sticky lg:top-0 lg:block lg:h-screen lg:border-r",
          open ? "fixed inset-x-0 top-14 bottom-0 z-30 block border-t" : "hidden",
        )}
      >
        <Sidebar onNavigate={() => setOpen(false)} />
      </aside>

      <main className="min-w-0">
        <div className="mx-auto max-w-[1240px]">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
