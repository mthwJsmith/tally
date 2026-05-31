-- 0002: full transaction content + categories + tags + rules + budgets + bills.
-- This is what turns tally from "sync pipe" into "full personal finance OS".

-- Full transaction content. Until this migration we only stored IDs in transactions_seen
-- for dedup; now we keep the whole record so the UI can display + filter without re-fetching
-- from the provider.
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_txn_id TEXT NOT NULL,             -- TrueLayer/Plaid transaction_id (stable across syncs)
    timestamp INTEGER NOT NULL,                -- unix seconds
    description TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,             -- integer to avoid float drift; always positive
    currency TEXT NOT NULL,
    is_credit INTEGER NOT NULL,                -- 1 = money in (deposit), 0 = money out (withdrawal)
    is_pending INTEGER NOT NULL DEFAULT 0,
    merchant_name TEXT,
    counterparty_iban TEXT,
    counterparty_name TEXT,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    notes TEXT,
    raw_json TEXT,                             -- full provider response for forensics / re-process
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(account_id, provider_txn_id)
);
CREATE INDEX idx_tx_account_ts ON transactions(account_id, timestamp DESC);
CREATE INDEX idx_tx_timestamp ON transactions(timestamp DESC);

CREATE TABLE categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    icon TEXT,                                 -- emoji or icon name
    colour TEXT,                               -- CSS colour hex
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_categories_parent ON categories(parent_id);

CREATE TABLE tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE transaction_tags (
    transaction_id INTEGER NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (transaction_id, tag_id)
);
CREATE INDEX idx_txntags_tag ON transaction_tags(tag_id);

-- The rule engine. Each row = one rule. Matched in priority order (asc). Each rule
-- can match on any subset of fields (NULL = wildcard) and apply any subset of effects.
CREATE TABLE rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 100,     -- lower = applied first
    -- Match (any null = wildcard):
    match_description_regex TEXT,
    match_merchant_regex TEXT,
    match_min_amount_cents INTEGER,
    match_max_amount_cents INTEGER,
    match_account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    match_is_credit INTEGER,                   -- 1/0/NULL
    -- Effect:
    set_category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    add_tag_ids TEXT,                          -- JSON array of tag ids
    set_notes TEXT,
    -- Bookkeeping:
    times_applied INTEGER NOT NULL DEFAULT 0,
    last_applied_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_rules_priority ON rules(enabled, priority);

CREATE TABLE budgets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    amount_cents INTEGER NOT NULL,
    period TEXT NOT NULL,                      -- 'monthly' | 'weekly' | 'yearly'
    currency TEXT NOT NULL DEFAULT 'GBP',
    rollover INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE bills (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    expected_amount_min_cents INTEGER NOT NULL,
    expected_amount_max_cents INTEGER NOT NULL,
    currency TEXT NOT NULL DEFAULT 'GBP',
    repeat_freq TEXT NOT NULL,                 -- 'weekly' | 'monthly' | 'fortnightly' | 'yearly'
    next_expected_date INTEGER,
    last_paid_date INTEGER,
    match_description_regex TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    source_recurring_id INTEGER REFERENCES recurring(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_bills_enabled ON bills(enabled);

CREATE TABLE bill_payments (
    bill_id INTEGER NOT NULL REFERENCES bills(id) ON DELETE CASCADE,
    transaction_id INTEGER NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    PRIMARY KEY (bill_id, transaction_id)
);

-- Seed default categories. `icon` is now a Lucide icon name, not an emoji.
INSERT INTO categories (name, icon, colour, created_at) VALUES
    ('Groceries',     'ShoppingCart',  '#505E4D', strftime('%s','now')),
    ('Eating Out',    'UtensilsCrossed','#c67e5b', strftime('%s','now')),
    ('Transport',     'Car',           '#6B7A63', strftime('%s','now')),
    ('Fuel',          'Fuel',          '#646e64', strftime('%s','now')),
    ('Rent',          'Home',          '#b86843', strftime('%s','now')),
    ('Bills',         'Receipt',       '#505E4D', strftime('%s','now')),
    ('Subscriptions', 'RotateCw',      '#6B7A63', strftime('%s','now')),
    ('Income',        'TrendingUp',    '#505E4D', strftime('%s','now')),
    ('Transfer',      'ArrowLeftRight','#9ca39c', strftime('%s','now')),
    ('Other',         'HelpCircle',    '#9ca39c', strftime('%s','now'));
