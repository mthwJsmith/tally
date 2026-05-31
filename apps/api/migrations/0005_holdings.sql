-- 0005: stock / fund / ETF holdings + activities + price quotes.
-- Wealthfolio's data model, clean-room reimplemented under MIT.
-- Manual entry first; price fetch from Yahoo Finance unofficial endpoint.

-- A logical broker / portfolio container. Lets the user separate Lightyear, AJ Bell, etc.
CREATE TABLE brokers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,                 -- 'Lightyear', 'AJ Bell SIPP', etc.
    kind TEXT NOT NULL DEFAULT 'general',      -- 'general' | 'isa' | 'sipp' | 'crypto'
    currency TEXT NOT NULL DEFAULT 'GBP',
    notes TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

-- Aggregated current position in a single instrument at a single broker.
-- (Computed by replaying holding_activities, but materialised here for fast UI reads.)
CREATE TABLE holdings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    broker_id INTEGER NOT NULL REFERENCES brokers(id) ON DELETE CASCADE,
    symbol TEXT NOT NULL,                      -- 'AAPL', 'VWRP.L', 'BTC-USD'
    asset_class TEXT NOT NULL DEFAULT 'equity',-- 'equity' | 'etf' | 'fund' | 'bond' | 'crypto' | 'cash'
    quantity REAL NOT NULL DEFAULT 0,
    avg_cost_per_unit REAL,                    -- weighted avg cost basis
    currency TEXT NOT NULL DEFAULT 'GBP',
    name TEXT,                                 -- 'Apple Inc.', resolved from market data
    last_synced_at INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(broker_id, symbol)
);
CREATE INDEX idx_holdings_broker ON holdings(broker_id);
CREATE INDEX idx_holdings_symbol ON holdings(symbol);

-- Event log: buys, sells, dividends, splits, transfers in/out, fees.
-- holdings row is recomputed from these.
CREATE TABLE holding_activities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    holding_id INTEGER NOT NULL REFERENCES holdings(id) ON DELETE CASCADE,
    activity_type TEXT NOT NULL,               -- 'BUY' | 'SELL' | 'DIVIDEND' | 'SPLIT' | 'TRANSFER_IN' | 'TRANSFER_OUT' | 'FEE' | 'INTEREST'
    timestamp INTEGER NOT NULL,
    quantity REAL NOT NULL DEFAULT 0,
    price_per_unit REAL,                       -- price at the time of the event
    fee REAL NOT NULL DEFAULT 0,
    currency TEXT NOT NULL DEFAULT 'GBP',
    notes TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_activities_holding ON holding_activities(holding_id, timestamp DESC);

-- Cache of current + historical prices from market-data provider (Yahoo Finance).
CREATE TABLE quotes (
    symbol TEXT NOT NULL,
    timestamp INTEGER NOT NULL,                -- unix seconds (00:00 UTC for daily close)
    price REAL NOT NULL,
    currency TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'yahoo',
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (symbol, timestamp)
);
CREATE INDEX idx_quotes_recent ON quotes(symbol, timestamp DESC);

-- Latest snapshot per symbol — convenience view that the UI hits.
CREATE TABLE latest_quotes (
    symbol TEXT PRIMARY KEY,
    price REAL NOT NULL,
    currency TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    -- for change % display
    previous_close REAL,
    day_change_pct REAL,
    company_name TEXT
);
