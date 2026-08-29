-- 0016: read-only reporting views for the MCP `query` tool.
--
-- These exist so an LLM can answer long-tail questions with deterministic SQL instead of
-- mental arithmetic. Conventions the views enforce so the model never has to convert:
--   * every money column is decimal POUNDS, named *_pounds (never pence)
--   * every date is an ISO string (never unix seconds)
--   * category / account / broker names are joined in (never bare ids)
--   * disabled rows are filtered out

DROP VIEW IF EXISTS v_accounts;
CREATE VIEW v_accounts AS
SELECT a.id,
       COALESCE(a.custom_display_name, c.nickname, a.display_name) AS name,
       CASE a.kind WHEN 'card' THEN 'credit_card' ELSE 'current_account' END AS type,
       -- credit_card rows: positive = amount owed (bank convention)
       ROUND(COALESCE(a.current_balance_cents, 0) / 100.0, 2)      AS balance_pounds,
       ROUND(a.available_balance_cents / 100.0, 2)                 AS available_pounds,
       ROUND(a.overdraft_cents / 100.0, 2)                         AS overdraft_limit_pounds,
       ROUND(a.credit_limit_cents / 100.0, 2)                      AS credit_limit_pounds,
       ROUND(a.payment_due_cents / 100.0, 2)                       AS payment_due_pounds,
       a.payment_due_date,
       a.currency,
       date(a.balance_updated_at, 'unixepoch')                     AS balance_updated
FROM accounts a
JOIN consents c ON c.id = a.consent_id
WHERE a.enabled = 1 AND c.enabled = 1;

DROP VIEW IF EXISTS v_transactions;
CREATE VIEW v_transactions AS
SELECT t.id,
       date(t.timestamp, 'unixepoch')                              AS date,
       t.description,
       t.merchant_name,
       CASE WHEN t.is_credit = 1 THEN 'in' ELSE 'out' END          AS direction,
       ROUND(t.amount_cents / 100.0, 2)                            AS amount_pounds,
       -- signed: money in positive, money out negative (useful for SUM)
       ROUND(CASE WHEN t.is_credit = 1 THEN t.amount_cents
                  ELSE -t.amount_cents END / 100.0, 2)             AS signed_pounds,
       t.is_pending,
       cat.name                                                    AS category,
       COALESCE(a.custom_display_name, co.nickname, a.display_name) AS account,
       t.currency
FROM transactions t
LEFT JOIN categories cat ON cat.id = t.category_id
LEFT JOIN accounts a     ON a.id = t.account_id
LEFT JOIN consents co    ON co.id = a.consent_id;

DROP VIEW IF EXISTS v_plan_accounts;
CREATE VIEW v_plan_accounts AS
SELECT p.id, p.name, p.kind, p.source,
       -- forecast sign convention: negative = owed / overdrawn (synced cards report what
       -- you owe as positive, so flip them; manual accounts are stored signed already)
       ROUND((CASE WHEN p.source = 'synced' AND p.kind = 'credit'
                   THEN -abs(COALESCE(a.current_balance_cents, p.balance_cents))
                   WHEN p.source = 'synced'
                   THEN COALESCE(a.current_balance_cents, p.balance_cents)
                   ELSE p.balance_cents END) / 100.0, 2)           AS balance_pounds,
       ROUND(p.floor_cents / 100.0, 2)                             AS floor_pounds,
       ROUND(p.credit_limit_cents / 100.0, 2)                      AS credit_limit_pounds,
       p.apr_bps / 100.0                                           AS apr_percent,
       p.statement_day,
       p.cliff_date,
       ROUND(p.cliff_new_floor_cents / 100.0, 2)                   AS cliff_new_floor_pounds
FROM plan_accounts p
LEFT JOIN accounts a ON a.id = p.linked_account_id
WHERE p.enabled = 1;

DROP VIEW IF EXISTS v_plan_events;
CREATE VIEW v_plan_events AS
SELECT e.id,
       e.date,
       e.label,
       e.source,
       ROUND(e.amount_cents / 100.0, 2)                            AS signed_pounds,
       ROUND(abs(e.amount_cents) / 100.0, 2)                       AS amount_pounds,
       CASE WHEN e.to_account_id IS NOT NULL THEN 'transfer'
            WHEN e.amount_cents >= 0 THEN 'in' ELSE 'out' END      AS direction,
       pa.name                                                     AS account,
       pb.name                                                     AS to_account,
       e.recurrence,
       e.recur_until,
       e.note
FROM plan_events e
LEFT JOIN plan_accounts pa ON pa.id = e.account_id
LEFT JOIN plan_accounts pb ON pb.id = e.to_account_id
WHERE e.enabled = 1;

DROP VIEW IF EXISTS v_bills;
CREATE VIEW v_bills AS
SELECT b.id, b.name,
       ROUND(b.expected_amount_min_cents / 100.0, 2)               AS expected_min_pounds,
       ROUND(b.expected_amount_max_cents / 100.0, 2)               AS expected_max_pounds,
       b.repeat_freq,
       date(b.next_expected_date, 'unixepoch')                     AS next_due,
       date(b.last_paid_date, 'unixepoch')                         AS last_paid,
       b.currency
FROM bills b
WHERE b.enabled = 1;

DROP VIEW IF EXISTS v_goals;
CREATE VIEW v_goals AS
SELECT g.id, g.name,
       ROUND(g.target_cents / 100.0, 2)                            AS target_pounds,
       ROUND(g.saved_cents / 100.0, 2)                             AS saved_pounds,
       ROUND(g.monthly_cents / 100.0, 2)                           AS monthly_pounds,
       g.target_date
FROM goals g
WHERE g.enabled = 1;

DROP VIEW IF EXISTS v_holdings;
CREATE VIEW v_holdings AS
SELECT h.id,
       b.name                                                      AS broker,
       h.symbol,
       COALESCE(h.name, q.company_name, h.symbol)                  AS name,
       h.quantity,
       h.avg_cost_per_unit,
       q.price                                                     AS last_price,
       ROUND(h.quantity * q.price, 2)                              AS market_value_pounds,
       ROUND(h.quantity * h.avg_cost_per_unit, 2)                  AS cost_basis_pounds,
       h.currency
FROM holdings h
JOIN brokers b ON b.id = h.broker_id
LEFT JOIN latest_quotes q ON q.symbol = h.symbol
WHERE h.enabled = 1;

DROP VIEW IF EXISTS v_categories;
CREATE VIEW v_categories AS
SELECT id, name, parent_id FROM categories;
