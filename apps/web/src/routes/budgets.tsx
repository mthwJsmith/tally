import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import type { Budget, BudgetStatus, Category } from "@/types/api";

export const Route = createFileRoute("/budgets")({ component: BudgetsPage });

function BudgetsPage() {
  const qc = useQueryClient();
  const budgets = useQuery({
    queryKey: ["budgets"],
    queryFn: () => api.get<{ budgets: Budget[] }>("/api/budgets"),
  });
  const cats = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });

  const [name, setName] = useState("");
  const [amount, setAmount] = useState("");
  const [category, setCategory] = useState<number | "">("");
  const [period, setPeriod] = useState<"monthly" | "weekly" | "yearly">("monthly");

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/budgets", {
        name,
        category_id: category || null,
        amount_cents: Math.round(parseFloat(amount) * 100),
        period,
      }),
    onSuccess: () => {
      setName("");
      setAmount("");
      setCategory("");
      qc.invalidateQueries({ queryKey: ["budgets"] });
    },
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/budgets/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["budgets"] }),
  });

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Budgets</em>
        </h1>
        <p className="text-mid text-sm">
          Track monthly, weekly or yearly spend per category.
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
          placeholder="Budget name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input"
          placeholder="Amount (e.g. 200.00)"
          inputMode="decimal"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
        />
        <select
          className="input"
          value={category}
          onChange={(e) =>
            setCategory(e.target.value ? Number(e.target.value) : "")
          }
        >
          <option value="">All spending</option>
          {cats.data?.categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        <select
          className="input"
          value={period}
          onChange={(e) => setPeriod(e.target.value as any)}
        >
          <option value="monthly">Monthly</option>
          <option value="weekly">Weekly</option>
          <option value="yearly">Yearly</option>
        </select>
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Create budget
        </button>
      </form>

      <div className="grid md:grid-cols-2 gap-3 fade-in-2">
        {budgets.data?.budgets.map((b) => (
          <BudgetCard key={b.id} b={b} onDelete={() => del.mutate(b.id)} />
        ))}
      </div>
    </div>
  );
}

function BudgetCard({ b, onDelete }: { b: Budget; onDelete: () => void }) {
  const s = useQuery({
    queryKey: ["budgets", b.id, "status"],
    queryFn: () => api.get<BudgetStatus>(`/api/budgets/${b.id}/status`),
  });
  const pct = Math.min(s.data?.percent ?? 0, 100);
  return (
    <div className="card p-5">
      <div className="flex justify-between items-start">
        <div>
          <p className="font-extrabold tracking-tight">{b.name}</p>
          <p className="text-[10px] uppercase tracking-widest text-mid mt-0.5">
            {b.period}
          </p>
        </div>
        <button className="btn-ghost text-xs" onClick={onDelete}>
          <Trash2 className="size-3.5" />
        </button>
      </div>
      <p className="mono mt-3 text-2xl font-bold">
        {formatMoney(s.data?.spent_cents ?? 0, b.currency)}
        <span className="text-mid text-sm font-normal ml-1">
          / {formatMoney(b.amount_cents, b.currency)}
        </span>
      </p>
      <div className="h-1.5 bg-cream mt-3 overflow-hidden">
        <div
          className={`h-full ${s.data?.over_budget ? "bg-danger" : "bg-green"}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      {s.data?.over_budget && (
        <p className="text-xs text-danger mt-1.5">Over budget</p>
      )}
    </div>
  );
}
