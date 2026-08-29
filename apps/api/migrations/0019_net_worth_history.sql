-- 0019: daily net-worth history — the whole-position curve over time.
--
-- balance_snapshots (0014) tracks per-plan-account cash history for the Ahead graph;
-- this table tracks the HEADLINE numbers (cash, debt, investments incl. pension, net)
-- once per day so the dashboard can chart net worth and chats can answer "am I up
-- this month?". Written by the scheduler daily + at app startup (upsert by day, so
-- restarts are harmless). No historical backfill: the series grows from install day.
CREATE TABLE net_worth_history (
    day               TEXT PRIMARY KEY,      -- ISO date (YYYY-MM-DD, UTC)
    cash_cents        INTEGER NOT NULL,
    debt_cents        INTEGER NOT NULL,      -- positive = owed
    investments_cents INTEGER NOT NULL,      -- GBP, FX-converted, includes pension
    pension_cents     INTEGER NOT NULL,      -- the sipp-broker subset of investments
    net_cents         INTEGER NOT NULL,      -- cash + investments - debt
    created_at        INTEGER NOT NULL
);

DROP VIEW IF EXISTS v_net_worth_history;
CREATE VIEW v_net_worth_history AS
SELECT day,
       ROUND(cash_cents / 100.0, 2)          AS cash_pounds,
       ROUND(debt_cents / 100.0, 2)          AS debt_pounds,
       ROUND(investments_cents / 100.0, 2)   AS investments_pounds,
       ROUND(pension_cents / 100.0, 2)       AS pension_pounds,
       ROUND(net_cents / 100.0, 2)           AS net_pounds
FROM net_worth_history;
