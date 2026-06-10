-- Remove the deal/price watchlist feature. The 0011 migration is left intact so existing DBs keep
-- a consistent migration history; this migration drops the tables it created. Children first.
DROP TABLE IF EXISTS deal_observations;
DROP TABLE IF EXISTS watchlist_sources;
DROP TABLE IF EXISTS watchlist_items;
