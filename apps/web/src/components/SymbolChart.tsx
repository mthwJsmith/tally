/**
 * Per-symbol price chart with buy/sell markers.
 *
 * Calls `/api/holdings/history?symbol=...&range=...&interval=...` and overlays the user's
 * own buy/sell activities as ReferenceDots — so you can see *where* you bought relative
 * to the live price (Ghostfolio-style).
 *
 * Range selector: 1D / 5D / 1M / 3M / 1Y / 5Y / MAX. Intraday tabs poll every 60s.
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
  ReferenceDot,
  ReferenceLine,
} from "recharts";
import { api } from "@/lib/api";
import {
  chartColors,
  chartAxisProps,
  chartTooltipStyle,
  chartGradients,
} from "@/lib/chart-theme";

type Range = "1d" | "5d" | "1mo" | "3mo" | "1y" | "5y" | "max";

const RANGES: {
  label: string;
  value: Range;
  interval: string;
  refetchMs: number | false;
}[] = [
  { label: "1D", value: "1d", interval: "1m", refetchMs: 60_000 },
  { label: "5D", value: "5d", interval: "5m", refetchMs: 60_000 },
  { label: "1M", value: "1mo", interval: "1d", refetchMs: false },
  { label: "3M", value: "3mo", interval: "1d", refetchMs: false },
  { label: "1Y", value: "1y", interval: "1d", refetchMs: false },
  { label: "5Y", value: "5y", interval: "1wk", refetchMs: false },
  { label: "MAX", value: "max", interval: "1mo", refetchMs: false },
];

export interface ActivityMarker {
  id: number;
  type: "BUY" | "SELL" | string;
  timestamp: number; // unix seconds
  price: number | null;
  quantity: number;
}

interface Props {
  symbol: string;
  /** Optional: cost basis line. */
  avgCost?: number | null;
  /** Optional: user's BUY/SELL events to overlay on the chart. */
  activities?: ActivityMarker[];
  height?: number;
  defaultRange?: Range;
}

export function SymbolChart({
  symbol,
  avgCost,
  activities = [],
  height = 360,
  defaultRange = "1y",
}: Props) {
  const [range, setRange] = useState<Range>(defaultRange);
  const cfg = RANGES.find((r) => r.value === range) ?? RANGES[4];
  const intraday = range === "1d" || range === "5d";

  const series = useQuery({
    queryKey: ["symbol-history", symbol, range],
    enabled: !!symbol,
    queryFn: () =>
      api.get<{ points: { timestamp: number; close: number }[] }>(
        `/api/holdings/history?symbol=${encodeURIComponent(
          symbol,
        )}&range=${range}&interval=${cfg.interval}`,
      ),
    staleTime: intraday ? 30_000 : 5 * 60_000,
    refetchInterval: cfg.refetchMs,
  });

  const points = series.data?.points ?? [];
  const data = points.map((p) => ({
    label: intraday
      ? new Date(p.timestamp * 1000).toLocaleTimeString("en-GB", {
          hour: "2-digit",
          minute: "2-digit",
        })
      : new Date(p.timestamp * 1000).toISOString().slice(0, 10),
    ts: p.timestamp,
    value: p.close,
  }));

  // For each activity, snap it to the closest chart point so the marker lands on the line.
  const markers = activities
    .filter((a) => a.type === "BUY" || a.type === "SELL")
    .map((a) => {
      let nearest = data[0];
      let bestDelta = Infinity;
      for (const d of data) {
        const delta = Math.abs(d.ts - a.timestamp);
        if (delta < bestDelta) {
          bestDelta = delta;
          nearest = d;
        }
      }
      if (!nearest) return null;
      return {
        ...a,
        x: nearest.label,
        // Use the user's actual fill price if they recorded one; else the close on that day.
        y: a.price ?? nearest.value,
      };
    })
    .filter((m): m is NonNullable<typeof m> => m != null);

  const first = data[0]?.value ?? 0;
  const last = data[data.length - 1]?.value ?? 0;
  const delta = last - first;
  const deltaPct = first > 0 ? (delta / first) * 100 : 0;
  const positive = delta >= 0;
  const gradient = positive ? chartGradients.green : chartGradients.danger;
  const lineColor = positive ? chartColors.green : chartColors.danger;

  return (
    <div className="space-y-3">
      <div className="flex items-baseline justify-between">
        <div>
          <p className="text-[10px] uppercase tracking-widest text-mid">
            {symbol} · {RANGES.find((r) => r.value === range)?.label}
          </p>
          {data.length > 0 && (
            <p
              className={`mono font-extrabold text-2xl ${
                positive ? "text-green" : "text-danger"
              }`}
            >
              {last.toFixed(2)}
              <span className="text-sm font-normal ml-2 text-mid">
                ({positive ? "+" : "−"}
                {Math.abs(deltaPct).toFixed(2)}%)
              </span>
            </p>
          )}
        </div>
        <div className="inline-flex gap-1 bg-cream p-1 rounded-md flex-wrap">
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
            No history available for this range.
          </div>
        )}
        {series.isSuccess && data.length > 0 && (
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={data} margin={{ top: 12, right: 12, left: 0, bottom: 0 }}>
              <defs>
                <linearGradient id={gradient.id + "-" + symbol} x1="0" y1="0" x2="0" y2="1">
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
                dataKey="label"
                {...chartAxisProps}
                tickFormatter={(v: string) =>
                  intraday ? v : v.slice(5)
                }
                minTickGap={32}
              />
              <YAxis {...chartAxisProps} domain={["auto", "auto"]} width={60} />
              <Tooltip
                formatter={(v: number) => [v.toFixed(2), symbol]}
                contentStyle={chartTooltipStyle}
                cursor={{ stroke: lineColor, strokeOpacity: 0.3 }}
              />
              <Area
                type="monotone"
                dataKey="value"
                stroke={lineColor}
                strokeWidth={2.5}
                fill={`url(#${gradient.id}-${symbol})`}
              />
              {avgCost != null && avgCost > 0 && (
                <ReferenceLine
                  y={avgCost}
                  stroke={chartColors.muted}
                  strokeDasharray="4 4"
                  label={{
                    value: `Avg cost ${avgCost.toFixed(2)}`,
                    position: "insideTopLeft",
                    fontSize: 10,
                    fill: chartColors.muted,
                  }}
                />
              )}
              {markers.map((m) => (
                <ReferenceDot
                  key={m.id}
                  x={m.x}
                  y={m.y}
                  r={5}
                  fill={m.type === "BUY" ? chartColors.green : chartColors.danger}
                  stroke="white"
                  strokeWidth={2}
                  label={{
                    value: `${m.type === "BUY" ? "B" : "S"} ${m.quantity}`,
                    position: "top",
                    fontSize: 10,
                    fill:
                      m.type === "BUY" ? chartColors.green : chartColors.danger,
                    fontWeight: 600,
                  }}
                />
              ))}
            </AreaChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}
