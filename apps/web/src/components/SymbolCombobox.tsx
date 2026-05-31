/**
 * Async stock-symbol autocomplete.
 *
 * - TanStack Query handles debounce + cache for the symbol search endpoint
 * - 300ms debounce after last keystroke before firing a search
 * - No external combobox lib — just a focused input + dropdown
 * - Server-side filter: cmdk-style shouldFilter:false equivalent
 *
 * Wires to: GET /api/holdings/symbol-search?q=...
 * which proxies Yahoo Finance unofficial search.
 */
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Search, X } from "lucide-react";
import { api } from "@/lib/api";

export interface SymbolHit {
  symbol: string;
  name: string;
  exchange: string;
  quote_type: string;
}

interface Props {
  value: string;
  onChange: (symbol: string, hit?: SymbolHit) => void;
  placeholder?: string;
  autoFocus?: boolean;
}

export function SymbolCombobox({
  value,
  onChange,
  placeholder = "Search symbol or company",
  autoFocus,
}: Props) {
  const [query, setQuery] = useState(value);
  const [debounced, setDebounced] = useState(value);
  const [open, setOpen] = useState(false);
  const [activeIdx, setActiveIdx] = useState(-1);
  const containerRef = useRef<HTMLDivElement>(null);

  // Sync external value -> local input
  useEffect(() => {
    setQuery(value);
  }, [value]);

  // Debounce — wait 300ms after last keystroke
  useEffect(() => {
    const t = setTimeout(() => setDebounced(query), 300);
    return () => clearTimeout(t);
  }, [query]);

  // Close on outside click
  useEffect(() => {
    function onDocClick(e: MouseEvent) {
      if (!containerRef.current?.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, []);

  const search = useQuery({
    queryKey: ["symbol-search", debounced],
    enabled: debounced.trim().length >= 1 && open,
    staleTime: 60_000,
    queryFn: () =>
      api.get<{ hits: SymbolHit[] }>(
        `/api/holdings/symbol-search?q=${encodeURIComponent(debounced)}`
      ),
  });

  const hits = search.data?.hits ?? [];

  function pick(hit: SymbolHit) {
    setQuery(hit.symbol);
    setDebounced(hit.symbol);
    onChange(hit.symbol, hit);
    setOpen(false);
    setActiveIdx(-1);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "Enter") {
        setOpen(true);
      }
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(i + 1, hits.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const h = activeIdx >= 0 ? hits[activeIdx] : hits[0];
      if (h) pick(h);
    } else if (e.key === "Escape") {
      setOpen(false);
      setActiveIdx(-1);
    }
  }

  return (
    <div ref={containerRef} className="relative">
      <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-mid pointer-events-none" />
      <input
        type="text"
        className="input pl-9 pr-9 mono"
        placeholder={placeholder}
        value={query}
        autoFocus={autoFocus}
        onChange={(e) => {
          setQuery(e.target.value);
          onChange(e.target.value.toUpperCase(), undefined);
          setOpen(true);
          setActiveIdx(-1);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={onKeyDown}
        autoComplete="off"
        spellCheck={false}
      />
      {query && (
        <button
          type="button"
          className="absolute right-2.5 top-1/2 -translate-y-1/2 text-mid hover:text-ink"
          onClick={() => {
            setQuery("");
            onChange("", undefined);
          }}
          tabIndex={-1}
        >
          <X className="size-4" />
        </button>
      )}

      {open && debounced.trim().length >= 1 && (
        <div className="absolute left-0 right-0 top-full mt-1 z-20 card max-h-72 overflow-y-auto">
          {search.isPending && (
            <div className="px-3 py-3 text-xs text-mid">Searching…</div>
          )}
          {search.isError && (
            <div className="px-3 py-3 text-xs text-danger">
              Search failed. You can still type a symbol manually.
            </div>
          )}
          {search.isSuccess && hits.length === 0 && (
            <div className="px-3 py-3 text-xs text-mid">
              No matches. Type a symbol exactly if you know it
              (e.g. <span className="mono">VWRP.L</span>).
            </div>
          )}
          {hits.map((h, i) => (
            <button
              key={`${h.symbol}-${h.exchange}-${i}`}
              type="button"
              className={`w-full text-left px-3 py-2.5 text-sm flex items-center justify-between gap-2 hover:bg-cream ${
                i === activeIdx ? "bg-cream" : ""
              }`}
              onMouseEnter={() => setActiveIdx(i)}
              onMouseDown={(e) => {
                e.preventDefault();
                pick(h);
              }}
            >
              <div className="min-w-0">
                <p className="mono font-semibold text-ink">{h.symbol}</p>
                <p className="text-[11px] text-mid truncate">{h.name}</p>
              </div>
              <div className="shrink-0 text-right">
                <span className="pill-grey">{h.quote_type || "—"}</span>
                <p className="text-[10px] text-mid uppercase tracking-widest mt-0.5">
                  {h.exchange || ""}
                </p>
              </div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
