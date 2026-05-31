import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2, Tag, ExternalLink } from "lucide-react";
import { api } from "@/lib/api";
import type { WatchlistItem, WatchlistSource, DealObservation } from "@/types/api";

export const Route = createFileRoute("/watchlist")({ component: WatchlistPage });

type ItemRow = { item: WatchlistItem; sources: WatchlistSource[] };

function WatchlistPage() {
  const qc = useQueryClient();
  const wl = useQuery({
    queryKey: ["watchlist"],
    queryFn: () =>
      api.get<{ items: ItemRow[]; deals: DealObservation[] }>("/api/watchlist"),
  });

  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [feeds, setFeeds] = useState("");

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/watchlist", {
        name: name.trim(),
        target_price_cents: target ? Math.round(Number(target) * 100) : null,
        sources: feeds
          .split(/\s+/)
          .filter(Boolean)
          .map((u) => ({ kind: "rss", ref: u })),
      }),
    onSuccess: () => {
      setName("");
      setTarget("");
      setFeeds("");
      qc.invalidateQueries({ queryKey: ["watchlist"] });
    },
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/watchlist/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["watchlist"] }),
  });

  const deals = wl.data?.deals ?? [];

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Watchlist</em>
        </h1>
        <p className="text-mid text-sm">
          Type what you want plus a target price, and tally auto-watches HotUKDeals for it and
          alerts you when a deal drops to your target. Adding extra feeds is optional.
        </p>
      </header>

      <form
        className="card p-5 space-y-2 fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) create.mutate();
        }}
      >
        <input
          className="input"
          placeholder="Item, e.g. Festool TS 55 track saw"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input"
          type="number"
          step="0.01"
          placeholder="target price £ (optional)"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
        />
        <textarea
          className="input"
          rows={2}
          placeholder="Optional: extra RSS feed URLs, one per line (HotUKDeals is auto-added for the name)"
          value={feeds}
          onChange={(e) => setFeeds(e.target.value)}
        />
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Watch item
        </button>
      </form>

      <section className="space-y-2 fade-in-2">
        {wl.data?.items.map((row) => (
          <div key={row.item.id} className="card p-4 flex justify-between items-start">
            <div>
              <p className="font-extrabold tracking-tight flex items-center gap-2">
                <Tag className="size-4 text-green" /> {row.item.name}
              </p>
              <p className="text-[11px] uppercase tracking-widest text-mid mt-0.5">
                {row.item.target_price_cents != null
                  ? `target £${(row.item.target_price_cents / 100).toFixed(2)} · `
                  : ""}
                {row.sources.length} source{row.sources.length === 1 ? "" : "s"}
              </p>
            </div>
            <button className="btn-ghost text-xs" onClick={() => del.mutate(row.item.id)}>
              <Trash2 className="size-3.5" />
            </button>
          </div>
        ))}
      </section>

      <section className="fade-in-3">
        <h2 className="text-lg font-semibold mb-2">Recent deals found</h2>
        {deals.length === 0 ? (
          <p className="text-sm text-mid">Nothing yet — the watchlist is polled every few hours.</p>
        ) : (
          <ul className="card divide-y divide-thin">
            {deals.map((d) => (
              <li key={d.id} className="px-5 py-3 flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-sm truncate">{d.title}</p>
                  <p className="text-[11px] mono text-mid">
                    {d.source_kind}
                    {d.price_cents != null ? ` · £${(d.price_cents / 100).toFixed(2)}` : ""}
                  </p>
                </div>
                {d.url && (
                  <a
                    className="btn-ghost text-xs shrink-0"
                    href={d.url}
                    target="_blank"
                    rel="noreferrer"
                  >
                    <ExternalLink className="size-3.5" />
                  </a>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
