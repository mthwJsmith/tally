-- FX-aware v_holdings: market value and cost basis converted to GBP using the FX
-- rows cached in latest_quotes (symbol '<CCY>GBP=X', price = pounds per 1 CCY).
-- Falls back to 1:1 when no rate is cached yet — same behaviour as before this view
-- existed. GBp pence prices never reach the DB (normalised in the Yahoo client).

DROP VIEW IF EXISTS v_holdings;
CREATE VIEW v_holdings AS
SELECT h.id,
       b.name                                                      AS broker,
       h.symbol,
       COALESCE(h.name, q.company_name, h.symbol)                  AS name,
       h.quantity,
       h.avg_cost_per_unit,
       q.price                                                     AS last_price,
       ROUND(h.quantity * q.price *
             CASE WHEN q.currency = 'GBP' OR q.currency IS NULL THEN 1.0
                  ELSE COALESCE(qfx.price, 1.0) END, 2)            AS market_value_pounds,
       ROUND(h.quantity * h.avg_cost_per_unit *
             CASE WHEN h.currency = 'GBP' THEN 1.0
                  ELSE COALESCE(hfx.price, 1.0) END, 2)            AS cost_basis_pounds,
       h.currency
FROM holdings h
JOIN brokers b ON b.id = h.broker_id
LEFT JOIN latest_quotes q   ON q.symbol = h.symbol
LEFT JOIN latest_quotes qfx ON qfx.symbol = q.currency || 'GBP=X'
LEFT JOIN latest_quotes hfx ON hfx.symbol = h.currency || 'GBP=X'
WHERE h.enabled = 1;
