import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatMoney, relativeTime } from "@/lib/format";
import type { Consent } from "@/types/api";
import { PortfolioChart } from "@/components/PortfolioChart";
import {
  chartColors,
  chartAxisProps,
  chartTooltipStyle,
  chartGradients,
} from "@/lib/chart-theme";
import type {
  Account,
  Bill,
  TransactionsListResponse,
} from "@/types/api";
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  Tooltip,
  XAxis,
  YAxis,
  CartesianGrid,
} from "recharts";
import {
  ArrowRight,
  ArrowDownRight,
  ArrowUpRight,
  LineChart as LineChartIcon,
  Plus,
  Wallet,
  RefreshCw,
} from "lucide-react";

export const Route = createFileRoute("/")({
  component: Dashboard,
});

function Dashboard() {
  const qc = useQueryClient();
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const syncStatus = useQuery({
    queryKey: ["sync-status"],
    queryFn: () => api.get<{ consents: Consent[] }>("/api/sync/status"),
    refetchInterval: 30_000,
  });
  const recent = useQuery({
    queryKey: ["txns", "recent"],
    queryFn: () =>
      api.get<TransactionsListResponse>("/api/transactions?limit=8"),
  });
  const upcoming = useQuery({
    queryKey: ["bills", "all"],
    queryFn: () => api.get<{ bills: Bill[] }>("/api/bills"),
  });
  const netWorth = useQuery({
    queryKey: ["holdings-net-worth"],
    queryFn: () =>
      api.get<{
        holdings_market_value: number;
        holdings_cost_basis: number;
        holdings_unrealised_gain: number;
      }>("/api/holdings/net-worth"),
  });

  const spendSeries = useQuery({
    queryKey: ["spend-series"],
    queryFn: async () => {
      const to = Math.floor(Date.now() / 1000);
      const from = to - 30 * 86_400;
      const txns = await api.get<TransactionsListResponse>(
        `/api/transactions?from=${from}&to=${to}&is_credit=false&limit=1000`
      );
      const byDay: Record<string, number> = {};
      for (let d = 0; d < 30; d++) {
        const ts = (from + d * 86_400) * 1000;
        byDay[new Date(ts).toISOString().slice(0, 10)] = 0;
      }
      for (const t of txns.transactions) {
        const day = new Date(t.timestamp * 1000).toISOString().slice(0, 10);
        if (byDay[day] !== undefined) byDay[day] += t.amount_cents;
      }
      return Object.entries(byDay).map(([day, cents]) => ({
        day: day.slice(5),
        spend: cents / 100,
      }));
    },
  });

  const totalSpend30d =
    spendSeries.data?.reduce((acc, d) => acc + d.spend, 0) ?? 0;
  const market = netWorth.data?.holdings_market_value ?? 0;
  const gain = netWorth.data?.holdings_unrealised_gain ?? 0;
  const cost = netWorth.data?.holdings_cost_basis ?? 0;
  const gainPct = cost > 0 ? (gain / cost) * 100 : 0;
  const portfolioPositive = gain >= 0;

  // Total cash across bank + card accounts. Cards subtract (they're debt) so the figure
  // is genuine net cash position. Skip accounts without a balance snapshot yet.
  const totalCashCents = (accounts.data?.accounts ?? []).reduce((acc, a) => {
    if (a.current_balance_cents == null) return acc;
    const sign = a.kind === "card" ? -1 : 1;
    return acc + sign * a.current_balance_cents;
  }, 0);
  const cashAccounts = (accounts.data?.accounts ?? []).filter(
    (a) => a.kind === "account" && a.current_balance_cents != null
  );
  const cardAccounts = (accounts.data?.accounts ?? []).filter(
    (a) => a.kind === "card" && a.current_balance_cents != null
  );

  // Most recent successful sync across all linked banks → "last updated" line.
  const lastSync = (syncStatus.data?.consents ?? []).reduce<number | null>(
    (acc, c) => (c.last_sync_at && (!acc || c.last_sync_at > acc) ? c.last_sync_at : acc),
    null
  );

  const sync = useMutation({
    mutationFn: () => api.post<{ triggered: number }>("/api/sync"),
    onSuccess: () => {
      // Banks take a few seconds to respond; refetch the views shortly after.
      setTimeout(() => {
        qc.invalidateQueries({ queryKey: ["accounts"] });
        qc.invalidateQueries({ queryKey: ["txns", "recent"] });
        qc.invalidateQueries({ queryKey: ["bills", "all"] });
        qc.invalidateQueries({ queryKey: ["spend-series"] });
        qc.invalidateQueries({ queryKey: ["sync-status"] });
      }, 4000);
    },
  });

  return (
    <div className="p-8 md:p-12 space-y-6">
      <header className="fade-in flex items-start justify-between gap-4">
        <div>
          <p className="text-xs uppercase tracking-widest text-mid mb-3">
            {new Date().toLocaleDateString("en-GB", {
              weekday: "long",
              day: "numeric",
              month: "long",
              year: "numeric",
            })}
          </p>
          <h1 className="text-5xl md:text-6xl mb-3">
            Your <em>money</em>, at a glance.
          </h1>
          <p className="text-mid">
            Tracking{" "}
            <span className="font-semibold text-ink">
              {accounts.data?.accounts.length ?? 0}
            </span>{" "}
            {accounts.data?.accounts.length === 1 ? "account" : "accounts"} across
            your linked banks.
          </p>
        </div>
        <div className="flex flex-col items-end gap-1.5 shrink-0">
          <button
            className="btn-primary"
            onClick={() => sync.mutate()}
            disabled={sync.isPending}
            title="Pull fresh data from your banks"
          >
            <RefreshCw
              className={`size-4 ${sync.isPending ? "animate-spin" : ""}`}
            />
            {sync.isPending ? "Syncing…" : "Sync now"}
          </button>
          <p className="text-[11px] text-mid mono">
            {sync.isSuccess && sync.isPending === false && !lastSync
              ? "sync triggered"
              : lastSync
                ? `updated ${relativeTime(lastSync)}`
                : "not synced yet"}
          </p>
        </div>
      </header>

      {/* Bento grid: 12 cols, varying spans */}
      <div className="grid grid-cols-12 auto-rows-[minmax(0,auto)] gap-4 fade-in-1">
        {/* Portfolio value — tall hero tile */}
        <section className="card p-7 col-span-12 lg:col-span-5 row-span-2 flex flex-col">
          <div className="flex items-start justify-between mb-3">
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
                Portfolio
              </p>
              <p className="font-extrabold text-4xl mono tracking-tight">
                {formatMoney(Math.round(market * 100))}
              </p>
              <p
                className={`text-sm mono mt-1 font-semibold ${
                  portfolioPositive ? "text-green" : "text-danger"
                }`}
              >
                {portfolioPositive ? "+" : "−"}
                {formatMoney(Math.round(Math.abs(gain) * 100))}{" "}
                <span className="text-mid font-normal">
                  ({portfolioPositive ? "+" : "−"}
                  {Math.abs(gainPct).toFixed(2)}%)
                </span>
              </p>
            </div>
            <Link
              to="/investments"
              className="btn-ghost text-xs"
              aria-label="Open investments"
            >
              <LineChartIcon className="size-4" />
            </Link>
          </div>
          <div className="flex-1 min-h-[200px]">
            <PortfolioChart showRangePicker={false} height={220} defaultRange="3mo" />
          </div>
        </section>

        {/* Spend tile */}
        <section className="card p-7 col-span-12 sm:col-span-6 lg:col-span-4">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
            Last 30 days · outgoing
          </p>
          <p className="font-extrabold text-3xl mono tracking-tight">
            {formatMoney(Math.round(totalSpend30d * 100))}
          </p>
          <div className="h-24 mt-3 -mx-2">
            {spendSeries.data && (
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart
                  data={spendSeries.data}
                  margin={{ top: 5, right: 5, left: 0, bottom: 0 }}
                >
                  <defs>
                    <linearGradient
                      id={chartGradients.green.id + "-mini"}
                      x1="0"
                      y1="0"
                      x2="0"
                      y2="1"
                    >
                      <stop
                        offset="0%"
                        stopColor={chartGradients.green.from}
                        stopOpacity={0.35}
                      />
                      <stop
                        offset="100%"
                        stopColor={chartGradients.green.to}
                        stopOpacity={0}
                      />
                    </linearGradient>
                  </defs>
                  <Tooltip
                    formatter={(v: number) => [`£${v.toFixed(2)}`, "spend"]}
                    contentStyle={chartTooltipStyle}
                    cursor={{ stroke: chartColors.green, strokeOpacity: 0.3 }}
                  />
                  <Area
                    type="monotone"
                    dataKey="spend"
                    stroke={chartColors.green}
                    strokeWidth={2}
                    fill={`url(#${chartGradients.green.id}-mini)`}
                  />
                </AreaChart>
              </ResponsiveContainer>
            )}
          </div>
        </section>

        {/* Accounts count tile */}
        <section className="card p-7 col-span-12 sm:col-span-6 lg:col-span-3 flex flex-col">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
            Linked banks
          </p>
          <p className="font-extrabold text-3xl mono tracking-tight">
            {accounts.data?.accounts.length ?? 0}
          </p>
          <p className="text-xs text-mid mt-1">
            {accounts.data?.accounts.length === 1 ? "account" : "accounts"}
          </p>
          <div className="flex-1" />
          <Link
            to="/banks"
            className="text-sm text-green font-semibold hover:text-green-deep inline-flex items-center gap-1 mt-3"
          >
            Manage <ArrowRight className="size-4" />
          </Link>
        </section>

        {/* Total cash tile */}
        <section className="card p-7 col-span-12 lg:col-span-7">
          <div className="flex items-baseline justify-between mb-4">
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
                Total cash
              </p>
              <p
                className={`font-extrabold text-3xl mono tracking-tight ${
                  totalCashCents < 0 ? "text-danger" : "text-ink"
                }`}
              >
                {formatMoney(totalCashCents)}
              </p>
              <p className="text-xs text-mid mt-1">
                {cashAccounts.length} cash · {cardAccounts.length} card
                {cardAccounts.length === 1 ? "" : "s"}
              </p>
            </div>
            <Link
              to="/banks"
              className="text-sm text-green font-semibold hover:text-green-deep inline-flex items-center gap-1"
            >
              Manage <ArrowRight className="size-4" />
            </Link>
          </div>

          {/* Per-account balance breakdown */}
          {accounts.data?.accounts.length ? (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 mt-2">
              {accounts.data.accounts.map((a) => {
                // Prefer the user's own labels: a per-account custom name if set, else the
                // bank nickname they chose at link time ("nationwide" → "Nationwide"). Fall
                // back to the bank's raw account name only if neither exists.
                const prettyBank = a.consent_nickname
                  ? a.consent_nickname
                      .split(/[-_\s]+/)
                      .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
                      .join(" ")
                  : null;
                const displayed =
                  a.custom_display_name ?? prettyBank ?? a.display_name;
                const subLabel =
                  displayed !== a.display_name ? a.display_name : null;
                const isCard = a.kind === "card";
                // Bank accounts: lead with the spendable "available" balance (what you can
                // actually spend right now), with the cleared/current balance as a secondary
                // line. Cards: lead with the current balance (the debt owed).
                const headlineCents = isCard
                  ? a.current_balance_cents
                  : a.available_balance_cents ?? a.current_balance_cents;
                const headlineNegative =
                  headlineCents != null &&
                  (isCard ? headlineCents > 0 : headlineCents < 0);
                const showLast4 = a.card_last4 && a.card_last4 !== "null";
                // Friendly type label. TrueLayer's raw account_type "TRANSACTION" means a
                // current account; cards from the /cards endpoint are credit cards.
                const typeLabel = isCard
                  ? "Credit card"
                  : a.account_type === "TRANSACTION"
                    ? "Current account"
                    : a.account_type === "SAVINGS"
                      ? "Savings"
                      : a.account_type
                        ? a.account_type
                            .replace(/_/g, " ")
                            .toLowerCase()
                            .replace(/^./, (c) => c.toUpperCase())
                        : "Account";
                return (
                  <div
                    key={a.id}
                    className="bg-cream/40 border border-thin rounded p-3.5"
                  >
                    <div className="flex items-baseline justify-between gap-2 mb-1.5">
                      <p className="text-[10px] uppercase tracking-widest text-mid">
                        {typeLabel}
                      </p>
                      {showLast4 && (
                        <p className="mono text-[10px] text-mid">
                          •••• {a.card_last4}
                        </p>
                      )}
                    </div>
                    <p className="font-extrabold tracking-tight text-sm truncate">
                      {displayed}
                    </p>
                    {subLabel && (
                      <p className="text-[10px] text-mid truncate">{subLabel}</p>
                    )}
                    {headlineCents != null ? (
                      <p
                        className={`mono font-extrabold text-lg mt-1.5 ${
                          headlineNegative ? "text-danger" : "text-ink"
                        }`}
                      >
                        {isCard && headlineCents > 0 ? "−" : ""}
                        {formatMoney(Math.abs(headlineCents), a.currency)}
                        {!isCard && (
                          <span className="text-[10px] text-mid font-normal ml-1">
                            available
                          </span>
                        )}
                      </p>
                    ) : (
                      <p className="mono text-mid text-xs mt-1.5">balance pending</p>
                    )}
                    {!isCard &&
                      a.current_balance_cents != null &&
                      a.current_balance_cents !== headlineCents && (
                        <p className="text-[10px] mono text-mid mt-0.5">
                          {formatMoney(a.current_balance_cents, a.currency)} balance
                        </p>
                      )}
                    {isCard && a.credit_limit_cents != null && (
                      <p className="text-[10px] mono text-mid mt-0.5">
                        of {formatMoney(a.credit_limit_cents, a.currency)} limit
                      </p>
                    )}
                    {isCard && a.payment_due_cents != null && a.payment_due_date && (
                      <p className="text-[10px] mono text-orange mt-1">
                        {formatMoney(a.payment_due_cents, a.currency)} due{" "}
                        {a.payment_due_date}
                      </p>
                    )}
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="card p-8 text-center mt-3">
              <Wallet className="size-6 text-mid mx-auto mb-3" />
              <p className="text-mid mb-4 text-sm">No accounts linked yet.</p>
              <Link to="/banks" className="btn-cta inline-flex">
                <Plus className="size-4" /> Link a bank
              </Link>
            </div>
          )}
        </section>

        {/* Direct debits & bills — no dates (DD dates can't be reliably forecast; see notes).
            Deduped by name so duplicate mandates don't show twice. */}
        <section className="card p-7 col-span-12 lg:col-span-5">
          <div className="flex items-baseline justify-between mb-4">
            <h2 className="text-lg font-semibold">
              Direct debits <em className="text-mid">&amp; bills</em>
            </h2>
            <Link
              to="/bills"
              className="text-xs text-mid hover:text-ink inline-flex items-center gap-1"
            >
              All <ArrowRight className="size-3.5" />
            </Link>
          </div>
          {(() => {
            const seen = new Set<string>();
            const deduped = (upcoming.data?.bills ?? []).filter((b) => {
              const key = b.name.replace(/\s*\(DD\)$/, "").toLowerCase();
              if (seen.has(key)) return false;
              seen.add(key);
              return true;
            });
            return deduped.length ? (
              <ul className="space-y-3">
                {deduped.slice(0, 6).map((b) => (
                  <li
                    key={b.id}
                    className="flex items-center justify-between text-sm pb-3 border-b border-thin last:border-0 last:pb-0"
                  >
                    <p className="font-medium truncate min-w-0">{b.name}</p>
                    <p className="mono font-semibold shrink-0 ml-2">
                      {b.expected_amount_max_cents > 0
                        ? formatMoney(b.expected_amount_max_cents, b.currency)
                        : "amount varies"}
                    </p>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-sm text-mid">No direct debits or bills yet.</p>
            );
          })()}
        </section>

        {/* Recent transactions — full width bottom */}
        <section className="card p-7 col-span-12 fade-in-3">
          <div className="flex items-baseline justify-between mb-5">
            <h2 className="text-2xl">
              Recent <em>activity</em>
            </h2>
            <Link
              to="/transactions"
              className="text-sm text-green font-semibold hover:text-green-deep inline-flex items-center gap-1"
            >
              All transactions <ArrowRight className="size-4" />
            </Link>
          </div>
          {recent.data?.transactions.length ? (
            <table className="w-full text-sm">
              <tbody>
                {recent.data.transactions.map((t) => (
                  <tr
                    key={t.id}
                    className="border-b border-thin last:border-0 hover:bg-cream/60 transition-colors"
                  >
                    <td className="py-3 pr-4">
                      {t.is_credit ? (
                        <ArrowDownRight className="size-4 text-green" />
                      ) : (
                        <ArrowUpRight className="size-4 text-mid" />
                      )}
                    </td>
                    <td className="py-3 pr-3 text-mid text-xs whitespace-nowrap mono">
                      {relativeTime(t.timestamp)}
                    </td>
                    <td className="py-3 pr-3">
                      <span className="font-medium">
                        {t.merchant_name || t.description}
                      </span>
                      {t.is_pending ? (
                        <span className="pill-orange ml-2">Pending</span>
                      ) : null}
                    </td>
                    <td
                      className={`py-3 text-right mono font-semibold ${
                        t.is_credit ? "text-green" : "text-ink"
                      }`}
                    >
                      {t.is_credit ? "+" : "−"}
                      {formatMoney(t.amount_cents, t.currency)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="text-sm text-mid">No transactions yet.</p>
          )}
        </section>
      </div>
    </div>
  );
}
