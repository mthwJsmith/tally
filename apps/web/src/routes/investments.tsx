import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  ChevronRight,
  Plus,
  RefreshCw,
  Trash2,
  TrendingUp,
  TrendingDown,
} from "lucide-react";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import { SymbolCombobox, type SymbolHit } from "@/components/SymbolCombobox";
import { PortfolioChart } from "@/components/PortfolioChart";

export const Route = createFileRoute("/investments")({ component: InvestmentsPage });

type Broker = {
  id: number;
  name: string;
  kind: string;
  currency: string;
  notes: string | null;
};

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
    last_synced_at: number | null;
  };
  current_price: number | null;
  market_value: number | null;
  cost_basis: number | null;
  gain: number | null;
  gain_pct: number | null;
  day_change_pct: number | null;
  company_name: string | null;
};

function InvestmentsPage() {
  const qc = useQueryClient();
  const brokers = useQuery({
    queryKey: ["brokers"],
    queryFn: () => api.get<{ brokers: Broker[] }>("/api/brokers"),
  });
  const holdings = useQuery({
    queryKey: ["holdings"],
    queryFn: () => api.get<{ holdings: EnrichedHolding[] }>("/api/holdings"),
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

  const syncQuotes = useMutation({
    mutationFn: () =>
      api.post<{ symbols: number; fetched: number }>("/api/holdings/sync-quotes"),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["holdings"] });
      qc.invalidateQueries({ queryKey: ["holdings-net-worth"] });
    },
  });

  const [showAddBroker, setShowAddBroker] = useState(false);
  const [showAddHolding, setShowAddHolding] = useState(false);

  const market = netWorth.data?.holdings_market_value ?? 0;
  const cost = netWorth.data?.holdings_cost_basis ?? 0;
  const gain = netWorth.data?.holdings_unrealised_gain ?? 0;
  const gainPct = cost > 0 ? (gain / cost) * 100 : 0;
  const positive = gain >= 0;
  // Pick the dominant currency from the holdings list so the totals don't render in £
  // when everything is USD. If holdings span multiple currencies we fall back to "" and
  // leave formatMoney to use its default — a multi-currency aggregate would need FX.
  const dominantCurrency = (() => {
    const list = holdings.data?.holdings ?? [];
    if (list.length === 0) return undefined;
    const ccys = new Set(list.map((x) => x.holding.currency));
    return ccys.size === 1 ? list[0].holding.currency : undefined;
  })();

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1280px]">
      <header className="flex items-end justify-between fade-in">
        <div>
          <h1 className="text-4xl mb-2">
            <em>Investments</em>
          </h1>
          <p className="text-mid text-sm">
            Manual holdings across all your brokers.
          </p>
        </div>
        <button
          className="btn-secondary"
          onClick={() => syncQuotes.mutate()}
          disabled={syncQuotes.isPending}
        >
          <RefreshCw
            className={`size-4 ${syncQuotes.isPending ? "animate-spin" : ""}`}
          />
          Sync prices
        </button>
      </header>

      {/* Net worth panel */}
      <section className="card p-7 fade-in-1">
        <div className="grid md:grid-cols-3 gap-6">
          <div>
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
              Market value
            </p>
            <p className="font-extrabold text-3xl mono tracking-tight">
              {formatMoney(Math.round(market * 100), dominantCurrency)}
            </p>
          </div>
          <div>
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
              Cost basis
            </p>
            <p className="font-extrabold text-3xl mono tracking-tight text-mid">
              {formatMoney(Math.round(cost * 100), dominantCurrency)}
            </p>
          </div>
          <div>
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
              Unrealised P&L
            </p>
            <p
              className={`font-extrabold text-3xl mono tracking-tight ${
                positive ? "text-green" : "text-danger"
              }`}
            >
              {positive ? "+" : "−"}
              {formatMoney(Math.round(Math.abs(gain) * 100), dominantCurrency)}
              <span className="text-base font-normal ml-2">
                ({positive ? "+" : "−"}
                {Math.abs(gainPct).toFixed(2)}%)
              </span>
            </p>
          </div>
        </div>
      </section>

      {/* Portfolio history chart */}
      <section className="card p-7 fade-in-2">
        <div className="flex items-baseline justify-between mb-4">
          <h2 className="text-2xl">
            Portfolio <em>over time</em>
          </h2>
          <p className="text-xs uppercase tracking-widest text-mid">
            Yahoo Finance · daily close
          </p>
        </div>
        <PortfolioChart showRangePicker height={320} />
      </section>

      {/* Brokers list */}
      <section className="fade-in-3">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold">Brokers</h2>
          <button
            className="btn-secondary text-xs"
            onClick={() => setShowAddBroker(!showAddBroker)}
          >
            <Plus className="size-3.5" /> Add broker
          </button>
        </div>
        {showAddBroker && <AddBrokerForm onDone={() => setShowAddBroker(false)} />}
        <div className="grid sm:grid-cols-2 md:grid-cols-3 gap-3">
          {brokers.data?.brokers.map((b) => (
            <BrokerCard key={b.id} broker={b} />
          ))}
          {!brokers.data?.brokers.length && (
            <p className="text-sm text-mid col-span-full">
              No brokers yet. Add Lightyear or AJ Bell to get started.
            </p>
          )}
        </div>
      </section>

      {/* Holdings table */}
      <section className="fade-in-4">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold">Holdings</h2>
          <button
            className="btn-cta text-xs"
            onClick={() => setShowAddHolding(!showAddHolding)}
          >
            <Plus className="size-3.5" /> Add activity
          </button>
        </div>
        {showAddHolding && (
          <AddActivityForm
            brokers={brokers.data?.brokers ?? []}
            onDone={() => setShowAddHolding(false)}
          />
        )}
        <div className="card overflow-hidden">
          <table className="w-full text-sm">
            <thead className="bg-cream">
              <tr className="text-[11px] uppercase tracking-widest text-mid">
                <th className="text-left px-4 py-2.5">Symbol</th>
                <th className="text-left px-4 py-2.5">Quantity</th>
                <th className="text-right px-4 py-2.5">Avg cost</th>
                <th className="text-right px-4 py-2.5">Price</th>
                <th className="text-right px-4 py-2.5">Value</th>
                <th className="text-right px-4 py-2.5">P&L</th>
                <th className="px-4 py-2.5"></th>
              </tr>
            </thead>
            <tbody>
              {holdings.data?.holdings.map((h) => (
                <HoldingRow key={h.holding.id} h={h} />
              ))}
              {!holdings.data?.holdings.length && (
                <tr>
                  <td colSpan={7} className="px-4 py-8 text-center text-mid">
                    No holdings yet. Add one above.
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

function BrokerCard({ broker }: { broker: Broker }) {
  const qc = useQueryClient();
  const del = useMutation({
    mutationFn: () => api.delete(`/api/brokers/${broker.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["brokers"] }),
  });
  return (
    <div className="card p-4 flex items-center justify-between">
      <div>
        <p className="font-extrabold tracking-tight">{broker.name}</p>
        <p className="text-[10px] uppercase tracking-widest text-mid mt-1">
          {broker.kind} · {broker.currency}
        </p>
      </div>
      <button className="btn-ghost text-xs" onClick={() => del.mutate()}>
        <Trash2 className="size-3.5" />
      </button>
    </div>
  );
}

function AddBrokerForm({ onDone }: { onDone: () => void }) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<"general" | "isa" | "sipp" | "crypto">("general");
  const [currency, setCurrency] = useState("GBP");
  const create = useMutation({
    mutationFn: () => api.post("/api/brokers", { name, kind, currency }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["brokers"] });
      onDone();
    },
  });
  return (
    <form
      className="card p-4 space-y-2 mb-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (name.trim()) create.mutate();
      }}
    >
      <input
        className="input"
        placeholder="Broker name (e.g. Lightyear, AJ Bell SIPP)"
        value={name}
        onChange={(e) => setName(e.target.value)}
        autoFocus
      />
      <div className="grid grid-cols-2 gap-2">
        <select
          className="input"
          value={kind}
          onChange={(e) => setKind(e.target.value as any)}
        >
          <option value="general">General</option>
          <option value="isa">ISA</option>
          <option value="sipp">SIPP / pension</option>
          <option value="crypto">Crypto</option>
        </select>
        <input
          className="input"
          value={currency}
          onChange={(e) => setCurrency(e.target.value)}
          placeholder="GBP"
        />
      </div>
      <div className="flex gap-2">
        <button className="btn-cta" type="submit" disabled={create.isPending}>
          Add
        </button>
        <button type="button" className="btn-ghost" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/**
 * Single-form "Add activity" — Ghostfolio model.
 *
 * Pick broker + symbol + type + date/time + quantity + price (+ optional fee/notes).
 * Backend upserts the holding by (broker_id, symbol) on first BUY of a symbol, then
 * recomputes the position's quantity + avg cost from the entire activity log. No
 * separate "create holding" step.
 */
function AddActivityForm({ brokers, onDone }: { brokers: Broker[]; onDone: () => void }) {
  const qc = useQueryClient();
  const [brokerId, setBrokerId] = useState<number | "">(
    brokers.length === 1 ? brokers[0].id : "",
  );
  const [type, setType] = useState<
    "BUY" | "SELL" | "DIVIDEND" | "SPLIT" | "FEE" | "INTEREST"
  >("BUY");
  const [symbol, setSymbol] = useState("");
  const [resolvedName, setResolvedName] = useState<string | null>(null);
  const [date, setDate] = useState(() => {
    const now = new Date();
    const off = now.getTimezoneOffset();
    return new Date(now.getTime() - off * 60_000).toISOString().slice(0, 16);
  });
  const [qty, setQty] = useState("");
  const [price, setPrice] = useState("");
  const [fee, setFee] = useState("");
  const [currency, setCurrency] = useState("GBP");
  const [notes, setNotes] = useState("");

  const create = useMutation({
    mutationFn: () =>
      api.post<{ id: number; holding_id: number }>("/api/activities", {
        broker_id: brokerId,
        symbol: symbol.toUpperCase().trim(),
        name: resolvedName,
        activity_type: type,
        timestamp: Math.floor(new Date(date).getTime() / 1000),
        quantity: parseFloat(qty || "0"),
        price_per_unit: price ? parseFloat(price) : null,
        fee: fee ? parseFloat(fee) : 0,
        currency,
        notes: notes || null,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["holdings"] });
      qc.invalidateQueries({ queryKey: ["holdings-net-worth"] });
      onDone();
    },
  });

  function onSymbolPick(sym: string, hit?: SymbolHit) {
    setSymbol(sym);
    if (hit) {
      setResolvedName(hit.name);
      if (hit.symbol.endsWith(".L")) setCurrency("GBP");
      else if (hit.quote_type === "CRYPTOCURRENCY") setCurrency("USD");
      else setCurrency("USD");
    } else {
      setResolvedName(null);
    }
  }

  const canSubmit = brokerId && symbol && qty;

  return (
    <form
      className="card p-5 space-y-3 mb-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (canSubmit) create.mutate();
      }}
    >
      <div className="grid sm:grid-cols-2 gap-3">
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Broker / account
          </label>
          <select
            className="input"
            value={brokerId}
            onChange={(e) =>
              setBrokerId(e.target.value ? Number(e.target.value) : "")
            }
          >
            <option value="">Choose broker…</option>
            {brokers.map((b) => (
              <option key={b.id} value={b.id}>
                {b.name}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Activity type
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
      </div>
      <div>
        <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
          Symbol
        </label>
        <SymbolCombobox value={symbol} onChange={onSymbolPick} autoFocus />
        {resolvedName && (
          <p className="text-xs text-green mt-1 truncate">{resolvedName}</p>
        )}
      </div>
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
        <div className="col-span-2">
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
            placeholder="e.g. 10"
            inputMode="decimal"
            value={qty}
            onChange={(e) => setQty(e.target.value)}
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Price / unit
          </label>
          <input
            className="input mono"
            placeholder="e.g. 99.62"
            inputMode="decimal"
            value={price}
            onChange={(e) => setPrice(e.target.value)}
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Fee (opt.)
          </label>
          <input
            className="input mono"
            inputMode="decimal"
            value={fee}
            onChange={(e) => setFee(e.target.value)}
          />
        </div>
        <div>
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Currency
          </label>
          <input
            className="input mono"
            value={currency}
            onChange={(e) => setCurrency(e.target.value)}
          />
        </div>
        <div className="col-span-2">
          <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
            Notes (optional)
          </label>
          <input
            className="input"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
          />
        </div>
      </div>
      <div className="flex gap-2 pt-1">
        <button
          className="btn-cta"
          type="submit"
          disabled={create.isPending || !canSubmit}
        >
          Record {type.toLowerCase()}
        </button>
        <button type="button" className="btn-ghost" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}

function HoldingRow({ h }: { h: EnrichedHolding }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const del = useMutation({
    mutationFn: () => api.delete(`/api/holdings/${h.holding.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["holdings"] }),
  });
  const open = () =>
    navigate({ to: "/investments/$id", params: { id: h.holding.id.toString() } });
  const positive = (h.gain ?? 0) >= 0;
  return (
    <tr
      className="border-t border-thin hover:bg-cream/60 cursor-pointer"
      onClick={open}
    >
      <td className="px-4 py-3">
        <p className="mono font-semibold">{h.holding.symbol}</p>
        <p className="text-[11px] text-mid mt-0.5 truncate max-w-[180px]">
          {h.company_name || h.holding.name || ""}
        </p>
      </td>
      <td className="px-4 py-3 mono">{h.holding.quantity}</td>
      <td className="px-4 py-3 mono text-right text-mid">
        {h.holding.avg_cost_per_unit != null
          ? `${h.holding.avg_cost_per_unit.toFixed(2)}`
          : "—"}
      </td>
      <td className="px-4 py-3 mono text-right">
        {h.current_price != null ? h.current_price.toFixed(2) : "—"}
        {h.day_change_pct != null && (
          <span
            className={`block text-[10px] mt-0.5 ${
              h.day_change_pct >= 0 ? "text-green" : "text-danger"
            }`}
          >
            {h.day_change_pct >= 0 ? "+" : ""}
            {h.day_change_pct.toFixed(2)}%
          </span>
        )}
      </td>
      <td className="px-4 py-3 mono text-right">
        {h.market_value != null
          ? formatMoney(Math.round(h.market_value * 100), h.holding.currency)
          : "—"}
      </td>
      <td className="px-4 py-3 mono text-right">
        {h.gain != null ? (
          <span
            className={`inline-flex items-center gap-1 ${
              positive ? "text-green" : "text-danger"
            }`}
          >
            {positive ? (
              <TrendingUp className="size-3" />
            ) : (
              <TrendingDown className="size-3" />
            )}
            {positive ? "+" : "−"}
            {formatMoney(
              Math.round(Math.abs(h.gain) * 100),
              h.holding.currency
            )}
            {h.gain_pct != null && (
              <span className="text-[10px] text-mid">
                ({h.gain_pct.toFixed(1)}%)
              </span>
            )}
          </span>
        ) : (
          <span className="text-mid">—</span>
        )}
      </td>
      <td className="px-4 py-3 text-right whitespace-nowrap">
        <button
          className="btn-secondary text-xs"
          onClick={(e) => {
            e.stopPropagation();
            open();
          }}
          title="Edit / view activity"
        >
          Edit <ChevronRight className="size-3.5" />
        </button>
        <button
          className="btn-ghost text-xs ml-1"
          onClick={(e) => {
            e.stopPropagation();
            if (confirm(`Delete ${h.holding.symbol}?`)) del.mutate();
          }}
          title="Delete holding"
        >
          <Trash2 className="size-3.5" />
        </button>
      </td>
    </tr>
  );
}
