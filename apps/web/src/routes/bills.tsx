import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2, Pencil, Check, X } from "lucide-react";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import type { Bill } from "@/types/api";

export const Route = createFileRoute("/bills")({ component: BillsPage });

function BillsPage() {
  const qc = useQueryClient();
  const bills = useQuery({
    queryKey: ["bills"],
    queryFn: () => api.get<{ bills: Bill[] }>("/api/bills"),
  });

  const [name, setName] = useState("");
  const [amount, setAmount] = useState("");
  const [regex, setRegex] = useState("");
  const [freq, setFreq] = useState<
    "monthly" | "weekly" | "yearly" | "fortnightly"
  >("monthly");

  const create = useMutation({
    mutationFn: () => {
      const cents = Math.round(parseFloat(amount) * 100);
      return api.post("/api/bills", {
        name,
        expected_amount_min_cents: Math.round(cents * 0.95),
        expected_amount_max_cents: Math.round(cents * 1.05),
        repeat_freq: freq,
        match_description_regex: regex || null,
      });
    },
    onSuccess: () => {
      setName("");
      setAmount("");
      setRegex("");
      qc.invalidateQueries({ queryKey: ["bills"] });
    },
  });
  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Bills</em>
        </h1>
        <p className="text-mid text-sm">
          Recurring expenses, auto-matched to incoming transactions.
        </p>
      </header>

      <form
        className="card p-5 space-y-2 fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name && amount) create.mutate();
        }}
      >
        <input
          className="input"
          placeholder="Bill name (e.g. Netflix)"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input"
          placeholder="Expected amount"
          inputMode="decimal"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
        />
        <input
          className="input"
          placeholder="Match regex (optional, e.g. NETFLIX)"
          value={regex}
          onChange={(e) => setRegex(e.target.value)}
        />
        <select
          className="input"
          value={freq}
          onChange={(e) => setFreq(e.target.value as any)}
        >
          <option value="monthly">Monthly</option>
          <option value="weekly">Weekly</option>
          <option value="fortnightly">Fortnightly</option>
          <option value="yearly">Yearly</option>
        </select>
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Create bill
        </button>
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {bills.data?.bills.map((b) => (
          <BillRow key={b.id} b={b} />
        ))}
      </ul>
    </div>
  );
}

function BillRow({ b }: { b: Bill }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(b.name);
  const [amount, setAmount] = useState(
    b.expected_amount_max_cents > 0 ? (b.expected_amount_max_cents / 100).toFixed(2) : "",
  );
  const [regex, setRegex] = useState(b.match_description_regex ?? "");

  const save = useMutation({
    mutationFn: () => {
      const cents = amount ? Math.round(parseFloat(amount) * 100) : null;
      return api.patch(`/api/bills/${b.id}`, {
        name,
        match_description_regex: regex || null,
        ...(cents != null
          ? {
              expected_amount_min_cents: Math.round(cents * 0.95),
              expected_amount_max_cents: Math.round(cents * 1.05),
            }
          : {}),
      });
    },
    onSuccess: () => {
      setEditing(false);
      qc.invalidateQueries({ queryKey: ["bills"] });
    },
  });
  const del = useMutation({
    mutationFn: () => api.delete(`/api/bills/${b.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["bills"] }),
  });

  if (editing) {
    return (
      <li className="px-5 py-3 space-y-2">
        <input
          className="input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Name"
        />
        <div className="grid grid-cols-2 gap-2">
          <input
            className="input"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            placeholder="Amount"
            inputMode="decimal"
          />
          <input
            className="input"
            value={regex}
            onChange={(e) => setRegex(e.target.value)}
            placeholder="Match regex"
          />
        </div>
        <div className="flex gap-2">
          <button className="btn-primary" onClick={() => save.mutate()} disabled={save.isPending}>
            <Check className="size-4" /> Save
          </button>
          <button className="btn-ghost text-xs" onClick={() => setEditing(false)}>
            <X className="size-4" /> Cancel
          </button>
        </div>
      </li>
    );
  }
  return (
    <li className="px-5 py-3.5 flex items-center justify-between">
      <div>
        <p className="font-semibold">{b.name}</p>
        <p className="text-[11px] mono text-mid mt-0.5">{b.repeat_freq}</p>
      </div>
      <div className="flex items-center gap-2">
        <span className="mono text-orange font-semibold">
          {b.expected_amount_max_cents > 0
            ? formatMoney(b.expected_amount_max_cents, b.currency)
            : "amount unknown"}
        </span>
        <button className="btn-ghost text-xs" onClick={() => setEditing(true)}>
          <Pencil className="size-3.5" />
        </button>
        <button className="btn-ghost text-xs" onClick={() => del.mutate()}>
          <Trash2 className="size-3.5" />
        </button>
      </div>
    </li>
  );
}
