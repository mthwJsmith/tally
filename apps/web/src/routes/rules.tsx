import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2, Play, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import type { Category, Rule } from "@/types/api";

export const Route = createFileRoute("/rules")({ component: RulesPage });

function RulesPage() {
  const qc = useQueryClient();
  const rules = useQuery({
    queryKey: ["rules"],
    queryFn: () => api.get<{ rules: Rule[] }>("/api/rules"),
  });
  const cats = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });

  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [merchant, setMerchant] = useState("");
  const [cat, setCat] = useState<number | "">("");

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/rules", {
        name,
        match_description_regex: desc || null,
        match_merchant_regex: merchant || null,
        set_category_id: cat || null,
      }),
    onSuccess: () => {
      setName("");
      setDesc("");
      setMerchant("");
      setCat("");
      qc.invalidateQueries({ queryKey: ["rules"] });
    },
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/rules/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["rules"] }),
  });
  const test = useMutation({
    mutationFn: (id: number) =>
      api.post<{ matched_count: number }>(`/api/rules/${id}/test`),
  });
  const runAll = useMutation({
    mutationFn: () =>
      api.post<{ matched: number; mutated: number }>("/api/rules/run-all"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["transactions"] }),
  });

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="flex items-end justify-between fade-in">
        <div>
          <h1 className="text-4xl mb-2">
            <em>Rules</em>
          </h1>
          <p className="text-mid text-sm">
            Auto-categorise transactions matching description or merchant
            patterns.
          </p>
        </div>
        <button
          className="btn-outlined"
          onClick={() => runAll.mutate()}
          disabled={runAll.isPending}
        >
          <RefreshCw className={`size-4 ${runAll.isPending ? "animate-spin" : ""}`} />
          Re-apply all
        </button>
      </header>
      {runAll.data && (
        <p className="text-sm text-green fade-in">
          Matched {runAll.data.matched}, updated {runAll.data.mutated}.
        </p>
      )}

      <form
        className="card p-5 space-y-2 fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) create.mutate();
        }}
      >
        <input
          className="input"
          placeholder="Rule name (e.g. Tesco → Groceries)"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input"
          placeholder="Description regex (e.g. TESCO|SAINSBURY)"
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
        />
        <input
          className="input"
          placeholder="Merchant regex (optional)"
          value={merchant}
          onChange={(e) => setMerchant(e.target.value)}
        />
        <select
          className="input"
          value={cat}
          onChange={(e) => setCat(e.target.value ? Number(e.target.value) : "")}
        >
          <option value="">Set category</option>
          {cats.data?.categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Create rule
        </button>
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {rules.data?.rules.map((r) => (
          <li key={r.id} className="px-5 py-4 text-sm">
            <div className="flex items-center justify-between">
              <p className="font-semibold">{r.name}</p>
              <div className="flex items-center gap-1">
                <button
                  className="btn-ghost text-xs"
                  onClick={() => test.mutate(r.id)}
                >
                  <Play className="size-3.5" /> Test
                </button>
                <button
                  className="btn-ghost text-xs"
                  onClick={() => del.mutate(r.id)}
                >
                  <Trash2 className="size-3.5" /> Delete
                </button>
              </div>
            </div>
            <p className="text-xs text-mid mt-1.5 mono">
              {r.match_description_regex && (
                <>
                  desc: <span className="text-green">{r.match_description_regex}</span>{" "}
                </>
              )}
              {r.match_merchant_regex && (
                <>
                  merchant:{" "}
                  <span className="text-green">{r.match_merchant_regex}</span>{" "}
                </>
              )}
              applied {r.times_applied}x
            </p>
            {test.variables === r.id && test.data && (
              <p className="text-xs text-green mt-1.5">
                Matches {test.data.matched_count} transactions
              </p>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
