-- 0006: per-account balance snapshots, card metadata, and user-overrideable labels.
-- Balance comes from /data/v1/accounts/{id}/balance and /data/v1/cards/{id}/balance
-- which we previously had OAuth scope for but never called.

ALTER TABLE accounts ADD COLUMN current_balance_cents INTEGER;
ALTER TABLE accounts ADD COLUMN available_balance_cents INTEGER;
ALTER TABLE accounts ADD COLUMN overdraft_cents INTEGER;
ALTER TABLE accounts ADD COLUMN credit_limit_cents INTEGER;
ALTER TABLE accounts ADD COLUMN last_statement_balance_cents INTEGER;
ALTER TABLE accounts ADD COLUMN last_statement_date TEXT;
ALTER TABLE accounts ADD COLUMN payment_due_cents INTEGER;
ALTER TABLE accounts ADD COLUMN payment_due_date TEXT;
ALTER TABLE accounts ADD COLUMN account_type TEXT;        -- TL: TRANSACTION/SAVINGS/BUSINESS_TRANSACTION/BUSINESS_SAVINGS
ALTER TABLE accounts ADD COLUMN card_network TEXT;        -- VISA/MASTERCARD/AMEX
ALTER TABLE accounts ADD COLUMN name_on_card TEXT;
ALTER TABLE accounts ADD COLUMN custom_display_name TEXT; -- user override; if NULL we fall back to display_name
ALTER TABLE accounts ADD COLUMN balance_updated_at INTEGER;
