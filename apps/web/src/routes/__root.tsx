import {
  createRootRouteWithContext,
  Outlet,
  Link,
  useNavigate,
} from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "@/lib/api";
import type { MeResponse } from "@/types/api";
import {
  LayoutDashboard,
  ReceiptText,
  FolderTree,
  Wand2,
  Wallet,
  CalendarClock,
  Landmark,
  Upload,
  Settings,
  LogOut,
  LineChart,
  AlarmClock,
  Tag,
} from "lucide-react";

export const Route = createRootRouteWithContext<{
  queryClient: QueryClient;
}>()({
  component: RootLayout,
});

function RootLayout() {
  const navigate = useNavigate();
  const { data: me, isPending } = useQuery({
    queryKey: ["me"],
    queryFn: () => api.get<MeResponse>("/auth/me"),
  });

  useEffect(() => {
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

  if (isPending) {
    return (
      <div className="min-h-screen grid place-items-center text-mid text-sm">
        Loading
      </div>
    );
  }

  if (!me?.authenticated) {
    return (
      <div className="min-h-screen grid place-items-center px-6">
        <Outlet />
      </div>
    );
  }

  return (
    <div className="min-h-screen flex">
      <aside className="w-60 shrink-0 border-r border-thin bg-soft hidden md:flex flex-col">
        <Link
          to="/"
          className="px-6 pt-6 pb-7 text-2xl font-extrabold tracking-tight"
        >
          Tally
        </Link>
        <nav className="flex-1 px-3 space-y-0.5">
          <NavItem to="/" icon={<LayoutDashboard className="size-[18px]" />}>
            Dashboard
          </NavItem>
          <NavItem
            to="/transactions"
            icon={<ReceiptText className="size-[18px]" />}
          >
            Transactions
          </NavItem>
          <NavItem
            to="/categories"
            icon={<FolderTree className="size-[18px]" />}
          >
            Categories
          </NavItem>
          <NavItem to="/rules" icon={<Wand2 className="size-[18px]" />}>
            Rules
          </NavItem>
          <NavItem to="/budgets" icon={<Wallet className="size-[18px]" />}>
            Budgets
          </NavItem>
          <NavItem
            to="/bills"
            icon={<CalendarClock className="size-[18px]" />}
          >
            Bills
          </NavItem>
          <NavItem to="/reminders" icon={<AlarmClock className="size-[18px]" />}>
            Reminders
          </NavItem>
          <NavItem to="/watchlist" icon={<Tag className="size-[18px]" />}>
            Watchlist
          </NavItem>
          <NavItem to="/banks" icon={<Landmark className="size-[18px]" />}>
            Banks
          </NavItem>
          <NavItem to="/investments" icon={<LineChart className="size-[18px]" />}>
            Investments
          </NavItem>
          <NavItem to="/import" icon={<Upload className="size-[18px]" />}>
            Import CSV
          </NavItem>
          <NavItem
            to="/settings"
            icon={<Settings className="size-[18px]" />}
          >
            Settings
          </NavItem>
        </nav>
        <div className="px-4 py-4 border-t border-thin text-xs text-mid space-y-1.5">
          <p className="font-medium text-ink">{me?.username}</p>
          <button
            onClick={async () => {
              await api.post("/auth/logout");
              window.location.href = "/login";
            }}
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
