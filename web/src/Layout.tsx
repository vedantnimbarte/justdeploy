import { useQueryClient } from "@tanstack/react-query";
import { Link, NavLink, Outlet, useLocation } from "react-router";
import { api } from "./api";

const NAV = [
  { to: "/projects", label: "Projects" },
  { to: "/templates", label: "Templates" },
  { to: "/settings", label: "Settings" },
];

/** The mark and the word as one lockup, in ink. */
export function Logo() {
  return <img src="/logo-full.svg" alt="justdeploy" className="block h-7 w-auto md:h-8" />;
}

function Wordmark() {
  return (
    <Link to="/projects" aria-label="justdeploy home">
      <Logo />
    </Link>
  );
}

export default function Layout() {
  const queryClient = useQueryClient();
  const { pathname } = useLocation();
  // A service page belongs to Projects.
  const current = (to: string) => pathname.startsWith(to) || (to === "/projects" && pathname.startsWith("/services"));
  const signOut = async () => {
    await api.post("/auth/logout");
    await queryClient.invalidateQueries({ queryKey: ["auth-status"] });
  };

  return (
    <div className="flex min-h-screen flex-col">
      <header className="bg-canvas px-4 md:px-10 xl:px-[72px]">
        <div className="mx-auto flex h-16 max-w-[1600px] items-center justify-between md:h-[72px]">
          <Wordmark />
          <button onClick={signOut} className="text-sm font-medium text-ink hover:underline">
            Sign out
          </button>
        </div>
      </header>

      {/* Dark sub-navigation; the gold bar marks the current section. */}
      <nav className="no-scrollbar sticky top-0 z-40 overflow-x-auto bg-ink-deep px-4 md:px-10 xl:px-[72px]">
        <div className="mx-auto flex h-16 max-w-[1600px] gap-8">
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={`relative flex shrink-0 items-center text-base text-white! ${current(item.to) ? "font-semibold" : "opacity-85 hover:opacity-100"}`}
            >
              {() => (
                <>
                  {item.label}
                  {current(item.to) && <span className="absolute inset-x-0 bottom-0 h-[5px] bg-gold-soft" aria-hidden />}
                </>
              )}
            </NavLink>
          ))}
        </div>
      </nav>

      <main className="flex-1">
        <Outlet />
      </main>

      <footer className="bg-canvas px-4 py-8 text-xs leading-5 md:px-10 xl:px-[72px]">
        <div className="mx-auto flex max-w-[1600px] flex-wrap gap-x-6 gap-y-2">
          <span>justdeploy, MIT licensed</span>
          <a className="text-ink!" href="https://github.com/vedantnimbarte/justdeploy">
            Source on GitHub
          </a>
          <a className="text-ink!" href="https://github.com/vedantnimbarte/justdeploy#readme">
            Documentation
          </a>
        </div>
      </footer>
    </div>
  );
}
