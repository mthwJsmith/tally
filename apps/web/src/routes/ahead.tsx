import { createFileRoute } from "@tanstack/react-router";
import { useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Sparkles,
  Check,
  ArrowRight,
  AlertTriangle,
  Plus,
  Trash2,
  CreditCard,
  Landmark,
  PiggyBank,
  RefreshCw,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  Pencil,
  X,
  ListChecks,
} from "lucide-react";
import {
  ResponsiveContainer,
  LineChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
  ReferenceLine,
  Legend as RLegend,
} from "recharts";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";

export const Route = createFileRoute("/ahead")({ component: AheadPage });

// ---------------------------------------------------------------------------
// "Ahead" — the cashflow forecast. Reads /api/ahead (planning accounts + dated
// events with per-account legs + goals), draws a Flipped grid (accounts down,
// days across) and a runway Curve. Edits persist via /api/plan + /api/goals.
// In dev with no session the API 401s, so we fall back to a mock scenario and
// keep edits local — the look-and-feel loop survives without logging in.
// ---------------------------------------------------------------------------

interface Leg {
  accountId: number;
  deltaCents: number;
}
interface AheadAccount {
  id: number;
  name: string;
  kind: string;
  source: string;
  balanceCents: number;
  currency: string;
  floorCents: number;
  overflowAccountId?: number | null;
  cliffDateIso: string | null;
  cliffNewFloorCents: number | null;
  creditLimitCents: number | null;
  aprBps: number | null;
  statementDay: number | null;
  isManual: boolean;
  lowCents: number;
  lowDateIso: string;
}
interface AheadEvent {
  id: string; // occurrence id "<eventId>-<dateIso>"
  dateIso: string;
  label: string;
  source: "actual" | "auto" | "planned" | "llm";
  recurrence: string;
  instance: boolean;
  note: string | null;
  reconciled?: boolean; // every leg already reflected in a live synced balance (e.g. salary that landed)
  legs: Leg[];
}
interface AheadMarker {
  dateIso: string;
  label: string;
  accountId: number;
}
interface AheadGoal {
  id: number;
  name: string;
  targetCents: number;
  savedCents: number;
  sourceAccountId: number | null;
  targetDateIso: string | null;
  monthlyCents: number;
}
interface Ahead {
  fromIso: string;
  toIso: string;
  accounts: AheadAccount[];
  events: AheadEvent[];
  markers: AheadMarker[];
  goals: AheadGoal[];
  actionPlan?: string;
}

// ---- mock fallback (dev, no session) --------------------------------------

const MOCK: Ahead = {
  fromIso: "2026-06-22",
  toIso: "2026-12-22",
  accounts: [
    { id: 1, name: "Main current", kind: "current", source: "synced", balanceCents: 124_000, currency: "GBP", floorCents: 0, cliffDateIso: null, cliffNewFloorCents: null, creditLimitCents: null, aprBps: null, statementDay: null, isManual: false, lowCents: -65_200, lowDateIso: "2026-06-28" },
    { id: 2, name: "Overdraft current", kind: "current", source: "synced", balanceCents: -83_200, currency: "GBP", floorCents: -100_000, cliffDateIso: "2026-07-15", cliffNewFloorCents: 0, creditLimitCents: null, aprBps: null, statementDay: null, isManual: false, lowCents: -83_200, lowDateIso: "2026-06-22" },
    { id: 3, name: "Credit card", kind: "credit", source: "manual", balanceCents: -41_000, currency: "GBP", floorCents: -120_000, cliffDateIso: null, cliffNewFloorCents: null, creditLimitCents: 120_000, aprBps: 3490, statementDay: 6, isManual: true, lowCents: -41_000, lowDateIso: "2026-06-22" },
  ],
  events: [
    { id: "1-2026-06-25", dateIso: "2026-06-25", label: "Energy bill", source: "auto", recurrence: "monthly", instance: false, note: null, legs: [{ accountId: 1, deltaCents: -11_700 }] },
    { id: "2-2026-06-26", dateIso: "2026-06-26", label: "Rent", source: "llm", recurrence: "monthly", instance: false, note: null, legs: [{ accountId: 1, deltaCents: -95_000 }] },
    { id: "3-2026-06-27", dateIso: "2026-06-27", label: "Card min payment", source: "planned", recurrence: "none", instance: false, note: null, legs: [{ accountId: 1, deltaCents: -2_500 }, { accountId: 3, deltaCents: 2_500 }] },
    { id: "4-2026-06-28", dateIso: "2026-06-28", label: "Car repair", source: "llm", recurrence: "none", instance: false, note: null, legs: [{ accountId: 1, deltaCents: -80_000 }] },
    { id: "5-2026-06-30", dateIso: "2026-06-30", label: "Payday", source: "auto", recurrence: "monthly", instance: false, note: null, legs: [{ accountId: 1, deltaCents: 210_000 }] },
    { id: "6-2026-07-14", dateIso: "2026-07-14", label: "Refill 0% overdraft", source: "llm", recurrence: "none", instance: false, note: null, legs: [{ accountId: 1, deltaCents: -90_000 }, { accountId: 2, deltaCents: 90_000 }] },
    { id: "8-2026-07-25", dateIso: "2026-07-25", label: "Energy bill", source: "auto", recurrence: "monthly", instance: true, note: null, legs: [{ accountId: 1, deltaCents: -11_700 }] },
    { id: "9-2026-07-26", dateIso: "2026-07-26", label: "Rent", source: "auto", recurrence: "monthly", instance: true, note: null, legs: [{ accountId: 1, deltaCents: -95_000 }] },
  ],
  actionPlan:
    "JULY\n- Mon 14 Jul — move £600 to the overdraft account (before the 0% cliff)\n- Fri 31 Jul — move £910 more to clear it\n- Keep the card on minimum; pay the lump in September",
  markers: [{ dateIso: "2026-07-15", label: "0% buffer ends", accountId: 2 }],
  goals: [
    { id: 1, name: "Car fund", targetCents: 300_000, savedCents: 90_000, sourceAccountId: null, targetDateIso: "2026-12-31", monthlyCents: 15_000 },
    { id: 2, name: "Emergency buffer", targetCents: 500_000, savedCents: 180_000, sourceAccountId: null, targetDateIso: "2027-06-30", monthlyCents: 10_000 },
  ],
};

const TODAY_ISO = MOCK.fromIso; // mock anchor; live mode uses real server dates

// ---- helpers --------------------------------------------------------------

// 15 distinct, cream-friendly line colours. An account is assigned one by its POSITION
// (deterministic — never two the same by chance); colours only repeat if you have >15 accounts.
const PALETTE = [
  "#505E4D", "#c67e5b", "#4B6B5A", "#b86843", "#6B7A63",
  "#3f6b7d", "#9a5b6e", "#b59a4d", "#5a8a6b", "#7d6b9a",
  "#a0703f", "#4a7a8c", "#c08a8a", "#6e6e8a", "#8c6a56",
];
const TOTAL_COLOR = "#2a1f1a"; // ink — the bold "Total" (net) line

const COLORS_KEY = "ahead-account-colors";
function loadColors(): Record<number, string> {
  try {
    return JSON.parse(localStorage.getItem(COLORS_KEY) || "{}");
  } catch {
    return {};
  }
}
function saveColors(c: Record<number, string>) {
  try {
    localStorage.setItem(COLORS_KEY, JSON.stringify(c));
  } catch {
    /* ignore */
  }
}

// Curve time-range options (months forward); "all" = the whole fetched window.
const RANGES: { l: string; v: number | "all" }[] = [
  { l: "1M", v: 1 },
  { l: "3M", v: 3 },
  { l: "6M", v: 6 },
  { l: "1Y", v: 12 },
  { l: "All", v: "all" },
];

function fmtDelta(cents: number) {
  return `${cents < 0 ? "▾" : "▴"}${(Math.abs(cents) / 100).toLocaleString("en-GB", { maximumFractionDigits: 0 })}`;
}
function fmtDay(iso: string) {
  const d = new Date(iso + "T00:00:00");
  return { dow: d.toLocaleDateString("en-GB", { weekday: "short" }), dm: d.toLocaleDateString("en-GB", { day: "2-digit", month: "short" }), d: d.getDate() };
}
function monthKey(iso: string) {
  return iso.slice(0, 7);
}
function monthLabel(key: string) {
  return new Date(key + "-01T00:00:00").toLocaleDateString("en-GB", { month: "long", year: "numeric" });
}
function floorOn(a: AheadAccount, iso: string) {
  if (a.cliffDateIso && a.cliffNewFloorCents != null && iso >= a.cliffDateIso) return a.cliffNewFloorCents;
  return a.floorCents;
}
const SRC_ICON: Record<AheadEvent["source"], React.ReactNode> = {
  actual: <Check className="size-3 text-green" />,
  auto: <RefreshCw className="size-3 text-mid" />,
  planned: <span className="inline-block size-1.5 rounded-full bg-mid/50" />,
  llm: <Sparkles className="size-3 text-orange" />,
};

// ---- data hook (live API → mock fallback) ---------------------------------

function useAhead() {
  return useQuery({
    queryKey: ["ahead"],
    queryFn: async (): Promise<{ live: boolean; data: Ahead }> => {
      try {
        const res = await fetch("/api/ahead?days=365", {
          credentials: "same-origin",
          headers: { Accept: "application/json" },
        });
        if (res.ok) return { live: true, data: (await res.json()) as Ahead };
      } catch {
        /* fall through to mock */
      }
      return { live: false, data: MOCK };
    },
  });
}

// ===========================================================================

// Hover card for the runway curve. Rendered into a body portal so it can't be
// clipped by the chart's 200px box (it used to get cut off once enough accounts
// made the list long). Positioned at the cursor via `mouseRef`, clamped to the
// viewport. Recharts injects `active`/`payload`/`label`; we pass `mouseRef`.
function CurveTooltip({
  active,
  payload,
  label,
  mouseRef,
}: {
  active?: boolean;
  payload?: Array<{ dataKey?: string; name?: string; value?: number; color?: string }>;
  label?: string;
  mouseRef: React.MutableRefObject<{ x: number; y: number }>;
}) {
  if (!active || !payload || payload.length === 0) return null;
  const W = 220;
  const H = 28 + payload.length * 18;
  const { x, y } = mouseRef.current;
  let left = x + 14;
  let top = y + 14;
  if (typeof window !== "undefined") {
    if (left + W > window.innerWidth) left = Math.max(8, x - W - 14);
    if (top + H > window.innerHeight) top = Math.max(8, window.innerHeight - H - 8);
  }
  return createPortal(
    <div
      style={{
        position: "fixed",
        left,
        top,
        zIndex: 60,
        pointerEvents: "none",
        background: "#fffcf7",
        border: "1px solid #ded7cb",
        fontSize: 12,
        padding: "6px 8px",
        maxWidth: W,
        boxShadow: "0 4px 14px rgba(42,31,26,0.12)",
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 4, color: "#2a1f1a" }}>{label}</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        {payload.map((p, i) => (
          <div key={p.dataKey ?? i} style={{ display: "flex", alignItems: "center", gap: 6, whiteSpace: "nowrap" }}>
            <span style={{ width: 9, height: 9, borderRadius: 9999, background: p.color, flex: "0 0 auto" }} />
            <span style={{ color: "#6b5a52", flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>{p.name}</span>
            <span style={{ color: "#2a1f1a", fontVariantNumeric: "tabular-nums" }}>{formatMoney(Math.round((p.value ?? 0) * 100))}</span>
          </div>
        ))}
      </div>
    </div>,
    document.body,
  );
}

function AheadPage() {
  const qc = useQueryClient();
  const query = useAhead();
  const live = query.data?.live ?? false;
  const server = query.data?.data ?? MOCK;

  // In mock mode edits live in local state; in live mode the server is the source.
  const [mock, setMock] = useState<Ahead>(MOCK);
  const model = live ? server : mock;

  const [viewMonth, setViewMonth] = useState<string>("all");
  const [focus, setFocus] = useState<number | "all">("all");
  const [adding, setAdding] = useState(false);
  const [dayOpen, setDayOpen] = useState<string | null>(null);
  const [colors, setColors] = useState<Record<number, string>>(loadColors);
  // graph controls (independent of the grid's focus chips)
  const [graphRange, setGraphRange] = useState<number | "all">(6);
  const [hiddenLines, setHiddenLines] = useState<Set<number>>(new Set());
  const [showTotal, setShowTotal] = useState(true);
  const [lineMenuOpen, setLineMenuOpen] = useState(false);
  // Latest cursor position (client coords) so the portal'd curve tooltip can sit at the pointer.
  const mouseRef = useRef({ x: 0, y: 0 });

  const accounts = model.accounts;
  // Colour for an account: an explicit override, else its position in the palette.
  const colourOf = (id: number) => colors[id] ?? PALETTE[Math.max(0, accounts.findIndex((a) => a.id === id)) % PALETTE.length];
  const setColour = (id: number, hex: string) =>
    setColors((prev) => {
      const next = { ...prev, [id]: hex };
      saveColors(next);
      return next;
    });
  const events = useMemo(() => [...model.events].sort((a, b) => a.dateIso.localeCompare(b.dateIso) || a.id.localeCompare(b.id)), [model.events]);
  const today = live ? model.fromIso : TODAY_ISO;

  const months = useMemo(() => {
    const s = new Set<string>([monthKey(today)]);
    events.forEach((e) => s.add(monthKey(e.dateIso)));
    model.markers.forEach((m) => s.add(monthKey(m.dateIso)));
    // Only upcoming months, capped to 6, so the chip row never runs off screen.
    // ("All" still shows the full fetched window.)
    const cur = monthKey(today);
    return [...s].filter((m) => m >= cur).sort().slice(0, 6);
  }, [events, model.markers, today]);

  const refresh = () => qc.invalidateQueries({ queryKey: ["ahead"] });

  // --- mutations (live: API + refetch; mock: local state) ---
  const addEvent = (body: NewEvent) => {
    if (live) {
      api.post("/api/plan/events", body).then(refresh);
    } else {
      const id = `${Date.now()}-${body.dateIso}`;
      const legs: Leg[] = body.toAccountId
        ? [{ accountId: body.accountId!, deltaCents: -Math.abs(body.amountCents) }, { accountId: body.toAccountId, deltaCents: Math.abs(body.amountCents) }]
        : [{ accountId: body.accountId!, deltaCents: body.amountCents }];
      setMock((m) => ({ ...m, events: [...m.events, { id, dateIso: body.dateIso, label: body.label, source: "planned", recurrence: body.recurrence ?? "none", instance: false, note: null, legs }] }));
    }
  };
  const eventNumericId = (occId: string) => Number(occId.split("-")[0]);
  const patchAmount = (occId: string, accountId: number, cents: number) => {
    if (live) {
      // A transfer stores a positive magnitude (the backend derives both legs from it); a
      // one-sided event stores the signed amount. Editing either leg of a transfer is the
      // same magnitude, so send abs for transfers.
      const ev = events.find((e) => e.id === occId);
      const isTransfer = (ev?.legs.length ?? 0) === 2;
      api.patch(`/api/plan/events/${eventNumericId(occId)}`, { amountCents: isTransfer ? Math.abs(cents) : cents }).then(refresh);
    } else {
      setMock((m) => ({
        ...m,
        events: m.events.map((e) =>
          e.id === occId ? { ...e, legs: e.legs.map((l) => (l.accountId === accountId ? { ...l, deltaCents: cents } : l)) } : e
        ),
      }));
    }
  };
  const deleteEvent = (occId: string) => {
    if (live) api.delete(`/api/plan/events/${eventNumericId(occId)}`).then(refresh);
    else setMock((m) => ({ ...m, events: m.events.filter((e) => e.id !== occId) }));
  };
  const saveActions = (text: string) => {
    if (live) api.put("/api/plan/actions", { text }).then(refresh);
    else setMock((m) => ({ ...m, actionPlan: text }));
  };

  // --- projection ---
  // Synced accounts already include everything up to today in their live balance, so a planned
  // leg dated today-or-earlier on a synced account would double-count (e.g. salary that landed).
  // Manual accounts have no live feed, so their legs always apply. Mirrors build_forecast in
  // ahead.rs — kept per-leg so a mixed transfer (synced + manual) suppresses only the synced side.
  const syncedIds = useMemo(() => new Set(accounts.filter((a) => a.source === "synced").map((a) => a.id)), [accounts]);
  const legSuppressed = (e: AheadEvent, l: Leg) => syncedIds.has(l.accountId) && e.dateIso <= today;

  // Precomputed day-by-day projection with the floor-overflow cascade: walk events in date order,
  // apply each day's (unsuppressed) legs, then for any account whose balance dropped below its
  // floor, draw the shortfall from its overflow target so it sits exactly at the floor and the
  // linked account absorbs it. Mirrors build_forecast in ahead.rs. One snapshot per event-date.
  const projection = useMemo(() => {
    const floorAt = (a: AheadAccount, iso: string) =>
      a.cliffDateIso && a.cliffNewFloorCents != null && iso >= a.cliffDateIso ? a.cliffNewFloorCents : a.floorCents;
    const links: [number, number][] = accounts
      .filter((a) => a.overflowAccountId != null)
      .map((a) => [a.id, a.overflowAccountId as number]);
    const bal = new Map(accounts.map((a) => [a.id, a.balanceCents]));
    const cascade = (iso: string) => {
      for (const [src, tgt] of links) {
        const a = accounts.find((x) => x.id === src);
        if (!a) continue;
        const fl = floorAt(a, iso);
        const b = bal.get(src) ?? 0;
        if (b < fl) {
          const short = fl - b;
          bal.set(src, b + short);
          bal.set(tgt, (bal.get(tgt) ?? 0) - short);
        }
      }
    };
    cascade(today);
    const series: { iso: string; bal: Map<number, number> }[] = [{ iso: today, bal: new Map(bal) }];
    let i = 0;
    while (i < events.length) {
      const d = events[i].dateIso;
      while (i < events.length && events[i].dateIso === d) {
        for (const l of events[i].legs) {
          if (legSuppressed(events[i], l)) continue;
          bal.set(l.accountId, (bal.get(l.accountId) ?? 0) + l.deltaCents);
        }
        i++;
      }
      cascade(d);
      series.push({ iso: d, bal: new Map(bal) });
    }
    return series;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accounts, events, today]);

  const balanceAsOf = (accId: number, iso: string) => {
    let v = accounts.find((a) => a.id === accId)?.balanceCents ?? 0;
    for (const s of projection) {
      if (s.iso <= iso) v = s.bal.get(accId) ?? v;
      else break;
    }
    return v;
  };
  const deltaOn = (accId: number, iso: string) =>
    events
      .filter((e) => e.dateIso === iso)
      .reduce((s, e) => s + e.legs.filter((l) => l.accountId === accId && !legSuppressed(e, l)).reduce((ss, l) => ss + l.deltaCents, 0), 0);

  const shown = accounts.filter((a) => focus === "all" || a.id === focus);

  // date columns = unique event/marker dates, filtered by month
  const dateCols = useMemo(() => {
    const set = new Set<string>();
    events.forEach((e) => set.add(e.dateIso));
    model.markers.forEach((m) => set.add(m.dateIso));
    let ds = [...set].sort();
    if (viewMonth !== "all") ds = ds.filter((d) => monthKey(d) === viewMonth);
    return ds;
  }, [events, model.markers, viewMonth]);

  // group date columns by month for the spanning header band
  const monthBands = useMemo(() => {
    const bands: { key: string; count: number }[] = [];
    dateCols.forEach((d) => {
      const k = monthKey(d);
      const last = bands[bands.length - 1];
      if (last && last.key === k) last.count++;
      else bands.push({ key: k, count: 1 });
    });
    return bands;
  }, [dateCols]);

  const stepMonth = (dir: number) => {
    if (viewMonth === "all") return setViewMonth(months[dir > 0 ? 0 : months.length - 1]);
    const i = months.indexOf(viewMonth);
    const ni = i + dir;
    setViewMonth(ni < 0 || ni >= months.length ? "all" : months[ni]);
  };

  const markersByDate = (iso: string) => model.markers.filter((m) => m.dateIso === iso);
  const eventsOn = (iso: string) => events.filter((e) => e.dateIso === iso);

  const ACC_W = 168;
  const COL_MIN = 64;

  // curve data — each account line plus a bold "Total" (net across all accounts). `_iso` is the
  // point's date for range-filtering (not plotted).
  const curve = useMemo(() => {
    const point = (date: string, iso: string, at: (id: number) => number) => {
      const p: Record<string, number | string> = { date, _iso: iso };
      let total = 0;
      accounts.forEach((a) => {
        const v = at(a.id) / 100;
        p[a.name] = v;
        total += v;
      });
      p.Total = total;
      return p;
    };
    const pts = [point("now", today, (id) => accounts.find((a) => a.id === id)?.balanceCents ?? 0)];
    events.forEach((e) => pts.push(point(fmtDay(e.dateIso).dm, e.dateIso, (id) => balanceAsOf(id, e.dateIso))));
    return pts;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [events, accounts]);

  // apply the time-range filter to the curve
  const curveData = useMemo(() => {
    if (graphRange === "all") return curve;
    const d = new Date(today + "T00:00:00");
    d.setMonth(d.getMonth() + graphRange);
    const cutoff = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    return curve.filter((p) => (p._iso as string) <= cutoff);
  }, [curve, graphRange, today]);

  if (query.isLoading) {
    return <div className="p-10 text-mid text-sm">Loading the forecast…</div>;
  }

  return (
    <div className="flex flex-col xl:flex-row min-h-screen">
      <div className="flex-1 min-w-0 p-6 md:p-8">
        <header className="fade-in mb-4">
          <h1 className="text-4xl mb-1.5">
            <em>Ahead</em>
          </h1>
          <p className="text-mid text-sm">
            Your money, projected forward. Read down an account to see its runway; red is where you break a floor.
            {!live && <span className="pill-grey ml-2 align-middle">preview · mock data</span>}
          </p>
        </header>

        {/* moves to make — the plain-English to-do, above the graph */}
        <ActionPlan text={model.actionPlan ?? ""} onSave={saveActions} />

        {/* month jumper + account focus */}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 mb-4 fade-in-1">
          <div className="flex flex-wrap items-center gap-1">
            <button className="btn-ghost px-1.5" onClick={() => stepMonth(-1)} title="Previous month">
              <ChevronLeft className="size-4" />
            </button>
            <MonthChip active={viewMonth === "all"} onClick={() => setViewMonth("all")}>
              All
            </MonthChip>
            {months.map((m) => (
              <MonthChip key={m} active={viewMonth === m} onClick={() => setViewMonth(m)}>
                {monthLabel(m).replace(/ \d+$/, "")}
              </MonthChip>
            ))}
            <button className="btn-ghost px-1.5" onClick={() => stepMonth(1)} title="Next month">
              <ChevronRight className="size-4" />
            </button>
          </div>
          <span className="text-thin text-mid">·</span>
          <div className="flex flex-wrap gap-1.5">
            <MonthChip active={focus === "all"} onClick={() => setFocus("all")}>
              All accounts
            </MonthChip>
            {accounts.map((a) => (
              <MonthChip key={a.id} active={focus === a.id} onClick={() => setFocus(a.id)}>
                {a.name}
              </MonthChip>
            ))}
          </div>
        </div>

        {/* curve */}
        <div className="card p-3 mb-4 fade-in-2">
          <div className="flex items-center justify-between gap-2 mb-2">
            {/* time range */}
            <div className="inline-flex border border-thin">
              {RANGES.map((r) => (
                <button
                  key={String(r.v)}
                  onClick={() => setGraphRange(r.v)}
                  className={`px-2.5 py-1 text-[11px] font-semibold border-r border-thin last:border-r-0 transition-colors ${
                    graphRange === r.v ? "bg-ink text-cream" : "text-mid hover:text-ink hover:bg-cream/50"
                  }`}
                >
                  {r.l}
                </button>
              ))}
            </div>
            {/* which lines show */}
            <div className="relative">
              <button className="btn-secondary text-xs py-1.5 px-2.5 inline-flex items-center gap-1" onClick={() => setLineMenuOpen((v) => !v)}>
                Lines <ChevronDown className="size-3.5" />
              </button>
              {lineMenuOpen && (
                <>
                  <div className="fixed inset-0 z-10" onClick={() => setLineMenuOpen(false)} />
                  <div className="absolute right-0 mt-1 z-20 card bg-soft p-2 w-44 space-y-0.5">
                    <LineToggle label="Total" color={TOTAL_COLOR} checked={showTotal} onChange={() => setShowTotal((v) => !v)} />
                    {accounts.map((a) => (
                      <LineToggle
                        key={a.id}
                        label={a.name}
                        color={colourOf(a.id)}
                        checked={!hiddenLines.has(a.id)}
                        onChange={() =>
                          setHiddenLines((prev) => {
                            const next = new Set(prev);
                            if (next.has(a.id)) next.delete(a.id);
                            else next.add(a.id);
                            return next;
                          })
                        }
                      />
                    ))}
                  </div>
                </>
              )}
            </div>
          </div>
          <div style={{ height: 200 }} onMouseMove={(e) => (mouseRef.current = { x: e.clientX, y: e.clientY })}>
            <ResponsiveContainer width="100%" height="100%">
              <LineChart data={curveData} margin={{ top: 6, right: 12, bottom: 0, left: 0 }}>
                <CartesianGrid strokeDasharray="3 3" stroke="rgba(42,31,26,0.06)" />
                <XAxis dataKey="date" tick={{ fontSize: 10, fill: "#6b5a52" }} interval="preserveStartEnd" />
                <YAxis tick={{ fontSize: 10, fill: "#6b5a52" }} tickFormatter={(v) => `£${v}`} width={48} />
                <Tooltip
                  isAnimationActive={false}
                  wrapperStyle={{ outline: "none" }}
                  content={<CurveTooltip mouseRef={mouseRef} />}
                />
                <RLegend wrapperStyle={{ fontSize: 11 }} iconType="plainline" />
                <ReferenceLine y={0} stroke="#dc2626" strokeDasharray="4 4" />
                {/* thin per-account lines (only the ticked ones) */}
                {accounts
                  .filter((a) => !hiddenLines.has(a.id))
                  .map((a) => (
                    <Line key={a.id} type="monotone" dataKey={a.name} stroke={colourOf(a.id)} strokeWidth={1.5} dot={false} activeDot={{ r: 3 }} />
                  ))}
                {/* bold Total (net worth across all accounts) drawn on top */}
                {showTotal && <Line type="monotone" dataKey="Total" stroke={TOTAL_COLOR} strokeWidth={3} dot={false} activeDot={{ r: 5 }} />}
              </LineChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* flipped grid — accounts down, days across, full width */}
        <div className="card overflow-auto fade-in-3" style={{ maxHeight: "60vh" }}>
          <table className="border-separate border-spacing-0 text-sm w-full" style={{ minWidth: ACC_W + dateCols.length * COL_MIN }}>
            <thead>
              {/* month band row */}
              <tr>
                <th className="sticky left-0 top-0 z-30 bg-soft border-b border-thin" style={{ width: ACC_W, minWidth: ACC_W }} />
                <th className="sticky top-0 z-20 bg-soft border-b border-l border-thin px-2 py-1 text-center" style={{ minWidth: COL_MIN }}>
                  <span className="text-[9px] uppercase tracking-widest text-mid">now</span>
                </th>
                {monthBands.map((b) => (
                  <th key={b.key} colSpan={b.count} className="sticky top-0 z-20 bg-soft border-b border-l border-thin px-2 py-1 text-left">
                    <span className="text-[10px] uppercase tracking-widest text-mid font-semibold">{monthLabel(b.key)}</span>
                  </th>
                ))}
              </tr>
              {/* day row */}
              <tr>
                <th className="sticky left-0 top-7 z-30 bg-soft border-b border-thin px-3 py-1.5 text-left" style={{ width: ACC_W, minWidth: ACC_W }}>
                  <span className="text-[10px] uppercase tracking-widest text-mid font-semibold">Account</span>
                </th>
                <th className="sticky top-7 z-20 bg-soft border-b border-l border-thin px-1 py-1.5 text-center align-bottom" style={{ minWidth: COL_MIN }}>
                  <span className="text-[10px] text-mid">today</span>
                </th>
                {dateCols.map((d) => {
                  const day = fmtDay(d);
                  const evs = eventsOn(d);
                  const mk = markersByDate(d);
                  const isToday = d === today;
                  return (
                    <th
                      key={d}
                      className={`sticky top-7 z-10 bg-soft border-b border-l border-thin px-1 py-1.5 text-center align-bottom cursor-pointer hover:bg-cream/50 ${isToday ? "ring-1 ring-inset ring-ink" : ""}`}
                      style={{ minWidth: COL_MIN }}
                      onClick={() => setDayOpen(d)}
                      title={evs.map((e) => e.label).join(", ")}
                    >
                      <div className="text-[11px] font-semibold text-ink leading-none">{day.d}</div>
                      <div className="text-[9px] text-mid">{day.dow}</div>
                      <div className="flex items-center justify-center gap-0.5 mt-0.5 h-3">
                        {mk.length > 0 && <AlertTriangle className="size-2.5 text-orange" />}
                        {evs.slice(0, 3).map((e) => (
                          <span key={e.id}>{e.reconciled ? <Check className="size-2.5 text-green" /> : SRC_ICON[e.source]}</span>
                        ))}
                      </div>
                    </th>
                  );
                })}
              </tr>
            </thead>
            <tbody>
              {shown.map((a) => (
                <tr key={a.id} className="hover:bg-cream/30">
                  {/* account name + now */}
                  <td className="sticky left-0 z-10 bg-soft border-b border-thin px-3 py-2" style={{ width: ACC_W, minWidth: ACC_W }}>
                    <div className="flex items-center gap-1.5">
                      {a.kind === "current" ? <Landmark className="size-3.5 text-mid" /> : <CreditCard className="size-3.5 text-mid" />}
                      <span className="font-semibold text-ink truncate">{a.name}</span>
                      {a.isManual && <span className="text-[9px] text-mid">·M</span>}
                    </div>
                    <div className="text-[10px] text-mid">floor {formatMoney(a.floorCents)}</div>
                  </td>
                  {/* now balance */}
                  <td className="border-b border-l border-thin px-1 py-2 text-right">
                    <span className={`mono text-xs font-semibold ${a.balanceCents < 0 ? "text-orange" : "text-ink"}`}>{formatMoney(a.balanceCents)}</span>
                  </td>
                  {dateCols.map((d) => {
                    const bal = balanceAsOf(a.id, d);
                    const delta = deltaOn(a.id, d);
                    const breached = bal < floorOn(a, d);
                    return (
                      <td key={d} className={`border-b border-l border-thin px-1 py-2 text-right ${breached ? "bg-danger/[0.08]" : ""}`}>
                        <div className={`mono text-xs font-semibold leading-none ${breached ? "text-danger" : bal < 0 ? "text-orange" : "text-ink"}`}>
                          {formatMoney(bal)}
                        </div>
                        {delta !== 0 && <div className={`mono text-[9px] mt-0.5 ${delta < 0 ? "text-mid" : "text-green"}`}>{fmtDelta(delta)}</div>}
                      </td>
                    );
                  })}
                </tr>
              ))}
              {dateCols.length === 0 && (
                <tr>
                  <td colSpan={2} className="px-4 py-8 text-sm text-mid">
                    Nothing planned this month.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>

        <button className="btn-ghost text-sm mt-2" onClick={() => setAdding((v) => !v)}>
          <Plus className="size-4" /> Add an event
        </button>
        {adding && <AddEvent accounts={accounts} defaultDate={viewMonth === "all" ? today : `${viewMonth}-01`} onAdd={(b) => { addEvent(b); setAdding(false); }} onClose={() => setAdding(false)} />}

        <Legend />

        {dayOpen && (
          <DayPopover
            dateIso={dayOpen}
            accounts={accounts}
            events={eventsOn(dayOpen)}
            markers={markersByDate(dayOpen)}
            onClose={() => setDayOpen(null)}
            onAdd={addEvent}
            onPatchAmount={patchAmount}
            onDelete={deleteEvent}
          />
        )}
      </div>

      <Rail
        live={live}
        accounts={accounts}
        goals={model.goals}
        onChanged={refresh}
        mock={!live}
        setMock={setMock}
        colourOf={colourOf}
        setColour={setColour}
      />
    </div>
  );
}

// ---- bits -----------------------------------------------------------------

function MonthChip({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={`px-2.5 py-1 text-xs font-medium border transition-colors ${active ? "bg-ink text-cream border-ink" : "border-thin text-mid hover:text-ink hover:bg-cream/50"}`}
    >
      {children}
    </button>
  );
}

function LineToggle({ label, color, checked, onChange }: { label: string; color: string; checked: boolean; onChange: () => void }) {
  return (
    <label className="flex items-center gap-2 px-1 py-0.5 text-xs cursor-pointer hover:bg-cream/50">
      <input type="checkbox" checked={checked} onChange={onChange} />
      <span className="inline-block size-2.5 rounded-full shrink-0" style={{ background: color }} />
      <span className="truncate">{label}</span>
    </label>
  );
}

// "Moves to make" — a plain-English checklist (markdown-ish) above the graph. Bullet lines
// ("- ...") become tick items (tick state is per-browser); other lines are headings/notes.
// Authored by you (pencil) or the assistant (MCP set_action_plan).
const ACTIONS_DONE_KEY = "ahead-actions-done";
function ActionPlan({ text, onSave }: { text: string; onSave: (t: string) => void }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(text);
  const [done, setDone] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(ACTIONS_DONE_KEY) || "[]"));
    } catch {
      return new Set();
    }
  });
  const toggle = (key: string) =>
    setDone((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      try {
        localStorage.setItem(ACTIONS_DONE_KEY, JSON.stringify([...next]));
      } catch {
        /* ignore */
      }
      return next;
    });

  const lines = text.split("\n");
  const hasItems = lines.some((l) => l.trim());

  return (
    <section className="card p-4 mb-4 fade-in">
      <div className="flex items-center justify-between mb-2">
        <h2 className="text-[10px] uppercase tracking-widest text-mid font-semibold flex items-center gap-1.5">
          <ListChecks className="size-3.5" /> Moves to make
        </h2>
        <button className="btn-ghost text-xs px-1.5" onClick={() => { setDraft(text); setEditing((v) => !v); }} title="Edit">
          <Pencil className="size-3.5" />
        </button>
      </div>
      {editing ? (
        <div className="space-y-2">
          <textarea
            className="input text-sm mono"
            rows={8}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={"JULY\n- Mon 14 Jul — move £600 to the overdraft account"}
          />
          <div className="flex gap-2">
            <button className="btn-primary py-1 text-sm" onClick={() => { onSave(draft); setEditing(false); }}>
              Save
            </button>
            <button className="btn-ghost text-sm py-1" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
          <p className="text-[11px] text-mid">Lines starting with “- ” become tick items. Your assistant can rewrite this anytime.</p>
        </div>
      ) : !hasItems ? (
        <p className="text-sm text-mid">
          Nothing here yet — ask your assistant to “plan my moves”, or add your own with the pencil. This is the manual transfers
          and to-dos behind the forecast, in plain English.
        </p>
      ) : (
        <ul className="space-y-1">
          {lines.map((line, i) => {
            const t = line.trim();
            if (!t) return null;
            const m = t.match(/^[-*]\s+(.*)$/);
            if (m) {
              const item = m[1];
              const isDone = done.has(item);
              return (
                <li key={i} className="flex items-start gap-2 text-sm">
                  <input type="checkbox" className="mt-0.5 shrink-0" checked={isDone} onChange={() => toggle(item)} />
                  <span className={isDone ? "line-through text-mid" : "text-ink"}>{item}</span>
                </li>
              );
            }
            return (
              <li key={i} className="text-[10px] uppercase tracking-widest text-mid font-semibold pt-2 first:pt-0">
                {t}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function Legend() {
  return (
    <p className="text-[11px] text-mid mt-3 flex flex-wrap gap-x-4 gap-y-1">
      <span className="inline-flex items-center gap-1"><Check className="size-3 text-green" /> reconciled</span>
      <span className="inline-flex items-center gap-1"><RefreshCw className="size-3" /> auto-detected</span>
      <span className="inline-flex items-center gap-1"><Sparkles className="size-3 text-orange" /> assistant</span>
      <span className="inline-flex items-center gap-1"><ArrowRight className="size-3" /> transfer (one event, two accounts)</span>
      <span className="inline-flex items-center gap-1"><AlertTriangle className="size-3 text-danger" /> below floor</span>
      <span>· click a day for detail</span>
    </p>
  );
}

interface NewEvent {
  dateIso: string;
  label: string;
  accountId?: number;
  toAccountId?: number;
  amountCents: number;
  recurrence?: string;
}

function AddEvent({
  accounts,
  defaultDate,
  onAdd,
  onClose,
}: {
  accounts: AheadAccount[];
  defaultDate: string;
  onAdd: (b: NewEvent) => void;
  onClose: () => void;
}) {
  const [date, setDate] = useState(defaultDate);
  const [label, setLabel] = useState("");
  const [type, setType] = useState<"out" | "in" | "transfer">("out");
  const [accA, setAccA] = useState(accounts[0]?.id ?? 1);
  const [accB, setAccB] = useState(accounts[1]?.id ?? accounts[0]?.id ?? 1);
  const [amount, setAmount] = useState("");
  const [recurrence, setRecurrence] = useState("none");

  const submit = () => {
    const cents = Math.round((parseFloat(amount) || 0) * 100);
    if (!date || !label.trim() || cents <= 0) return;
    onAdd({
      dateIso: date,
      label: label.trim(),
      accountId: accA,
      toAccountId: type === "transfer" ? accB : undefined,
      amountCents: type === "transfer" ? cents : type === "out" ? -cents : cents,
      recurrence,
    });
  };

  return (
    <div className="card p-4 mt-1 space-y-3 fade-in">
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-2">
        <input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        <input className="input md:col-span-2" placeholder="e.g. Rent" value={label} onChange={(e) => setLabel(e.target.value)} />
        <select className="input" value={type} onChange={(e) => setType(e.target.value as typeof type)}>
          <option value="out">money out</option>
          <option value="in">money in</option>
          <option value="transfer">transfer</option>
        </select>
        <select className="input" value={accA} onChange={(e) => setAccA(Number(e.target.value))}>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>{type === "transfer" ? `from ${a.name}` : a.name}</option>
          ))}
        </select>
        {type === "transfer" && (
          <select className="input" value={accB} onChange={(e) => setAccB(Number(e.target.value))}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>to {a.name}</option>
            ))}
          </select>
        )}
        <input className="input" inputMode="decimal" placeholder="£ amount" value={amount} onChange={(e) => setAmount(e.target.value)} />
        <select className="input" value={recurrence} onChange={(e) => setRecurrence(e.target.value)}>
          <option value="none">one-off</option>
          <option value="daily">daily</option>
          <option value="weekly">weekly</option>
          <option value="fortnightly">fortnightly</option>
          <option value="monthly">monthly</option>
          <option value="yearly">yearly</option>
        </select>
      </div>
      <div className="flex gap-2">
        <button className="btn-primary" onClick={submit}>
          <Plus className="size-4" /> Add
        </button>
        <button className="btn-ghost text-sm" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}

function DayPopover({
  dateIso,
  accounts,
  events,
  markers,
  onClose,
  onAdd,
  onPatchAmount,
  onDelete,
}: {
  dateIso: string;
  accounts: AheadAccount[];
  events: AheadEvent[];
  markers: AheadMarker[];
  onClose: () => void;
  onAdd: (b: NewEvent) => void;
  onPatchAmount: (occId: string, accountId: number, cents: number) => void;
  onDelete: (occId: string) => void;
}) {
  const [showAdd, setShowAdd] = useState(events.length === 0);
  const accName = (id: number) => accounts.find((a) => a.id === id)?.name ?? `#${id}`;
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-ink/20 p-4" onClick={onClose}>
      <div className="card bg-soft p-5 w-full max-w-md max-h-[80vh] overflow-auto" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-lg font-bold">{new Date(dateIso + "T00:00:00").toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long" })}</h3>
          <button className="btn-ghost px-1.5" onClick={onClose}><X className="size-4" /></button>
        </div>
        {markers.map((m, i) => (
          <div key={i} className="text-sm text-orange flex items-center gap-1.5 mb-2">
            <AlertTriangle className="size-4" /> {m.label}
          </div>
        ))}
        <ul className="divide-y divide-thin">
          {events.map((e) => (
            <li key={e.id} className="py-2.5">
              <div className="flex items-center gap-2">
                <span>{SRC_ICON[e.source]}</span>
                <span className="font-medium text-sm flex items-center gap-1">
                  {e.legs.length === 2 && <ArrowRight className="size-3 text-mid" />}
                  {e.label}
                  {e.recurrence !== "none" && <RefreshCw className="size-3 text-mid" />}
                </span>
                <button className="ml-auto text-mid hover:text-danger" onClick={() => onDelete(e.id)} title="Delete">
                  <Trash2 className="size-3.5" />
                </button>
              </div>
              <div className="mt-1.5 space-y-1">
                {e.legs.map((l) => (
                  <div key={l.accountId} className="flex items-center gap-2 text-xs">
                    <span className="text-mid w-24 truncate">{accName(l.accountId)}</span>
                    <input
                      className="input py-1 text-right mono text-xs w-28"
                      defaultValue={String(l.deltaCents / 100)}
                      onBlur={(ev) => {
                        const c = Math.round((parseFloat(ev.target.value) || 0) * 100);
                        if (c !== l.deltaCents) onPatchAmount(e.id, l.accountId, c);
                      }}
                    />
                  </div>
                ))}
              </div>
            </li>
          ))}
        </ul>
        {events.length === 0 && !showAdd && <p className="text-sm text-mid py-2">Nothing planned this day.</p>}
        {showAdd ? (
          <div className="mt-2 border-t border-thin pt-3">
            <AddEvent accounts={accounts} defaultDate={dateIso} onAdd={(b) => { onAdd(b); onClose(); }} onClose={() => setShowAdd(false)} />
          </div>
        ) : (
          <button className="btn-ghost text-sm mt-2" onClick={() => setShowAdd(true)}>
            <Plus className="size-4" /> Add on this day
          </button>
        )}
      </div>
    </div>
  );
}

// ---- right rail: accounts/cards + goals -----------------------------------

function Rail({
  live,
  accounts,
  goals,
  onChanged,
  mock,
  setMock,
  colourOf,
  setColour,
}: {
  live: boolean;
  accounts: AheadAccount[];
  goals: AheadGoal[];
  onChanged: () => void;
  mock: boolean;
  setMock: React.Dispatch<React.SetStateAction<Ahead>>;
  colourOf: (id: number) => string;
  setColour: (id: number, hex: string) => void;
}) {
  const [editAcc, setEditAcc] = useState<number | null>(null);
  const [addAcc, setAddAcc] = useState(false);
  const [addGoal, setAddGoal] = useState(false);

  const saveAccount = (id: number, patch: Record<string, unknown>) => {
    if (live) api.patch(`/api/plan/accounts/${id}`, patch).then(onChanged);
    else setMock((m) => ({ ...m, accounts: m.accounts.map((a) => (a.id === id ? applyAcctPatch(a, patch) : a)) }));
    setEditAcc(null);
  };
  const createAccount = (body: Record<string, unknown>) => {
    if (live) api.post("/api/plan/accounts", body).then(onChanged);
    else
      setMock((m) => ({
        ...m,
        accounts: [
          ...m.accounts,
          { id: Date.now(), name: String(body.name), kind: String(body.kind ?? "current"), source: "manual", balanceCents: Number(body.balanceCents ?? 0), currency: "GBP", floorCents: Number(body.floorCents ?? 0), cliffDateIso: null, cliffNewFloorCents: null, creditLimitCents: (body.creditLimitCents as number) ?? null, aprBps: (body.aprBps as number) ?? null, statementDay: null, isManual: true, lowCents: Number(body.balanceCents ?? 0), lowDateIso: "" },
        ],
      }));
    setAddAcc(false);
  };
  const saveGoal = (body: Record<string, unknown>, id?: number) => {
    if (live) {
      if (id) api.patch(`/api/goals/${id}`, body).then(onChanged);
      else api.post("/api/goals", body).then(onChanged);
    } else {
      setMock((m) => ({
        ...m,
        goals: [...m.goals, { id: Date.now(), name: String(body.name), targetCents: Number(body.targetCents), savedCents: Number(body.savedCents ?? 0), sourceAccountId: null, targetDateIso: (body.targetDateIso as string) ?? null, monthlyCents: Number(body.monthlyCents ?? 0) }],
      }));
    }
    setAddGoal(false);
  };
  const deleteGoal = (id: number) => {
    if (live) api.delete(`/api/goals/${id}`).then(onChanged);
    else setMock((m) => ({ ...m, goals: m.goals.filter((g) => g.id !== id) }));
  };

  return (
    <aside className="xl:w-80 shrink-0 border-t xl:border-t-0 xl:border-l border-thin p-6 space-y-6 bg-soft/40">
      <section className="fade-in-2">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-[10px] uppercase tracking-widest text-mid font-semibold">Accounts &amp; cards</h2>
          <button className="btn-ghost text-xs px-1.5" onClick={() => setAddAcc((v) => !v)} title="Add manual account">
            <Plus className="size-3.5" />
          </button>
        </div>
        {addAcc && <AddAccountForm onSave={createAccount} onClose={() => setAddAcc(false)} />}
        <div className="space-y-2.5">
          {accounts.map((a) =>
            editAcc === a.id ? (
              <EditAccountForm key={a.id} a={a} color={colourOf(a.id)} onColor={(hex) => setColour(a.id, hex)} onSave={(p) => saveAccount(a.id, p)} onClose={() => setEditAcc(null)} />
            ) : (
              <div key={a.id} className="card p-3.5">
                <div className="flex items-center gap-2 mb-1">
                  <span className="inline-block size-2.5 rounded-full shrink-0" style={{ background: colourOf(a.id) }} title="line colour" />
                  {a.kind === "current" ? <Landmark className="size-4 text-mid" /> : <CreditCard className="size-4 text-mid" />}
                  <span className="font-semibold text-sm">{a.name}</span>
                  {a.isManual && <span className="pill-grey ml-1">manual</span>}
                  <button className="ml-auto text-mid hover:text-ink" onClick={() => setEditAcc(a.id)} title="Edit">
                    <Pencil className="size-3.5" />
                  </button>
                </div>
                <div className={`mono text-lg font-bold ${a.balanceCents < 0 ? "text-orange" : "text-ink"}`}>{formatMoney(a.balanceCents)}</div>
                {a.creditLimitCents != null && (
                  <div className="mt-1.5">
                    <div className="flex justify-between text-[11px] text-mid mb-1">
                      <span>{Math.round((Math.abs(a.balanceCents) / a.creditLimitCents) * 100)}% used{a.aprBps != null ? ` · APR ${(a.aprBps / 100).toFixed(1)}%` : ""}</span>
                      <span>limit {formatMoney(a.creditLimitCents)}</span>
                    </div>
                    <div className="h-1.5 bg-cream rounded-full overflow-hidden">
                      <div className="h-full bg-orange" style={{ width: `${Math.min(100, (Math.abs(a.balanceCents) / a.creditLimitCents) * 100)}%` }} />
                    </div>
                  </div>
                )}
                {a.cliffDateIso && (
                  <div className="text-[11px] text-orange mt-1.5 flex items-center gap-1">
                    <AlertTriangle className="size-3" /> buffer ends {fmtDay(a.cliffDateIso).dm}
                  </div>
                )}
                <div className={`text-[11px] mt-1.5 ${a.lowCents < floorOn(a, a.lowDateIso) ? "text-danger" : "text-mid"}`}>
                  low {formatMoney(a.lowCents)}{a.lowDateIso ? ` (${fmtDay(a.lowDateIso).dm})` : ""}
                </div>
              </div>
            )
          )}
        </div>
      </section>

      <section className="fade-in-3">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-[10px] uppercase tracking-widest text-mid font-semibold">Goals</h2>
          <button className="btn-ghost text-xs px-1.5" onClick={() => setAddGoal((v) => !v)} title="Add goal">
            <Plus className="size-3.5" />
          </button>
        </div>
        {addGoal && <AddGoalForm onSave={(b) => saveGoal(b)} onClose={() => setAddGoal(false)} />}
        <div className="space-y-2.5">
          {goals.map((g) => {
            const pct = Math.min(100, Math.round((g.savedCents / g.targetCents) * 100));
            return (
              <div key={g.id} className="card p-3.5">
                <div className="flex items-center gap-2 mb-1.5">
                  <PiggyBank className="size-4 text-green" />
                  <span className="font-semibold text-sm">{g.name}</span>
                  <button className="ml-auto text-mid hover:text-danger" onClick={() => deleteGoal(g.id)}>
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
                <div className="flex justify-between mono text-[11px] text-mid mb-1">
                  <span>{formatMoney(g.savedCents)} / {formatMoney(g.targetCents)}</span>
                  <span>{pct}%</span>
                </div>
                <div className="h-1.5 bg-cream rounded-full overflow-hidden">
                  <div className="h-full bg-green" style={{ width: `${pct}%` }} />
                </div>
                <div className="text-[11px] text-mid mt-1.5">
                  {formatMoney(g.monthlyCents)}/mo{g.targetDateIso ? ` · by ${fmtDay(g.targetDateIso).dm} ${new Date(g.targetDateIso).getFullYear()}` : ""}
                </div>
              </div>
            );
          })}
        </div>
      </section>
    </aside>
  );
}

function applyAcctPatch(a: AheadAccount, p: Record<string, unknown>): AheadAccount {
  return {
    ...a,
    floorCents: p.floorCents != null ? Number(p.floorCents) : a.floorCents,
    balanceCents: p.balanceCents != null ? Number(p.balanceCents) : a.balanceCents,
    creditLimitCents: p.creditLimitCents != null ? Number(p.creditLimitCents) : a.creditLimitCents,
    aprBps: p.aprBps != null ? Number(p.aprBps) : a.aprBps,
    cliffDateIso: p.cliffDateIso !== undefined ? ((p.cliffDateIso as string) || null) : a.cliffDateIso,
    cliffNewFloorCents: p.cliffNewFloorCents != null ? Number(p.cliffNewFloorCents) : a.cliffNewFloorCents,
  };
}

function poundsField(v: string): number {
  return Math.round((parseFloat(v) || 0) * 100);
}

function EditAccountForm({
  a,
  color,
  onColor,
  onSave,
  onClose,
}: {
  a: AheadAccount;
  color: string;
  onColor: (hex: string) => void;
  onSave: (p: Record<string, unknown>) => void;
  onClose: () => void;
}) {
  const [floor, setFloor] = useState(String(a.floorCents / 100));
  const [balance, setBalance] = useState(String(a.balanceCents / 100));
  const [limit, setLimit] = useState(a.creditLimitCents != null ? String(a.creditLimitCents / 100) : "");
  const [apr, setApr] = useState(a.aprBps != null ? String(a.aprBps / 100) : "");
  const [cliff, setCliff] = useState(a.cliffDateIso ?? "");
  const [cliffFloor, setCliffFloor] = useState(a.cliffNewFloorCents != null ? String(a.cliffNewFloorCents / 100) : "");
  return (
    <div className="card p-3 space-y-2">
      <p className="text-xs font-semibold">{a.name}</p>
      <div>
        <span className="text-[11px] text-mid">line colour</span>
        <div className="flex flex-wrap gap-1.5 mt-1">
          {PALETTE.map((hex) => (
            <button
              key={hex}
              onClick={() => onColor(hex)}
              className="size-5 rounded-full border transition-transform hover:scale-110"
              style={{ background: hex, borderColor: color === hex ? "#2a1f1a" : "transparent", borderWidth: color === hex ? 2 : 1 }}
              title={hex}
            />
          ))}
        </div>
      </div>
      {a.isManual && (
        <Labeled label="balance £"><input className="input py-1 text-sm" value={balance} onChange={(e) => setBalance(e.target.value)} /></Labeled>
      )}
      <Labeled label="floor £"><input className="input py-1 text-sm" value={floor} onChange={(e) => setFloor(e.target.value)} /></Labeled>
      <Labeled label="credit limit £"><input className="input py-1 text-sm" value={limit} onChange={(e) => setLimit(e.target.value)} /></Labeled>
      <Labeled label="APR %"><input className="input py-1 text-sm" value={apr} onChange={(e) => setApr(e.target.value)} /></Labeled>
      <Labeled label="0% ends"><input className="input py-1 text-sm" type="date" value={cliff} onChange={(e) => setCliff(e.target.value)} /></Labeled>
      {cliff && <Labeled label="new floor £"><input className="input py-1 text-sm" value={cliffFloor} onChange={(e) => setCliffFloor(e.target.value)} /></Labeled>}
      <div className="flex gap-2 pt-1">
        <button
          className="btn-primary py-1 text-sm"
          onClick={() =>
            onSave({
              floorCents: poundsField(floor),
              ...(a.isManual ? { balanceCents: poundsField(balance) } : {}),
              creditLimitCents: limit ? poundsField(limit) : null,
              aprBps: apr ? Math.round(parseFloat(apr) * 100) : null,
              cliffDateIso: cliff,
              cliffNewFloorCents: cliff ? poundsField(cliffFloor) : null,
            })
          }
        >
          Save
        </button>
        <button className="btn-ghost text-sm py-1" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}

function AddAccountForm({ onSave, onClose }: { onSave: (b: Record<string, unknown>) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState("credit");
  const [balance, setBalance] = useState("");
  const [limit, setLimit] = useState("");
  const [apr, setApr] = useState("");
  return (
    <div className="card p-3 space-y-2 mb-2.5">
      <input className="input py-1 text-sm" placeholder="name e.g. credit card" value={name} onChange={(e) => setName(e.target.value)} />
      <select className="input py-1 text-sm" value={kind} onChange={(e) => setKind(e.target.value)}>
        <option value="current">current</option>
        <option value="savings">savings</option>
        <option value="credit">credit card</option>
        <option value="cash">cash</option>
      </select>
      <Labeled label="balance £"><input className="input py-1 text-sm" placeholder="-410" value={balance} onChange={(e) => setBalance(e.target.value)} /></Labeled>
      <Labeled label="credit limit £"><input className="input py-1 text-sm" value={limit} onChange={(e) => setLimit(e.target.value)} /></Labeled>
      <Labeled label="APR %"><input className="input py-1 text-sm" value={apr} onChange={(e) => setApr(e.target.value)} /></Labeled>
      <div className="flex gap-2 pt-1">
        <button
          className="btn-primary py-1 text-sm"
          onClick={() => name.trim() && onSave({ name: name.trim(), kind, balanceCents: poundsField(balance), creditLimitCents: limit ? poundsField(limit) : null, aprBps: apr ? Math.round(parseFloat(apr) * 100) : null })}
        >
          Add
        </button>
        <button className="btn-ghost text-sm py-1" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}

function AddGoalForm({ onSave, onClose }: { onSave: (b: Record<string, unknown>) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [saved, setSaved] = useState("");
  const [monthly, setMonthly] = useState("");
  const [date, setDate] = useState("");
  return (
    <div className="card p-3 space-y-2 mb-2.5">
      <input className="input py-1 text-sm" placeholder="goal e.g. Car fund" value={name} onChange={(e) => setName(e.target.value)} />
      <Labeled label="target £"><input className="input py-1 text-sm" value={target} onChange={(e) => setTarget(e.target.value)} /></Labeled>
      <Labeled label="saved £"><input className="input py-1 text-sm" value={saved} onChange={(e) => setSaved(e.target.value)} /></Labeled>
      <Labeled label="per month £"><input className="input py-1 text-sm" value={monthly} onChange={(e) => setMonthly(e.target.value)} /></Labeled>
      <Labeled label="by date"><input className="input py-1 text-sm" type="date" value={date} onChange={(e) => setDate(e.target.value)} /></Labeled>
      <div className="flex gap-2 pt-1">
        <button
          className="btn-primary py-1 text-sm"
          onClick={() => name.trim() && target && onSave({ name: name.trim(), targetCents: poundsField(target), savedCents: poundsField(saved), monthlyCents: poundsField(monthly), targetDateIso: date })}
        >
          Add
        </button>
        <button className="btn-ghost text-sm py-1" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}

function Labeled({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex items-center gap-2">
      <span className="text-[11px] text-mid w-24 shrink-0">{label}</span>
      {children}
    </label>
  );
}
