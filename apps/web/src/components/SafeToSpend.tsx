import { useMemo, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Settings2, Check, X, Wallet } from "lucide-react";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import type { Account, Bill, TransactionsListResponse } from "@/types/api";
import {
  DEFAULT_CONFIG,
  computeSafeToSpend,
  affordCheck,
  nextPayday,
  type SafeToSpendConfig,
} from "@/lib/safe-to-spend";

// Forward-looking "how much can I spend today" tile for the dashboard. Reads existing endpoints
// (accounts, bills, today's transactions) — no backend changes. Config lives in localStorage.

export function SafeToSpend() {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [buyInput, setBuyInput] = useState("");
  const [buyFrom, setBuyFrom] = useState<number | "">("");

  // Config is persisted server-side so the dashboard, the MCP tool and the Telegram ping all agree.
  const cfgQuery = useQuery({
    queryKey: ["sts-config"],
    queryFn: () => api.get<SafeToSpendConfig>("/api/safe-to-spend/config"),
  });
  const cfg = cfgQuery.data ?? DEFAULT_CONFIG;
  const saveMutation = useMutation({
    mutationFn: (next: SafeToSpendConfig) => api.put("/api/safe-to-spend/config", next),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sts-config"] }),
  });

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const bills = useQuery({
    queryKey: ["bills", "all"],
    queryFn: () => api.get<{ bills: Bill[] }>("/api/bills"),
  });
  const todaySpend = useQuery({
    queryKey: ["txns", "today-spend"],
    queryFn: async () => {
      const now = new Date();
      const from = Math.floor(
        new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000
      );
      const to = Math.floor(Date.now() / 1000);
      const txns = await api.get<TransactionsListResponse>(
        `/api/transactions?from=${from}&to=${to}&is_credit=false&limit=1000`
      );
      return txns.transactions.reduce((acc, t) => acc + t.amount_cents, 0);
    },
  });

  const acctList = accounts.data?.accounts ?? [];
  const billList = bills.data?.bills ?? [];
  const spentToday = todaySpend.data ?? 0;
  const spendAccounts = acctList.filter((a) => a.kind === "account");

  const result = useMemo(
    () => computeSafeToSpend(acctList, billList, spentToday, cfg, new Date()),
    [acctList, billList, spentToday, cfg]
  );

  const buyCents = Math.round((parseFloat(buyInput) || 0) * 100);
  const afford = useMemo(() => {
    if (buyCents <= 0) return null;
    return affordCheck(
      buyCents,
      buyFrom === "" ? null : buyFrom,
      acctList,
      billList,
      spentToday,
      cfg,
      new Date()
    );
  }, [buyCents, buyFrom, acctList, billList, spentToday, cfg]);

  const safe = result.safeTodayCents;
  const tone = safe < 0 ? "text-danger" : safe < 1000 ? "text-orange" : "text-green";
  const paydayLabel = nextPayday(cfg, new Date()).toLocaleDateString("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
  });

  const loading =
    accounts.isLoading || bills.isLoading || todaySpend.isLoading || cfgQuery.isLoading;

  return (
    <section className="card p-7 col-span-12 lg:col-span-7">
      <div className="flex items-start justify-between mb-3">
        <p className="text-[10px] uppercase tracking-widest text-mid">Safe to spend today</p>
        <button
          className="btn-ghost text-xs"
          onClick={() => setEditing((e) => !e)}
          title="Configure payday, floors and ring-fencing"
          aria-label="Configure safe to spend"
        >
          <Settings2 className="size-4" />
        </button>
      </div>

      {!cfg.configured ? (
        <SetupPrompt onStart={() => setEditing(true)} />
      ) : loading ? (
        <p className="mono text-mid text-sm">Crunching…</p>
      ) : (
        <>
          <p className={`font-extrabold text-5xl mono tracking-tight ${tone}`}>
            {safe < 0 ? "−" : ""}
            {formatMoney(Math.abs(safe))}
          </p>
          <p className="text-xs text-mid mt-1.5">
            {result.safePerDayCents < 0 ? "−" : ""}
            {formatMoney(Math.abs(result.safePerDayCents))}/day · {result.daysLeft} day
            {result.daysLeft === 1 ? "" : "s"} until payday · {paydayLabel}
          </p>

          {/* breakdown */}
          <div className="grid grid-cols-3 gap-2 mt-4 text-center">
            <Stat label="Spendable now" cents={result.spendableNowCents} />
            <Stat label="Ring-fenced" cents={-(result.committedCents + result.ringfenceCents)} />
            <Stat label="Free" cents={result.freeCents} emphasise />
          </div>

          {/* buy-check */}
          <div className="mt-5 pt-4 border-t border-thin">
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Can I afford this?</p>
            <div className="flex gap-2">
              <input
                className="input flex-1"
                inputMode="decimal"
                placeholder="£ amount"
                value={buyInput}
                onChange={(e) => setBuyInput(e.target.value)}
              />
              {spendAccounts.length > 1 && (
                <select
                  className="input w-40"
                  value={buyFrom}
                  onChange={(e) => setBuyFrom(e.target.value ? Number(e.target.value) : "")}
                >
                  <option value="">any account</option>
                  {spendAccounts.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.custom_display_name ?? a.consent_nickname ?? a.display_name}
                    </option>
                  ))}
                </select>
              )}
            </div>
            {afford && (
              <div
                className={`flex items-start gap-2 mt-2.5 text-sm font-medium ${
                  afford.ok ? "text-green" : "text-danger"
                }`}
              >
                {afford.ok ? (
                  <Check className="size-4 mt-0.5 shrink-0" />
                ) : (
                  <X className="size-4 mt-0.5 shrink-0" />
                )}
                <span>
                  {afford.ok ? "Yes. " : "No. "}
                  {afford.reason}
                </span>
              </div>
            )}
          </div>
        </>
      )}

      {editing && (
        <ConfigEditor
          cfg={cfg}
          accounts={spendAccounts}
          onSave={(next) => {
            saveMutation.mutate({ ...next, configured: true });
            setEditing(false);
          }}
          onClose={() => setEditing(false)}
        />
      )}
    </section>
  );
}

function Stat({
  label,
  cents,
  emphasise,
}: {
  label: string;
  cents: number;
  emphasise?: boolean;
}) {
  return (
    <div className="bg-cream/40 border border-thin rounded p-2.5">
      <p className="text-[9px] uppercase tracking-widest text-mid">{label}</p>
      <p
        className={`mono mt-1 ${emphasise ? "font-extrabold text-base" : "font-semibold text-sm"} ${
          cents < 0 ? "text-danger" : "text-ink"
        }`}
      >
        {cents < 0 ? "−" : ""}
        {formatMoney(Math.abs(cents))}
      </p>
    </div>
  );
}

function SetupPrompt({ onStart }: { onStart: () => void }) {
  return (
    <div className="py-4">
      <Wallet className="size-6 text-mid mb-3" />
      <p className="text-sm text-mid mb-4 max-w-md">
        Set your payday and (optionally) an overdraft floor so Tally can tell you exactly how much
        is safe to spend today without breaking your plan.
      </p>
      <button className="btn-cta inline-flex" onClick={onStart}>
        <Settings2 className="size-4" /> Set it up
      </button>
    </div>
  );
}

function ConfigEditor({
  cfg,
  accounts,
  onSave,
  onClose,
}: {
  cfg: SafeToSpendConfig;
  accounts: Account[];
  onSave: (cfg: SafeToSpendConfig) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState<SafeToSpendConfig>(cfg);

  const setFloor = (id: number, pounds: string) => {
    const floors = { ...draft.floorsCents };
    if (pounds.trim() === "") delete floors[id];
    else floors[id] = Math.round(parseFloat(pounds) * 100);
    setDraft({ ...draft, floorsCents: floors });
  };
  const cliffFor = (id: number) => draft.cliffs.find((c) => c.accountId === id);
  const setCliff = (id: number, dateIso: string, newFloorPounds: string) => {
    const others = draft.cliffs.filter((c) => c.accountId !== id);
    if (!dateIso) {
      setDraft({ ...draft, cliffs: others });
      return;
    }
    setDraft({
      ...draft,
      cliffs: [
        ...others,
        { accountId: id, dateIso, newFloorCents: Math.round((parseFloat(newFloorPounds) || 0) * 100) },
      ],
    });
  };

  return (
    <div className="mt-5 pt-5 border-t border-thin space-y-5">
      {/* payday */}
      <div>
        <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
          Payday
        </label>
        <div className="flex gap-2 mt-1.5">
          <select
            className="input"
            value={draft.payday.kind}
            onChange={(e) =>
              setDraft({
                ...draft,
                payday:
                  e.target.value === "lastWorkingDay"
                    ? { kind: "lastWorkingDay" }
                    : { kind: "dayOfMonth", day: 28 },
              })
            }
          >
            <option value="lastWorkingDay">Last working day of month</option>
            <option value="dayOfMonth">A day of the month</option>
          </select>
          {draft.payday.kind === "dayOfMonth" && (
            <input
              className="input w-24"
              type="number"
              min={1}
              max={31}
              value={draft.payday.day}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  payday: { kind: "dayOfMonth", day: Number(e.target.value) || 1 },
                })
              }
            />
          )}
        </div>
      </div>

      {/* floors */}
      <div>
        <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
          Account floors (leave blank = £0)
        </label>
        <p className="text-[11px] text-mid mt-0.5 mb-2">
          The lowest balance you'll let an account reach. Use a negative number for an overdraft
          buffer line (e.g. −1000). A cliff date flips the floor when 0% overdraft ends.
        </p>
        <div className="space-y-2.5">
          {accounts.map((a) => {
            const cliff = cliffFor(a.id);
            return (
              <div key={a.id} className="bg-cream/40 border border-thin rounded p-2.5">
                <p className="text-xs font-semibold mb-1.5">
                  {a.custom_display_name ?? a.consent_nickname ?? a.display_name}
                </p>
                <div className="flex flex-wrap gap-2 items-center">
                  <input
                    className="input w-28"
                    inputMode="decimal"
                    placeholder="floor £"
                    value={
                      draft.floorsCents[a.id] != null
                        ? String(draft.floorsCents[a.id] / 100)
                        : ""
                    }
                    onChange={(e) => setFloor(a.id, e.target.value)}
                  />
                  <span className="text-[11px] text-mid">cliff:</span>
                  <input
                    className="input w-36"
                    type="date"
                    value={cliff?.dateIso ?? ""}
                    onChange={(e) =>
                      setCliff(
                        a.id,
                        e.target.value,
                        cliff ? String(cliff.newFloorCents / 100) : "0"
                      )
                    }
                  />
                  {cliff && (
                    <input
                      className="input w-28"
                      inputMode="decimal"
                      placeholder="new floor £"
                      value={String(cliff.newFloorCents / 100)}
                      onChange={(e) => setCliff(a.id, cliff.dateIso, e.target.value)}
                    />
                  )}
                </div>
              </div>
            );
          })}
          {accounts.length === 0 && (
            <p className="text-xs text-mid">No current accounts linked yet.</p>
          )}
        </div>
      </div>

      {/* ring-fence */}
      <div>
        <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
          Also set aside (debt paydown etc.)
        </label>
        <input
          className="input w-40 mt-1.5"
          inputMode="decimal"
          placeholder="£ 0"
          value={draft.ringfenceCents ? String(draft.ringfenceCents / 100) : ""}
          onChange={(e) =>
            setDraft({
              ...draft,
              ringfenceCents: Math.round((parseFloat(e.target.value) || 0) * 100),
            })
          }
        />
      </div>

      <div className="flex gap-2">
        <button className="btn-primary" onClick={() => onSave(draft)}>
          <Check className="size-4" /> Save
        </button>
        <button className="btn-ghost text-sm" onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}
