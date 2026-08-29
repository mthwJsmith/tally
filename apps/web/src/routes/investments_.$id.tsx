/**
 * Per-holding detail page.
 *
 * Shows:
 *   - Big price chart for the symbol (1D…MAX, intraday auto-refreshes)
 *   - Avg cost basis line + buy/sell markers overlaid on the chart
 *   - Activities log (date, type, qty, price, fee, notes)
 *   - "Record buy/sell" form
 *
 * The holding row's quantity + avg cost are read from the existing /api/holdings list
 * (no per-id GET endpoint yet; we filter by id client-side — cheap, the list is small).
 */
import { createFileRoute, Link, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ArrowLeft, Check, Pencil, Plus, Trash2, X } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate, formatMoney } from "@/lib/format";
import { SymbolChart, type ActivityMarker } from "@/components/SymbolChart";

/** unix-seconds → value for a <input type="datetime-local"> in local time. */
function tsToLocalInput(ts: number): string {
  const d = new Date(ts * 1000);
  const off = d.getTimezoneOffset();
  return new Date(d.getTime() - off * 60_000).toISOString().slice(0, 16);
}

export const Route = createFileRoute("/investments_/$id")({
  component: HoldingDetail,
});

type EnrichedHolding = {
  holding: {
    id: number;
    broker_id: number;
    symbol: string;
    asset_class: string;
    quantity: number;
    avg_cost_per_unit: number | null;
    currency: string;
    name: string | null;
  };
  current_price: number | null;
  market_value: number | null;
  cost_basis: number | null;
  gain: number | null;
  gain_pct: number | null;
  company_name: string | null;
};

type Activity = {
  id: number;
  holding_id: number;
  activity_type: string;
  timestamp: number;
  quantity: number;
  price_per_unit: number | null;
  fee: number;
  currency: string;
  notes: string | null;
};

function HoldingDetail() {
  const { id } = useParams({ from: "/investments_/$id" });
  const holdingId = parseInt(id, 10);

  const holdings = useQuery({
    queryKey: ["holdings"],
    queryFn: () => api.get<{ holdings: EnrichedHolding[] }>("/api/holdings"),
  });
  const activities = useQuery({
    queryKey: ["activities", holdingId],
    queryFn: () =>
      api.get<{ activities: Activity[] }>(
        `/api/holdings/${holdingId}/activities`,
      ),
  });

  const h = holdings.data?.holdings.find((x) => x.holding.id === holdingId);
  const markers: ActivityMarker[] = (activities.data?.activities ?? []).map(
    (a) => ({
      id: a.id,
      type: a.activity_type,
      timestamp: a.timestamp,
      price: a.price_per_unit,
      quantity: a.quantity,
    }),
  );

  const totalBuyQty = (activities.data?.activities ?? [])
    .filter((a) => a.activity_type === "BUY")
    .reduce((s, a) => s + a.quantity, 0);
  const totalSellQty = (activities.data?.activities ?? [])
    .filter((a) => a.activity_type === "SELL")
    .reduce((s, a) => s + a.quantity, 0);
  const totalDividends = (activities.data?.activities ?? [])
    .filter((a) => a.activity_type === "DIVIDEND")
    .reduce((s, a) => s + (a.price_per_unit ?? 0) * a.quantity, 0);

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1280px]">
      <header className="fade-in">
        <Link
          to="/investments"
          className="text-xs uppercase tracking-widest text-mid hover:text-ink inline-flex items-center gap-1 mb-3"
        >
          <ArrowLeft className="size-3.5" /> Back to investments
        </Link>
        <h1 className="text-4xl mb-1">
          <span className="mono">{h?.holding.symbol ?? "…"}</span>
          {h?.company_name && (
            <span className="text-mid text-2xl ml-3 font-normal">
              · {h.company_name}
            </span>
          )}
        </h1>
        {h && (
          <div className="grid sm:grid-cols-4 gap-6 mt-6">
            <Stat
              label="Quantity"
              value={h.holding.quantity.toString()}
              mono
            />
            <Stat
              label="Avg cost"
              value={
                h.holding.avg_cost_per_unit != null
                  ? h.holding.avg_cost_per_unit.toFixed(2)
                  : "—"
              }
              mono
            />
            <Stat
              label="Current price"
              value={
                h.current_price != null ? h.current_price.toFixed(2) : "—"
              }
              mono
            />
            <Stat
              label="Market value"
              value={
                h.market_value != null
                  ? formatMoney(
                      Math.round(h.market_value * 100),
                      h.holding.currency,
                    )
                  : "—"
              }
              mono
              accent={(h.gain ?? 0) >= 0 ? "green" : "danger"}
              hint={
                h.gain != null && h.gain_pct != null
                  ? `${(h.gain ?? 0) >= 0 ? "+" : "−"}${formatMoney(
                      Math.round(Math.abs(h.gain) * 100),
                      h.holding.currency,
                    )} (${h.gain_pct.toFixed(2)}%)`
                  : undefined
              }
            />
          </div>
        )}
      </header>

      {/* Big price chart */}
      <section className="card p-7 fade-in-1">
        {h ? (
          <SymbolChart
            symbol={h.holding.symbol}
            avgCost={h.holding.avg_cost_per_unit ?? undefined}
            activities={markers}
            height={420}
            defaultRange="1y"
          />
        ) : (
          <div className="h-[420px] flex items-center justify-center text-mid">
            Loading…
          </div>
        )}
      </section>

      {/* Quantity + avg cost are computed from the activity log — no manual edit. */}

      {/* Activity summary stats */}
      <section className="grid sm:grid-cols-3 gap-3 fade-in-3">
        <SummaryCard label="Total bought" value={totalBuyQty} suffix="units" />
        <SummaryCard label="Total sold" value={totalSellQty} suffix="units" />
        <SummaryCard
          label="Dividends received"
          value={totalDividends}
          prefix="£"
          decimals={2}
        />
      </section>

      {/* Activities */}
      <section className="fade-in-4">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold">Activity log</h2>
        </div>
        <AddActivityForm
          holdingId={holdingId}
          symbol={h?.holding.symbol ?? ""}
          startOpen={(activities.data?.activities.length ?? 0) === 0}
        />
        <div className="card overflow-hidden mt-3">
          <table className="w-full text-sm">
            <thead className="bg-cream">
              <tr className="text-[11px] uppercase tracking-widest text-mid">
                <th className="text-left px-4 py-2.5">Date</th>
                <th className="text-left px-4 py-2.5">Type</th>
                <th className="text-right px-4 py-2.5">Quantity</th>
                <th className="text-right px-4 py-2.5">Price</th>
                <th className="text-right px-4 py-2.5">Fee</th>
                <th className="text-left px-4 py-2.5">Notes</th>
                <th className="px-4 py-2.5"></th>
              </tr>
            </thead>
            <tbody>
              {(activities.data?.activities ?? []).map((a) => (
                <ActivityRow key={a.id} a={a} holdingId={holdingId} />
              ))}
              {!activities.data?.activities.length && (
                <tr>
                  <td colSpan={7} className="px-4 py-8 text-center text-mid">
                    No activities recorded yet.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}

function Stat({
  label,
  value,
  mono,
  accent,
  hint,
}: {
  label: string;
  value: string;
  mono?: boolean;
  accent?: "green" | "danger";
  hint?: string;
}) {
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-mid mb-1.5">
        {label}
      </p>
      <p
        className={`font-extrabold text-2xl tracking-tight ${
          mono ? "mono" : ""
        } ${accent === "green" ? "text-green" : ""} ${
          accent === "danger" ? "text-danger" : ""
        }`}
      >
        {value}
      </p>
      {hint && (
        <p
          className={`text-[11px] mono mt-1 ${
            accent === "green"
              ? "text-green"
              : accent === "danger"
                ? "text-danger"
                : "text-mid"
          }`}
        >
          {hint}
        </p>
      )}
    </div>
  );
}

function SummaryCard({
  label,
  value,
  suffix,
  prefix = "",
  decimals = 0,
}: {
  label: string;
  value: number;
  suffix?: string;
  prefix?: string;
  decimals?: number;
}) {
  return (
    <div className="card p-5">
      <p className="text-[10px] uppercase tracking-widest text-mid mb-1.5">
        {label}
      </p>
      <p className="font-extrabold text-2xl mono tracking-tight">
        {prefix}
        {value.toFixed(decimals)}
        {suffix ? <span className="text-sm font-normal ml-1">{suffix}</span> : null}
      </p>
    </div>
  );
}

function ActivityRow({ a, holdingId }: { a: Activity; holdingId: number }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["activities", holdingId] });
    qc.invalidateQueries({ queryKey: ["holdings"] });
    qc.invalidateQueries({ queryKey: ["holdings-net-worth"] });
    qc.invalidateQueries({ queryKey: ["portfolio-history"] });
  };
  const del = useMutation({
    mutationFn: () => api.delete(`/api/activities/${a.id}`),
    onSuccess: invalidate,
  });

  // Editable local state, seeded from the row.
  const [type, setType] = useState(a.activity_type);
  const [date, setDate] = useState(() => tsToLocalInput(a.timestamp));
  const [qty, setQty] = useState(String(a.quantity));
  const [price, setPrice] = useState(
    a.price_per_unit != null ? String(a.price_per_unit) : "",
  );
  const [fee, setFee] = useState(a.fee > 0 ? String(a.fee) : "");
  const [notes, setNotes] = useState(a.notes ?? "");

  const save = useMutation({
    mutationFn: () =>
      api.patch(`/api/activities/${a.id}`, {
        activity_type: type,
        timestamp: Math.floor(new Date(date).getTime() / 1000),
        quantity: parseFloat(qty || "0"),
        price_per_unit: price ? parseFloat(price) : null,
        fee: fee ? parseFloat(fee) : 0,
        notes: notes || null,
      }),
    onSuccess: () => {
      invalidate();
      setEditing(false);
    },
  });

  const color =
    a.activity_type === "BUY"
      ? "text-green"
      : a.activity_type === "SELL"
        ? "text-danger"
        : "text-mid";

  if (editing) {
    return (
      <tr className="border-t border-thin bg-cream/60">
        <td className="px-2 py-2">
          <input
            type="datetime-local"
            className="input mono text-xs"
            value={date}
            onChange={(e) => setDate(e.target.value)}
          />
        </td>
        <td className="px-2 py-2">
          <select
            className="input text-xs"
            value={type}
            onChange={(e) => setType(e.target.value)}
          >
            {["BUY", "SELL", "DIVIDEND", "SPLIT", "FEE", "INTEREST"].map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </td>
        <td className="px-2 py-2">
          <input
            className="input mono text-xs text-right"
            inputMode="decimal"
            value={qty}
            onChange={(e) => setQty(e.target.value)}
          />
        </td>
        <td className="px-2 py-2">
          <input
            className="input mono text-xs text-right"
            inputMode="decimal"
            value={price}
            onChange={(e) => setPrice(e.target.value)}
          />
        </td>
        <td className="px-2 py-2">
          <input
            className="input mono text-xs text-right"
            inputMode="decimal"
            value={fee}
            onChange={(e) => setFee(e.target.value)}
          />
        </td>
        <td className="px-2 py-2">
          <input
            className="input text-xs"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
          />
        </td>
        <td className="px-4 py-3 text-right whitespace-nowrap">
          <button
            className="btn-ghost text-xs text-green"
            onClick={() => save.mutate()}
            disabled={save.isPending}
            title="Save"
          >
            <Check className="size-3.5" />
          </button>
          <button
            className="btn-ghost text-xs"
            onClick={() => setEditing(false)}
            title="Cancel"
          >
            <X className="size-3.5" />
          </button>
        </td>
      </tr>
    );
  }

  return (
    <tr className="border-t border-thin hover:bg-cream/60">
      <td className="px-4 py-3 mono whitespace-nowrap">
        {formatDate(a.timestamp)}
      </td>
      <td className={`px-4 py-3 font-semibold ${color}`}>
        {a.activity_type}
      </td>
      <td className="px-4 py-3 mono text-right">{a.quantity}</td>
      <td className="px-4 py-3 mono text-right">
        {a.price_per_unit != null ? a.price_per_unit.toFixed(2) : "—"}
      </td>
      <td className="px-4 py-3 mono text-right text-mid">
        {a.fee > 0 ? a.fee.toFixed(2) : "—"}
      </td>
      <td className="px-4 py-3 text-mid text-xs truncate max-w-[260px]">
        {a.notes || ""}
      </td>
      <td className="px-4 py-3 text-right whitespace-nowrap">
        <button
          className="btn-ghost text-xs"
          onClick={() => setEditing(true)}
          title="Edit"
        >
          <Pencil className="size-3.5" />
        </button>
        <button
          className="btn-ghost text-xs"
          onClick={() => del.mutate()}
          title="Delete"
        >
          <Trash2 className="size-3.5" />
        </button>
      </td>
    </tr>
  );
}

function AddActivityForm({
  holdingId,
  symbol,
  startOpen = false,
}: {
  holdingId: number;
  symbol: string;
  startOpen?: boolean;
}) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(startOpen);
  const [type, setType] = useState<
    "BUY" | "SELL" | "DIVIDEND" | "SPLIT" | "FEE" | "INTEREST"
  >("BUY");
  const [date, setDate] = useState(() => {
    const now = new Date();
    const off = now.getTimezoneOffset();
    return new Date(now.getTime() - off * 60_000).toISOString().slice(0, 16);
  });
  const [qty, setQty] = useState("");
  const [price, setPrice] = useState("");
  const [fee, setFee] = useState("");
  const [notes, setNotes] = useState("");

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/activities", {
        holding_id: holdingId,
        activity_type: type,
        timestamp: Math.floor(new Date(date).getTime() / 1000),
        quantity: parseFloat(qty || "0"),
        price_per_unit: price ? parseFloat(price) : null,
        fee: fee ? parseFloat(fee) : 0,
        notes: notes || null,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["activities", holdingId] });
      qc.invalidateQueries({ queryKey: ["holdings"] });
      qc.invalidateQueries({ queryKey: ["holdings-net-worth"] });
      qc.invalidateQueries({ queryKey: ["portfolio-history"] });
      setQty("");
      setPrice("");
      setFee("");
      setNotes("");
      setOpen(false);
    },
  });

  if (!open) {
    return (
      <button className="btn-cta" onClick={() => setOpen(true)}>
        <Plus className="size-4" /> Record buy / sell / dividend
      </button>
    );
  }

  return (
    <form
      className="card p-5 space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (qty) create.mutate();
      }}
    >
      <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Type
          </label>
          <select
            className="input"
            value={type}
            onChange={(e) => setType(e.target.value as any)}
          >
            <option value="BUY">Buy</option>
            <option value="SELL">Sell</option>
            <option value="DIVIDEND">Dividend</option>
            <option value="SPLIT">Split</option>
            <option value="FEE">Fee</option>
            <option value="INTEREST">Interest</option>
          </select>
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Date &amp; time
          </label>
          <input
            type="datetime-local"
            className="input mono"
            value={date}
            onChange={(e) => setDate(e.target.value)}
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Quantity
          </label>
          <input
            className="input mono"
            inputMode="decimal"
            value={qty}
            onChange={(e) => setQty(e.target.value)}
            placeholder="e.g. 10"
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Price / unit
          </label>
          <input
            className="input mono"
            inputMode="decimal"
            value={price}
            onChange={(e) => setPrice(e.target.value)}
            placeholder={symbol ? `e.g. price of ${symbol}` : "price"}
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Fee (optional)
          </label>
          <input
            className="input mono"
            inputMode="decimal"
            value={fee}
            onChange={(e) => setFee(e.target.value)}
          />
        </div>
        <div className="md:col-span-3">
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Notes (optional)
          </label>
          <input
            className="input"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            placeholder="e.g. dividend reinvestment"
          />
        </div>
      </div>
      <div className="flex gap-2">
        <button className="btn-cta" type="submit" disabled={create.isPending}>
          Record
        </button>
        <button type="button" className="btn-ghost" onClick={() => setOpen(false)}>
          Cancel
        </button>
      </div>
    </form>
  );
}

