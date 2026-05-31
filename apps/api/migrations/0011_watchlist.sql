-- Deal watchlist: items the user wants, each watched via one or more sources (free RSS feeds
-- such as HotUKDeals / CamelCamelCamel, or a changedetection.io watch). Observations are the
-- individual deals/prices found; the unique (item_id, guid) index dedups so we alert once.
CREATE TABLE watchlist_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    keywords TEXT,                          -- optional extra match terms
    target_price_cents INTEGER,             -- alert when a found price <= this
    currency TEXT NOT NULL DEFAULT 'GBP',
    archived INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE watchlist_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id INTEGER NOT NULL REFERENCES watchlist_items(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,                     -- 'rss' | 'changedetection'
    ref TEXT NOT NULL,                      -- RSS feed URL, or changedetection watch UUID
    created_at INTEGER NOT NULL
);

CREATE TABLE deal_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id INTEGER NOT NULL REFERENCES watchlist_items(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    url TEXT,
    price_cents INTEGER,                    -- parsed if available
    source_kind TEXT NOT NULL,
    guid TEXT NOT NULL,                     -- dedup key (RSS guid, or cd.io uuid+timestamp)
    found_at INTEGER NOT NULL,
    notified INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX idx_deal_obs_dedup ON deal_observations(item_id, guid);
CREATE INDEX idx_deal_obs_item ON deal_observations(item_id, found_at);
