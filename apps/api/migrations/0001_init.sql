-- tally: initial schema
-- Multi-bank UK Open Banking → Firefly III sync via TrueLayer

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- One row per linked bank/card consent.
-- access_token + refresh_token are encrypted with ChaCha20-Poly1305 keyed by TALLY_MASTER_KEY env var.
CREATE TABLE consents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    nickname TEXT UNIQUE NOT NULL,           -- e.g. "Nationwide", "Santander", "Aqua" -- chosen by user
    credentials_id TEXT NOT NULL,             -- TrueLayer JWT sub
    provider_id TEXT,                         -- e.g. "uk-ob-nationwide", "uk-oauth-aqua"
    provider_display_name TEXT,
    access_token_enc BLOB NOT NULL,           -- ChaCha20-Poly1305 ciphertext
    access_token_nonce BLOB NOT NULL,         -- 12-byte nonce
    refresh_token_enc BLOB NOT NULL,
    refresh_token_nonce BLOB NOT NULL,
    expires_at INTEGER NOT NULL,              -- unix ts of access_token expiry
    consent_expires_at INTEGER,                -- unix ts of 90-day PSD2 deadline
    scopes TEXT NOT NULL,                      -- space-separated scope list at consent time
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_sync_at INTEGER,
    last_sync_status TEXT,                     -- 'success' | 'fail' | 'in_progress' | NULL
    last_sync_error TEXT,
    enabled INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_consents_enabled ON consents(enabled) WHERE enabled = 1;

-- Cache of TrueLayer accounts/cards per consent, mapped to Firefly asset accounts.
CREATE TABLE accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    consent_id INTEGER NOT NULL REFERENCES consents(id) ON DELETE CASCADE,
    truelayer_id TEXT NOT NULL,                -- account_id or card_id
    kind TEXT NOT NULL,                        -- 'account' | 'card'
    display_name TEXT NOT NULL,
    iban TEXT,
    sort_code TEXT,
    account_number TEXT,
    card_last4 TEXT,
    currency TEXT NOT NULL DEFAULT 'GBP',
    firefly_account_id INTEGER,                -- mapped to Firefly asset account; NULL = unmapped yet
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(consent_id, truelayer_id)
);

-- Idempotency layer. Every TrueLayer transaction we've processed gets a row here.
-- If we see the same truelayer_txn_id again, we skip it. Period.
-- This is the proper fix for issue #265 in upstream truelayer2firefly.
CREATE TABLE transactions_seen (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    truelayer_txn_id TEXT NOT NULL,
    firefly_txn_id INTEGER,                    -- populated after successful Firefly POST
    is_pending INTEGER NOT NULL DEFAULT 0,     -- 1 if pulled from /transactions/pending
    raw_json TEXT,                             -- for debugging / future re-process
    imported_at INTEGER NOT NULL,
    UNIQUE(account_id, truelayer_txn_id)
);

CREATE INDEX idx_txn_seen_account ON transactions_seen(account_id);

-- Standing orders + direct debits cache. Re-fetched each sync, used to populate Firefly Bills.
CREATE TABLE recurring (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    truelayer_id TEXT NOT NULL,                -- standing_order_id or direct_debit_id
    kind TEXT NOT NULL,                        -- 'standing_order' | 'direct_debit'
    name TEXT NOT NULL,
    amount REAL,
    currency TEXT,
    frequency TEXT,                            -- 'WEEKLY' | 'MONTHLY' | 'YEARLY' etc., null if inferred
    next_payment_date TEXT,                    -- ISO date
    status TEXT,                               -- 'ACTIVE' | 'INACTIVE' etc.
    firefly_bill_id INTEGER,                   -- mapped to Firefly Bill
    last_seen_at INTEGER NOT NULL,
    UNIQUE(account_id, truelayer_id, kind)
);

-- Audit log of every sync attempt
CREATE TABLE sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    consent_id INTEGER REFERENCES consents(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    status TEXT NOT NULL,                      -- 'success' | 'fail' | 'partial'
    accounts_synced INTEGER DEFAULT 0,
    transactions_imported INTEGER DEFAULT 0,
    transactions_skipped INTEGER DEFAULT 0,    -- already seen
    recurring_imported INTEGER DEFAULT 0,
    error_message TEXT
);

CREATE INDEX idx_sync_log_consent ON sync_log(consent_id, started_at DESC);

-- Transient OAuth state during the consent-add flow
CREATE TABLE oauth_states (
    state TEXT PRIMARY KEY,
    nickname TEXT NOT NULL,                    -- the consent nickname being created
    created_at INTEGER NOT NULL
);
