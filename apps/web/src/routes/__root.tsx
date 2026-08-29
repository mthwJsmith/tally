import {
  createRootRouteWithContext,
  Outlet,
  Link,
  useNavigate,
  useRouterState,
} from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import type { MeResponse } from "@/types/api";
import {
  LayoutDashboard,
  ReceiptText,
  FolderTree,
  Wallet,
  CalendarClock,
  Landmark,
  Upload,
  Settings,
  LogOut,
  LineChart,
  AlarmClock,
  TrendingUp,
  PiggyBank,
  Menu,
  X,
} from "lucide-react";

export const Route = createRootRouteWithContext<{
  queryClient: QueryClient;
}>()({
  component: RootLayout,
});

const NAV_ITEMS = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard },
  { to: "/ahead", label: "Ahead", icon: TrendingUp },
  { to: "/transactions", label: "Transactions", icon: ReceiptText },
  { to: "/categories", label: "Categories", icon: FolderTree },
  { to: "/budgets", label: "Budgets", icon: Wallet },
  { to: "/bills", label: "Bills", icon: CalendarClock },
  { to: "/reminders", label: "Reminders", icon: AlarmClock },
  { to: "/banks", label: "Banks", icon: Landmark },
  { to: "/investments", label: "Investments", icon: LineChart },
  { to: "/retirement", label: "Retirement", icon: PiggyBank },
  { to: "/import", label: "Import CSV", icon: Upload },
  { to: "/settings", label: "Settings", icon: Settings },
] as const;

function RootLayout() {
  const navigate = useNavigate();
  const [menuOpen, setMenuOpen] = useState(false);
  const pathname = useRouterState({
    select: (s) => s.location.pathname,
  });
  const { data: me, isPending } = useQuery({
    queryKey: ["me"],
    queryFn: () => api.get<MeResponse>("/auth/me"),
  });

  useEffect(() => {
    // Dev-only: skip the auth redirects so any page can be previewed via `npm run dev`
    // without logging in. `import.meta.env.DEV` is false in production builds, so the
    // deployed app is unaffected.
    if (import.meta.env.DEV) return;
    if (isPending) return;
    const path = window.location.pathname;
    if (me?.setup_required && path !== "/setup") {
      navigate({ to: "/setup" });
    } else if (
      !me?.setup_required &&
      !me?.authenticated &&
      path !== "/login" &&
      path !== "/setup"
    ) {
      navigate({ to: "/login" });
    } else if (me?.authenticated && (path === "/login" || path === "/setup")) {
      navigate({ to: "/" });
    }
  }, [me, isPending, navigate]);

  // Close the mobile drawer whenever the route changes.
  useEffect(() => {
    setMenuOpen(false);
  }, [pathname]);

  if (isPending) {
    return (
      <div className="min-h-screen grid place-items-center text-mid text-sm">
        Loading
      </div>
    );
  }

  if (!me?.authenticated && !import.meta.env.DEV) {
    return (
      <div className="min-h-screen grid place-items-center px-6">
        <Outlet />
      </div>
    );
  }

  const signOut = async () => {
    await api.post("/auth/logout");
    window.location.href = "/login";
  };

  return (
    <div className="min-h-screen flex flex-col md:flex-row">
      {/* Mobile top bar */}
      <header className="md:hidden sticky top-0 z-40 flex items-center justify-between border-b border-thin bg-soft px-4 py-3">
        <Link to="/" className="text-xl font-extrabold tracking-tight">
          Tally
        </Link>
        <button
          onClick={() => setMenuOpen(true)}
          aria-label="Open menu"
          aria-expanded={menuOpen}
          className="p-2 -mr-2 text-mid hover:text-ink"
        >
          <Menu className="size-6" />
        </button>
      </header>

      {/* Mobile drawer */}
      {menuOpen && (
        <div className="md:hidden fixed inset-0 z-50">
          <div
            className="absolute inset-0 bg-black/40"
            onClick={() => setMenuOpen(false)}
          />
          <div className="absolute inset-y-0 left-0 w-72 max-w-[85vw] bg-soft border-r border-thin flex flex-col">
            <div className="flex items-center justify-between px-6 pt-6 pb-4">
              <Link to="/" className="text-2xl font-extrabold tracking-tight">
                Tally
              </Link>
              <button
                onClick={() => setMenuOpen(false)}
                aria-label="Close menu"
                className="p-2 -mr-2 text-mid hover:text-ink"
              >
                <X className="size-6" />
              </button>
            </div>
            <nav className="flex-1 overflow-y-auto px-3 space-y-0.5">
              {NAV_ITEMS.map(({ to, label, icon: Icon }) => (
                <NavItem key={to} to={to} icon={<Icon className="size-[18px]" />}>
                  {label}
                </NavItem>
              ))}
            </nav>
            <div className="px-4 py-4 border-t border-thin text-xs text-mid space-y-1.5">
              <p className="font-medium text-ink">{me?.username}</p>
              <button
                onClick={signOut}
                className="inline-flex items-center gap-1.5 hover:text-ink"
              >
                <LogOut className="size-3.5" /> Sign out
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Desktop sidebar */}
      <aside className="w-60 shrink-0 border-r border-thin bg-soft hidden md:flex flex-col">
        <Link
          to="/"
          className="px-6 pt-6 pb-7 text-2xl font-extrabold tracking-tight"
        >
          Tally
        </Link>
        <nav className="flex-1 px-3 space-y-0.5">
          {NAV_ITEMS.map(({ to, label, icon: Icon }) => (
            <NavItem key={to} to={to} icon={<Icon className="size-[18px]" />}>
              {label}
            </NavItem>
          ))}
        </nav>
        <div className="px-4 py-4 border-t border-thin text-xs text-mid space-y-1.5">
          <p className="font-medium text-ink">{me?.username}</p>
          <button
            onClick={signOut}
            className="inline-flex items-center gap-1.5 hover:text-ink"
          >
            <LogOut className="size-3.5" /> Sign out
          </button>
        </div>
      </aside>
      <main className="flex-1 min-w-0 overflow-x-hidden">
        <Outlet />
      </main>
    </div>
  );
}

function NavItem({
  to,
  icon,
  children,
}: {
  to: string;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Link
      to={to}
      className="flex items-center gap-2.5 px-3 py-2 text-sm text-mid hover:bg-cream hover:text-ink transition-colors [&.active]:bg-cream [&.active]:text-ink [&.active]:font-semibold"
      activeProps={{ className: "active" }}
    >
      {icon}
      {children}
    </Link>
  );
}
