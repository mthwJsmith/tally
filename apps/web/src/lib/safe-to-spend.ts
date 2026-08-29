// "Safe to Spend" — forward-looking runway to the next payday.
//
// Answers one question: "how much can I spend today without wrecking the plan?"
// It is deliberately distinct from the Budgets feature (backward-looking per-category caps).
// This is pure + side-effect-free (except the localStorage load/save helpers) so it can be
// unit-tested and reasoned about in isolation.
//
// The headline idea is the per-account FLOOR: money sitting below an account's floor is never
// counted as spendable. A current account defaults to a £0 floor (spend down to empty). An
// arranged-overdraft account can be given a negative floor (e.g. -£1000 = a 0% buffer line), and
// that floor can flip on a CLIFF date (e.g. the day the 0% overdraft ends and interest kicks in).

import type { Account, Bill } from "@/types/api";

export type PaydayConfig =
  | { kind: "lastWorkingDay" }
  | { kind: "dayOfMonth"; day: number };

export interface Cliff {
  accountId: number;
  /** ISO date (YYYY-MM-DD). On/after this local date, the account's floor becomes newFloorCents. */
  dateIso: string;
  newFloorCents: number;
}

export interface SafeToSpendConfig {
  payday: PaydayConfig;
  /** accountId -> floor in cents (may be negative for overdraft accounts). Missing = £0 floor. */
  floorsCents: Record<number, number>;
  cliffs: Cliff[];
  /** Extra money to set aside before dividing (planned debt paydown, etc.). */
  ringfenceCents: number;
  /** False until the user has set this up at least once; the tile shows a setup prompt instead. */
  configured: boolean;
}

export const DEFAULT_CONFIG: SafeToSpendConfig = {
  payday: { kind: "lastWorkingDay" },
  floorsCents: {},
  cliffs: [],
  ringfenceCents: 0,
  configured: false,
};

const STORAGE_KEY = "tally.safeToSpend";

export function loadConfig(): SafeToSpendConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_CONFIG;
    return { ...DEFAULT_CONFIG, ...(JSON.parse(raw) as Partial<SafeToSpendConfig>) };
  } catch {
    return DEFAULT_CONFIG;
  }
}

export function saveConfig(cfg: SafeToSpendConfig): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
  } catch {
    // localStorage unavailable (private mode etc.) — non-fatal, config just won't persist.
  }
}

// ---- date helpers (all local time) ------------------------------------------------------------

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

const DAY_MS = 86_400_000;

/** Last Mon–Fri of the given month. month is 0-based. */
function lastWorkingDayOfMonth(year: number, month: number): Date {
  const d = new Date(year, month + 1, 0); // last calendar day of the month
  while (d.getDay() === 0 || d.getDay() === 6) d.setDate(d.getDate() - 1);
  return startOfDay(d);
}

/** The next payday strictly after `now` (a payday that falls on today rolls to next month). */
export function nextPayday(cfg: SafeToSpendConfig, now: Date): Date {
  const today = startOfDay(now);
  const build = (year: number, month: number): Date => {
    if (cfg.payday.kind === "lastWorkingDay") return lastWorkingDayOfMonth(year, month);
    const lastDay = new Date(year, month + 1, 0).getDate();
    return startOfDay(new Date(year, month, Math.min(cfg.payday.day, lastDay)));
  };
  const thisMonth = build(today.getFullYear(), today.getMonth());
  if (thisMonth.getTime() > today.getTime()) return thisMonth;
  return build(today.getFullYear(), today.getMonth() + 1);
}

// ---- floor resolution -------------------------------------------------------------------------

/** The active floor (cents) for an account, applying the latest cliff that has already passed. */
export function floorFor(account: Account, cfg: SafeToSpendConfig, now: Date): number {
  let floor = cfg.floorsCents[account.id] ?? 0;
  const today = startOfDay(now).getTime();
  let bestCliff = -Infinity;
  for (const c of cfg.cliffs) {
    if (c.accountId !== account.id) continue;
    const t = startOfDay(new Date(c.dateIso + "T00:00:00")).getTime();
    if (t <= today && t > bestCliff) {
      bestCliff = t;
      floor = c.newFloorCents;
    }
  }
  return floor;
}

// ---- the calculation --------------------------------------------------------------------------

export interface PerAccountSpendable {
  accountId: number;
  name: string;
  currentCents: number;
  floorCents: number;
  spendableCents: number;
}

export interface SafeToSpendResult {
  safeTodayCents: number;
  safePerDayCents: number;
  spendableNowCents: number;
  committedCents: number;
  ringfenceCents: number;
  freeCents: number;
  daysLeft: number;
  nextPaydayUnix: number;
  perAccount: PerAccountSpendable[];
}

function accountLabel(a: Account): string {
  return a.custom_display_name ?? a.consent_nickname ?? a.display_name;
}

/** Committed direct-debit / bill outflows between now and payday, deduped by name. */
export function committedBeforePayday(bills: Bill[], paydayUnix: number, now: Date): number {
  const nowUnix = Math.floor(startOfDay(now).getTime() / 1000);
  const seen = new Set<string>();
  let total = 0;
  for (const b of bills) {
    if (b.next_expected_date == null) continue;
    if (b.next_expected_date < nowUnix || b.next_expected_date > paydayUnix) continue;
    const key = b.name.replace(/\s*\(DD\)$/, "").toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    total += b.expected_amount_max_cents > 0
      ? b.expected_amount_max_cents
      : b.expected_amount_min_cents;
  }
  return total;
}

export function computeSafeToSpend(
  accounts: Account[],
  bills: Bill[],
  todaySpentCents: number,
  cfg: SafeToSpendConfig,
  now: Date
): SafeToSpendResult {
  const payday = nextPayday(cfg, now);
  const nextPaydayUnix = Math.floor(payday.getTime() / 1000);
  const daysLeft = Math.max(
    1,
    Math.ceil((payday.getTime() - startOfDay(now).getTime()) / DAY_MS)
  );

  const perAccount: PerAccountSpendable[] = [];
  let spendableNowCents = 0;
  for (const a of accounts) {
    if (a.kind !== "account" || a.current_balance_cents == null) continue;
    const floorCents = floorFor(a, cfg, now);
    const spendable = Math.max(0, a.current_balance_cents - floorCents);
    spendableNowCents += spendable;
    perAccount.push({
      accountId: a.id,
      name: accountLabel(a),
      currentCents: a.current_balance_cents,
      floorCents,
      spendableCents: spendable,
    });
  }

  const committedCents = committedBeforePayday(bills, nextPaydayUnix, now);
  const freeCents = spendableNowCents - committedCents - cfg.ringfenceCents;
  const safePerDayCents = Math.floor(freeCents / daysLeft);
  const safeTodayCents = safePerDayCents - todaySpentCents;

  return {
    safeTodayCents,
    safePerDayCents,
    spendableNowCents,
    committedCents,
    ringfenceCents: cfg.ringfenceCents,
    freeCents,
    daysLeft,
    nextPaydayUnix,
    perAccount,
  };
}

// ---- "can I afford this?" ----------------------------------------------------------------------

export interface AffordResult {
  ok: boolean;
  newSafePerDayCents: number;
  /** How far the purchase pushes the chosen account below its floor (>0 = breach). */
  floorBreachCents: number;
  reason: string;
}

export function affordCheck(
  amountCents: number,
  fromAccountId: number | null,
  accounts: Account[],
  bills: Bill[],
  todaySpentCents: number,
  cfg: SafeToSpendConfig,
  now: Date
): AffordResult {
  const base = computeSafeToSpend(accounts, bills, todaySpentCents, cfg, now);
  const newFree = base.freeCents - amountCents;
  const newSafePerDayCents = Math.floor(newFree / base.daysLeft);

  let floorBreachCents = 0;
  let breachName = "";
  if (fromAccountId != null) {
    const a = accounts.find((x) => x.id === fromAccountId);
    if (a && a.current_balance_cents != null) {
      const floor = floorFor(a, cfg, now);
      const after = a.current_balance_cents - amountCents;
      if (after < floor) {
        floorBreachCents = floor - after;
        breachName = accountLabel(a);
      }
    }
  }

  const ok = newSafePerDayCents >= 0 && floorBreachCents <= 0;
  let reason: string;
  if (floorBreachCents > 0) {
    reason = `Breaches your ${breachName} floor by ${pounds(floorBreachCents)}.`;
  } else if (newSafePerDayCents < 0) {
    reason = `Would put you ${pounds(-newSafePerDayCents)}/day in the red until payday.`;
  } else {
    reason = `Leaves you ${pounds(newSafePerDayCents)}/day until payday.`;
  }
  return { ok, newSafePerDayCents, floorBreachCents, reason };
}

function pounds(cents: number): string {
  return new Intl.NumberFormat("en-GB", { style: "currency", currency: "GBP" }).format(
    cents / 100
  );
}
