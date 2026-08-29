/**
 * Dashboard tile: net worth over time, from the daily `net_worth_history` rows the
 * backend records (cash + investments − debt, all GBP). The series starts the day
 * the feature shipped, so early on this shows a short line — it grows a point a day.
 */
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  Tooltip,
  XAxis,
  YAxis,
  CartesianGrid,
  ReferenceLine,
} from "recharts";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import {
  chartColors,
  chartAxisProps,
  chartTooltipStyle,
  chartGradients,
} from "@/lib/chart-theme";

type Point = {
  day: string;
  cash: number;
  debt: number;
  investments: number;
  pension: number;
  net: number;
};

export function NetWorthCard() {
  const history = useQuery({
    queryKey: ["net-worth-history"],
    queryFn: () => api.get<{ points: Point[] }>("/api/net-worth/history?days=365"),
    staleTime: 5 * 60_000,
  });
  // Pension is locked until ~57, so "without" is the spendable-reality view.
  const [withPension, setWithPension] = useState(true);

  const raw = history.data?.points ?? [];
  const points = raw.map((p) => ({
    ...p,
    value: withPension ? p.net : p.net - p.pension,
  }));
  const last = points[points.length - 1];
  const first = points[0];
  const delta = last && first ? last.value - first.value : 0;
  const improving = delta >= 0;
  const gradient = improving ? chartGradients.green : chartGradients.danger;

  return (
    <section className="card p-7 col-span-12 sm:col-span-6 lg:col-span-4 flex flex-col">
      <div className="flex items-start justify-between">
        <div>
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
            Net worth
          </p>
          <p
            className={`font-extrabold text-3xl mono tracking-tight ${
              (last?.value ?? 0) < 0 ? "text-danger" : ""
            }`}
          >
            {last ? formatMoney(Math.round(last.value * 100)) : "—"}
          </p>
          {last && first && first.day !== last.day && (
            <p
              className={`text-sm mono mt-1 font-semibold ${
                improving ? "text-green" : "text-danger"
              }`}
            >
              {improving ? "+" : "−"}
              {formatMoney(Math.round(Math.abs(delta) * 100))}{" "}
              <span className="text-mid font-normal">since {first.day.slice(5)}</span>
            </p>
          )}
        </div>
        <div className="inline-flex gap-1 bg-cream p-1 rounded-md">
          {(
            [
              [true, "Pension"],
              [false, "No pension"],
            ] as const
          ).map(([value, label]) => (
            <button
              key={label}
              onClick={() => setWithPension(value)}
              className={`text-[10px] px-2 py-1 rounded font-semibold ${
                withPension === value ? "bg-ink text-cream" : "text-mid hover:text-ink"
              }`}
              title={
                value
                  ? "Include the SIPP/pension pot"
                  : "Spendable view — excludes the pension (locked until ~57)"
              }
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      <div className="flex-1 min-h-[120px] mt-3 -mx-2">
        {points.length >= 2 ? (
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={points} margin={{ top: 5, right: 5, left: 0, bottom: 0 }}>
              <defs>
                <linearGradient id={`nw-${gradient.id}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={gradient.from} stopOpacity={0.35} />
                  <stop offset="100%" stopColor={gradient.to} stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid
                strokeDasharray="2 3"
                stroke={chartColors.border}
                vertical={false}
              />
              <XAxis
                dataKey="day"
                {...chartAxisProps}
                tickFormatter={(v: string) => v.slice(5)}
                minTickGap={32}
              />
              <YAxis {...chartAxisProps} width={50} />
              <Tooltip
                formatter={(v: number) => [
                  formatMoney(Math.round(v * 100)),
                  withPension ? "Net worth" : "Net worth (excl. pension)",
                ]}
                contentStyle={chartTooltipStyle}
              />
              <ReferenceLine y={0} stroke={chartColors.muted} strokeDasharray="4 4" />
              <Area
                type="monotone"
                dataKey="value"
                stroke={improving ? chartColors.green : chartColors.danger}
                strokeWidth={2.5}
                fill={`url(#nw-${gradient.id})`}
              />
            </AreaChart>
          </ResponsiveContainer>
        ) : (
          <p className="text-sm text-mid mt-4">
            Recording daily from today — the curve appears as history builds. Breakdown:
            cash {last ? formatMoney(Math.round(last.cash * 100)) : "—"} + investments{" "}
            {last
              ? formatMoney(
                  Math.round(
                    (last.investments - (withPension ? 0 : last.pension)) * 100
                  )
                )
              : "—"}{" "}
            − debt {last ? formatMoney(Math.round(last.debt * 100)) : "—"}
            {!withPension && " (pension excluded)"}.
          </p>
        )}
      </div>
    </section>
  );
}
