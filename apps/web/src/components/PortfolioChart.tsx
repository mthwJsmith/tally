/**
 * Portfolio history chart.
 *
 * Calls `/api/holdings/portfolio-history?range=...` which aggregates Yahoo
 * close prices across all holdings × quantity. Range selector toggles 1M / 3M / 1Y / 5Y.
 *
 * Used on both Investments page (full-width hero) and Dashboard (compact tile).
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
import {
  chartColors,
  chartAxisProps,
  chartTooltipStyle,
  chartGradients,
} from "@/lib/chart-theme";

type Range = "since" | "1d" | "5d" | "1mo" | "3mo" | "1y" | "5y";

/**
 * Each range picks its own interval. Yahoo's chart endpoint supports `1m` for `1d` (≈390
 * intraday bars), `5m` for `5d`, `1d` for monthly+, and weekly for multi-year.
 * Intraday tabs poll every 60s for a near-live feel.
 * "Since buy" is special: the backend derives the range from the earliest activity timestamp.
 */
const RANGES: {
  label: string;
  value: Range;
  interval: string;
  refetchMs: number | false;
}[] = [
  { label: "SINCE BUY", value: "since", interval: "1d", refetchMs: false },
  { label: "1D", value: "1d", interval: "1m", refetchMs: 60_000 },
  { label: "5D", value: "5d", interval: "5m", refetchMs: 60_000 },
  { label: "1M", value: "1mo", interval: "1d", refetchMs: false },
  { label: "3M", value: "3mo", interval: "1d", refetchMs: false },
  { label: "1Y", value: "1y", interval: "1d", refetchMs: false },
  { label: "5Y", value: "5y", interval: "1wk", refetchMs: false },
];

interface Props {
  /** Show the range buttons (Investments page) vs hide them (Dashboard tile). */
  showRangePicker?: boolean;
  /** Vertical height in px. */
  height?: number;
  /** Initial range. */
  defaultRange?: Range;
}

export function PortfolioChart({
  showRangePicker = true,
  height = 280,
  defaultRange = "since",
}: Props) {
  const [range, setRange] = useState<Range>(defaultRange);
  const cfg = RANGES.find((r) => r.value === range) ?? RANGES[0];
  const intraday = range === "1d" || range === "5d";
  const sinceBuy = range === "since";
  const series = useQuery({
    queryKey: ["portfolio-history", range],
    queryFn: () =>
      api.get<{
        points: { timestamp: number; value: number }[];
        cost_basis: number;
      }>(
        `/api/holdings/portfolio-history?range=${range}&interval=${cfg.interval}${
          sinceBuy ? "&since_buy=true" : ""
        }`,
      ),
    staleTime: intraday ? 30_000 : 5 * 60_000,
    refetchInterval: cfg.refetchMs,
  });

  const data = (series.data?.points ?? []).map((p) => {
    const d = new Date(p.timestamp * 1000);
    return {
      day: intraday
        ? d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })
        : d.toISOString().slice(0, 10),
      value: p.value,
    };
  });

  const costBasis = series.data?.cost_basis ?? 0;
  // "Since buy" performance = current value vs cost basis (what you actually paid).
  // For other ranges, fall back to range-start performance.
  const last = data[data.length - 1]?.value ?? 0;
  const first = sinceBuy && costBasis > 0 ? costBasis : data[0]?.value ?? 0;
  const delta = last - first;
  const deltaPct = first > 0 ? (delta / first) * 100 : 0;
  const positive = delta >= 0;
  const gradient = positive ? chartGradients.green : chartGradients.danger;

  return (
    <div className="space-y-3">
      {showRangePicker && (
        <div className="flex items-baseline justify-between">
          <div>
            <p className="text-[10px] uppercase tracking-widest text-mid">
              {sinceBuy ? "Total return (since you bought in)" : "Range performance"}
            </p>
            <p
              className={`mono font-extrabold text-2xl ${
                positive ? "text-green" : "text-danger"
              }`}
            >
              {positive ? "+" : "−"}£{Math.abs(delta).toFixed(2)}
              <span className="text-sm font-normal ml-2">
                ({positive ? "+" : "−"}
                {Math.abs(deltaPct).toFixed(2)}%)
              </span>
            </p>
          </div>
          <div className="inline-flex gap-1 bg-cream p-1 rounded-md">
            {RANGES.map((r) => (
              <button
                key={r.value}
                onClick={() => setRange(r.value)}
                className={`text-xs px-3 py-1 rounded font-semibold ${
                  range === r.value
                    ? "bg-ink text-cream"
                    : "text-mid hover:text-ink"
                }`}
              >
                {r.label}
              </button>
            ))}
          </div>
        </div>
      )}
      <div style={{ height }}>
        {series.isPending && (
          <div className="h-full flex items-center justify-center text-sm text-mid">
            Loading prices…
          </div>
        )}
        {series.isError && (
          <div className="h-full flex items-center justify-center text-sm text-danger">
            Could not fetch history.
          </div>
        )}
        {series.isSuccess && data.length === 0 && (
          <div className="h-full flex items-center justify-center text-sm text-mid">
            No history yet. Add a holding to see your portfolio.
          </div>
        )}
        {series.isSuccess && data.length > 0 && (
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={data} margin={{ top: 5, right: 5, left: 0, bottom: 0 }}>
              <defs>
                <linearGradient id={gradient.id} x1="0" y1="0" x2="0" y2="1">
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
              <YAxis {...chartAxisProps} width={55} />
              <Tooltip
                formatter={(v: number) => [
                  `£${v.toFixed(2)}`,
                  "Portfolio value",
                ]}
                contentStyle={chartTooltipStyle}
                cursor={{
                  stroke: positive ? chartColors.green : chartColors.danger,
                  strokeOpacity: 0.3,
                }}
              />
              <Area
                type="monotone"
                dataKey="value"
                stroke={positive ? chartColors.green : chartColors.danger}
                strokeWidth={2.5}
                fill={`url(#${gradient.id})`}
              />
              {costBasis > 0 && (
                <ReferenceLine
                  y={costBasis}
                  stroke={chartColors.muted}
                  strokeDasharray="4 4"
                  label={{
                    value: `Cost basis ${costBasis.toFixed(2)}`,
                    position: "insideTopLeft",
                    fontSize: 10,
                    fill: chartColors.muted,
                  }}
                />
              )}
            </AreaChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}
