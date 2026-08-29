-- 0013: the planning / "Ahead" layer.
--
-- Three tables that turn Tally from "what happened" into "what's going to happen":
--   * plan_accounts  — a unified account list: synced TrueLayer accounts (balance read
--                      live from `accounts`) PLUS manual ones Open Banking can't reach
--                      (e.g. an Aqua credit card), each carrying planning metadata.
--   * plan_events    — dated money events on the forecast: planned/expected cashflows and
--                      transfers between accounts. These are the inputs the Ahead grid
--                      computes running balances from, and they reconcile against real
--                      transactions as those land.
--   * goals          — savings targets with a monthly contribution + progress.

-- A unified planning account. `source='synced'` links to a real `accounts` row and reads
-- its balance live; `source='manual'` is maintained here (by the user or the assistant).
CREATE TABLE plan_accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'current',          -- current | savings | credit | cash
    source TEXT NOT NULL DEFAULT 'manual',         -- synced | manual
    linked_account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    balance_cents INTEGER NOT NULL DEFAULT 0,      -- authoritative for manual; cache for synced
    currency TEXT NOT NULL DEFAULT 'GBP',
    floor_cents INTEGER NOT NULL DEFAULT 0,        -- lowest allowed; negative = overdraft buffer
    cliff_date TEXT,                               -- ISO date; on/after it the floor flips
    cliff_new_floor_cents INTEGER,
    credit_limit_cents INTEGER,                    -- for credit cards
    apr_bps INTEGER,                               -- basis points: 3490 = 34.90%
    statement_day INTEGER,                         -- day-of-month the statement issues
    payment_intent TEXT,                           -- pay_in_full | revolve (planning hint)
    balance_updated_at INTEGER,
    sort_order INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(linked_account_id)
);
CREATE INDEX idx_plan_accounts_enabled ON plan_accounts(enabled);

-- A dated event on the forecast.
--   normal:   account_id set, to_account_id NULL, amount_cents SIGNED (- out, + in)
--   transfer: account_id + to_account_id set, amount_cents POSITIVE magnitude
--             (moves from account_id into to_account_id)
--   note:     account_id NULL, amount_cents 0 (a dated annotation)
-- Recurrence is expanded forward at read time, never materialised as rows.
CREATE TABLE plan_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,                            -- ISO date (YYYY-MM-DD) of first occurrence
    label TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'planned',        -- actual | auto | planned | llm
    account_id INTEGER REFERENCES plan_accounts(id) ON DELETE CASCADE,
    to_account_id INTEGER REFERENCES plan_accounts(id) ON DELETE CASCADE,
    amount_cents INTEGER NOT NULL DEFAULT 0,
    recurrence TEXT NOT NULL DEFAULT 'none',       -- none | weekly | fortnightly | monthly | yearly
    recur_until TEXT,                              -- ISO date; NULL = open-ended
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    match_regex TEXT,                              -- reconcile against real transaction descriptions
    matched_txn_id INTEGER REFERENCES transactions(id) ON DELETE SET NULL,
    note TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_plan_events_date ON plan_events(date);

CREATE TABLE goals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    target_cents INTEGER NOT NULL,
    saved_cents INTEGER NOT NULL DEFAULT 0,        -- manual progress, or mirror a source account
    source_account_id INTEGER REFERENCES plan_accounts(id) ON DELETE SET NULL,
    target_date TEXT,                              -- ISO date
    monthly_cents INTEGER NOT NULL DEFAULT 0,      -- planned monthly contribution
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
