-- 0014: daily balance history per planning account.
--
-- The "Ahead" graph could only ever show the FUTURE (the forecast). This adds the PAST: one
-- balance row per planning account per day, so the curve can show where you came from — e.g.
-- the pre-payday trough — not just where you're going.
--
-- Filled two ways:
--   * going forward — a snapshot is written on every bank sync (and covers manual accounts too);
--   * backfilled    — synced current/savings accounts are reconstructed from stored transactions
--                     (current balance minus the transactions posted since each past day).
CREATE TABLE balance_snapshots (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_account_id INTEGER NOT NULL REFERENCES plan_accounts(id) ON DELETE CASCADE,
    day             TEXT NOT NULL,             -- ISO date (YYYY-MM-DD)
    balance_cents   INTEGER NOT NULL,          -- sign-adjusted like forecast_balance (credit = negative)
    created_at      INTEGER NOT NULL,
    UNIQUE(plan_account_id, day)
);
CREATE INDEX idx_balance_snapshots_day ON balance_snapshots(day);
