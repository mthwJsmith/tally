//! Database layer. Wraps sqlx + encrypts/decrypts secrets transparently.

use crate::crypto::Crypto;
use crate::models::*;
use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use std::str::FromStr;

#[derive(Clone)]
pub struct Db {
    pub pool: SqlitePool,
    pub crypto: Crypto,
}

impl Db {
    pub async fn connect(database_url: &str, crypto: Crypto) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .log_statements(tracing::log::LevelFilter::Debug);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .context("connecting to sqlite")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;
        Ok(Self { pool, crypto })
    }

    // ---------- settings ----------

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- consents ----------

    pub async fn list_consents(&self) -> Result<Vec<Consent>> {
        Ok(sqlx::query_as::<_, Consent>(
            "SELECT * FROM consents ORDER BY nickname",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_enabled_consents(&self) -> Result<Vec<Consent>> {
        Ok(sqlx::query_as::<_, Consent>(
            "SELECT * FROM consents WHERE enabled = 1 ORDER BY nickname",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_consent(&self, id: i64) -> Result<Option<Consent>> {
        Ok(sqlx::query_as::<_, Consent>("SELECT * FROM consents WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn get_consent_by_nickname(&self, nickname: &str) -> Result<Option<Consent>> {
        Ok(
            sqlx::query_as::<_, Consent>("SELECT * FROM consents WHERE nickname = ?")
                .bind(nickname)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Insert a new consent or fully replace an existing one (matched by nickname).
    /// Encrypts the tokens before write.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_consent(
        &self,
        nickname: &str,
        credentials_id: &str,
        provider_id: Option<&str>,
        provider_display_name: Option<&str>,
        access_token: &str,
        refresh_token: &str,
        expires_at: i64,
        consent_expires_at: Option<i64>,
        scopes: &str,
    ) -> Result<Consent> {
        let (atok_nonce, atok_ct) = self.crypto.encrypt(access_token)?;
        let (rtok_nonce, rtok_ct) = self.crypto.encrypt(refresh_token)?;
        let now = Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO consents (nickname, credentials_id, provider_id, provider_display_name,
                access_token_enc, access_token_nonce, refresh_token_enc, refresh_token_nonce,
                expires_at, consent_expires_at, scopes, created_at, updated_at, enabled)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
             ON CONFLICT(nickname) DO UPDATE SET
                credentials_id = excluded.credentials_id,
                provider_id = excluded.provider_id,
                provider_display_name = excluded.provider_display_name,
                access_token_enc = excluded.access_token_enc,
                access_token_nonce = excluded.access_token_nonce,
                refresh_token_enc = excluded.refresh_token_enc,
                refresh_token_nonce = excluded.refresh_token_nonce,
                expires_at = excluded.expires_at,
                consent_expires_at = excluded.consent_expires_at,
                scopes = excluded.scopes,
                updated_at = excluded.updated_at,
                enabled = 1",
        )
        .bind(nickname)
        .bind(credentials_id)
        .bind(provider_id)
        .bind(provider_display_name)
        .bind(&atok_ct)
        .bind(&atok_nonce)
        .bind(&rtok_ct)
        .bind(&rtok_nonce)
        .bind(expires_at)
        .bind(consent_expires_at)
        .bind(scopes)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.get_consent_by_nickname(nickname)
            .await?
            .context("consent disappeared after upsert")
    }

    /// Update tokens after a successful refresh.
    pub async fn update_tokens(
        &self,
        consent_id: i64,
        access_token: &str,
        refresh_token: &str,
        expires_at: i64,
    ) -> Result<()> {
        let (atok_nonce, atok_ct) = self.crypto.encrypt(access_token)?;
        let (rtok_nonce, rtok_ct) = self.crypto.encrypt(refresh_token)?;
        sqlx::query(
            "UPDATE consents SET
                access_token_enc = ?, access_token_nonce = ?,
                refresh_token_enc = ?, refresh_token_nonce = ?,
                expires_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&atok_ct)
        .bind(&atok_nonce)
        .bind(&rtok_ct)
        .bind(&rtok_nonce)
        .bind(expires_at)
        .bind(Utc::now().timestamp())
        .bind(consent_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Decrypt access token for a consent.
    pub fn decrypt_access_token(&self, c: &Consent) -> Result<String> {
        self.crypto
            .decrypt(&c.access_token_nonce, &c.access_token_enc)
    }

    pub fn decrypt_refresh_token(&self, c: &Consent) -> Result<String> {
        self.crypto
            .decrypt(&c.refresh_token_nonce, &c.refresh_token_enc)
    }

    pub async fn set_consent_enabled(&self, consent_id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE consents SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(if enabled { 1 } else { 0 })
            .bind(Utc::now().timestamp())
            .bind(consent_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_consent(&self, consent_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM consents WHERE id = ?")
            .bind(consent_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_consent_sync_status(
        &self,
        consent_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE consents SET last_sync_at = ?, last_sync_status = ?, last_sync_error = ? WHERE id = ?",
        )
        .bind(Utc::now().timestamp())
        .bind(status)
        .bind(error)
        .bind(consent_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- accounts ----------

    pub async fn list_accounts_for_consent(&self, consent_id: i64) -> Result<Vec<Account>> {
        Ok(sqlx::query_as::<_, Account>(
            "SELECT * FROM accounts WHERE consent_id = ? ORDER BY kind, display_name",
        )
        .bind(consent_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_all_enabled_accounts(&self) -> Result<Vec<Account>> {
        Ok(sqlx::query_as::<_, Account>(
            "SELECT a.*, c.nickname AS consent_nickname,
                (SELECT COALESCE(SUM(CASE WHEN t.is_credit = 1 THEN t.amount_cents
                                          ELSE -t.amount_cents END), 0)
                 FROM transactions t WHERE t.account_id = a.id AND t.is_pending = 1)
                    AS pending_net_cents
             FROM accounts a
             INNER JOIN consents c ON c.id = a.consent_id
             WHERE a.enabled = 1 AND c.enabled = 1
             ORDER BY a.kind, a.display_name",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_account(
        &self,
        consent_id: i64,
        truelayer_id: &str,
        kind: &str,
        display_name: &str,
        iban: Option<&str>,
        sort_code: Option<&str>,
        account_number: Option<&str>,
        card_last4: Option<&str>,
        currency: &str,
    ) -> Result<Account> {
        let now = Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO accounts (consent_id, truelayer_id, kind, display_name, iban, sort_code,
                account_number, card_last4, currency, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
             ON CONFLICT(consent_id, truelayer_id) DO UPDATE SET
                display_name = excluded.display_name,
                iban = excluded.iban,
                sort_code = excluded.sort_code,
                account_number = excluded.account_number,
                card_last4 = excluded.card_last4,
                currency = excluded.currency,
                updated_at = excluded.updated_at",
        )
        .bind(consent_id)
        .bind(truelayer_id)
        .bind(kind)
        .bind(display_name)
        .bind(iban)
        .bind(sort_code)
        .bind(account_number)
        .bind(card_last4)
        .bind(currency)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(sqlx::query_as::<_, Account>(
            "SELECT * FROM accounts WHERE consent_id = ? AND truelayer_id = ?",
        )
        .bind(consent_id)
        .bind(truelayer_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn map_account_to_firefly(
        &self,
        account_id: i64,
        firefly_account_id: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE accounts SET firefly_account_id = ?, updated_at = ? WHERE id = ?",
        )
        .bind(firefly_account_id)
        .bind(Utc::now().timestamp())
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Snapshot the balance for a bank account.
    pub async fn update_account_balance(
        &self,
        account_id: i64,
        current_cents: i64,
        available_cents: Option<i64>,
        overdraft_cents: Option<i64>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        sqlx::query(
            "UPDATE accounts SET
                current_balance_cents = ?,
                available_balance_cents = ?,
                overdraft_cents = ?,
                balance_updated_at = ?,
                updated_at = ?
             WHERE id = ?",
        )
        .bind(current_cents)
        .bind(available_cents)
        .bind(overdraft_cents)
        .bind(now)
        .bind(now)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Snapshot the balance for a credit card — strictly richer than account balance.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_card_balance(
        &self,
        account_id: i64,
        current_cents: i64,
        available_cents: Option<i64>,
        credit_limit_cents: Option<i64>,
        last_statement_balance_cents: Option<i64>,
        last_statement_date: Option<&str>,
        payment_due_cents: Option<i64>,
        payment_due_date: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        sqlx::query(
            "UPDATE accounts SET
                current_balance_cents = ?,
                available_balance_cents = ?,
                credit_limit_cents = ?,
                last_statement_balance_cents = ?,
                last_statement_date = ?,
                payment_due_cents = ?,
                payment_due_date = ?,
                balance_updated_at = ?,
                updated_at = ?
             WHERE id = ?",
        )
        .bind(current_cents)
        .bind(available_cents)
        .bind(credit_limit_cents)
        .bind(last_statement_balance_cents)
        .bind(last_statement_date)
        .bind(payment_due_cents)
        .bind(payment_due_date)
        .bind(now)
        .bind(now)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One-off metadata bump separate from `upsert_account` (which would otherwise need 14 params).
    pub async fn update_account_metadata(
        &self,
        account_id: i64,
        account_type: Option<&str>,
        card_network: Option<&str>,
        name_on_card: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE accounts SET
                account_type = COALESCE(?, account_type),
                card_network = COALESCE(?, card_network),
                name_on_card = COALESCE(?, name_on_card),
                updated_at = ?
             WHERE id = ?",
        )
        .bind(account_type)
        .bind(card_network)
        .bind(name_on_card)
        .bind(Utc::now().timestamp())
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// User-overridable label that wins over TrueLayer's display_name in the UI. Pass None
    /// to clear back to TL-provided.
    pub async fn set_account_custom_display_name(
        &self,
        account_id: i64,
        custom: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE accounts SET custom_display_name = ?, updated_at = ? WHERE id = ?",
        )
        .bind(custom)
        .bind(Utc::now().timestamp())
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rename a consent's user-visible label. Same sanitisation as on create.
    pub async fn rename_consent(&self, id: i64, new_nickname: &str) -> Result<()> {
        let cleaned = new_nickname
            .trim()
            .to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "-");
        if cleaned.is_empty() {
            return Err(anyhow::anyhow!("nickname cannot be empty"));
        }
        sqlx::query("UPDATE consents SET nickname = ?, updated_at = ? WHERE id = ?")
            .bind(&cleaned)
            .bind(Utc::now().timestamp())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- transactions_seen ----------

    pub async fn is_txn_seen(&self, account_id: i64, truelayer_txn_id: &str) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM transactions_seen WHERE account_id = ? AND truelayer_txn_id = ?",
        )
        .bind(account_id)
        .bind(truelayer_txn_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    pub async fn record_txn_imported(
        &self,
        account_id: i64,
        truelayer_txn_id: &str,
        firefly_txn_id: Option<i64>,
        is_pending: bool,
        raw_json: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO transactions_seen (account_id, truelayer_txn_id, firefly_txn_id, is_pending, raw_json, imported_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(account_id, truelayer_txn_id) DO UPDATE SET
                firefly_txn_id = COALESCE(excluded.firefly_txn_id, transactions_seen.firefly_txn_id),
                is_pending = excluded.is_pending",
        )
        .bind(account_id)
        .bind(truelayer_txn_id)
        .bind(firefly_txn_id)
        .bind(if is_pending { 1 } else { 0 })
        .bind(raw_json)
        .bind(Utc::now().timestamp())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- recurring ----------

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_recurring(
        &self,
        account_id: i64,
        truelayer_id: &str,
        kind: &str,
        name: &str,
        amount: Option<f64>,
        currency: Option<&str>,
        frequency: Option<&str>,
        next_payment_date: Option<&str>,
        status: Option<&str>,
    ) -> Result<RecurringEntry> {
        sqlx::query(
            "INSERT INTO recurring (account_id, truelayer_id, kind, name, amount, currency,
                frequency, next_payment_date, status, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(account_id, truelayer_id, kind) DO UPDATE SET
                name = excluded.name,
                amount = excluded.amount,
                currency = excluded.currency,
                frequency = excluded.frequency,
                next_payment_date = excluded.next_payment_date,
                status = excluded.status,
                last_seen_at = excluded.last_seen_at",
        )
        .bind(account_id)
        .bind(truelayer_id)
        .bind(kind)
        .bind(name)
        .bind(amount)
        .bind(currency)
        .bind(frequency)
        .bind(next_payment_date)
        .bind(status)
        .bind(Utc::now().timestamp())
        .execute(&self.pool)
        .await?;

        Ok(sqlx::query_as::<_, RecurringEntry>(
            "SELECT * FROM recurring WHERE account_id = ? AND truelayer_id = ? AND kind = ?",
        )
        .bind(account_id)
        .bind(truelayer_id)
        .bind(kind)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn map_recurring_to_firefly_bill(
        &self,
        recurring_id: i64,
        firefly_bill_id: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE recurring SET firefly_bill_id = ? WHERE id = ?")
            .bind(firefly_bill_id)
            .bind(recurring_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_recurring_for_account(&self, account_id: i64) -> Result<Vec<RecurringEntry>> {
        Ok(sqlx::query_as::<_, RecurringEntry>(
            "SELECT * FROM recurring WHERE account_id = ? ORDER BY kind, name",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?)
    }

    // ---------- sync_log ----------

    pub async fn start_sync_log(&self, consent_id: Option<i64>) -> Result<i64> {
        let result = sqlx::query(
            "INSERT INTO sync_log (consent_id, started_at, status) VALUES (?, ?, 'in_progress')",
        )
        .bind(consent_id)
        .bind(Utc::now().timestamp())
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_sync_log(
        &self,
        log_id: i64,
        status: &str,
        accounts: i64,
        imported: i64,
        skipped: i64,
        recurring: i64,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sync_log SET ended_at = ?, status = ?, accounts_synced = ?,
                transactions_imported = ?, transactions_skipped = ?,
                recurring_imported = ?, error_message = ?
             WHERE id = ?",
        )
        .bind(Utc::now().timestamp())
        .bind(status)
        .bind(accounts)
        .bind(imported)
        .bind(skipped)
        .bind(recurring)
        .bind(error)
        .bind(log_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn recent_sync_logs(&self, limit: i64) -> Result<Vec<SyncLogEntry>> {
        Ok(sqlx::query_as::<_, SyncLogEntry>(
            "SELECT * FROM sync_log ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    // ---------- oauth_states ----------

    pub async fn save_oauth_state(&self, state: &str, nickname: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO oauth_states (state, nickname, created_at) VALUES (?, ?, ?)",
        )
        .bind(state)
        .bind(nickname)
        .bind(Utc::now().timestamp())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn consume_oauth_state(&self, state: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT nickname FROM oauth_states WHERE state = ?",
        )
        .bind(state)
        .fetch_optional(&self.pool)
        .await?;
        if row.is_some() {
            sqlx::query("DELETE FROM oauth_states WHERE state = ?")
                .bind(state)
                .execute(&self.pool)
                .await?;
            // Garbage-collect old states (> 10 min)
            let cutoff = Utc::now().timestamp() - 600;
            sqlx::query("DELETE FROM oauth_states WHERE created_at < ?")
                .bind(cutoff)
                .execute(&self.pool)
                .await?;
        }
        Ok(row.map(|r| r.0))
    }

    // ============================================================
    // v2 — Transactions
    // ============================================================

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_transaction(
        &self,
        account_id: i64,
        provider_txn_id: &str,
        timestamp: i64,
        description: &str,
        amount_cents: i64,
        currency: &str,
        is_credit: bool,
        is_pending: bool,
        merchant_name: Option<&str>,
        counterparty_iban: Option<&str>,
        counterparty_name: Option<&str>,
        category_id: Option<i64>,
        notes: Option<&str>,
        raw_json: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO transactions (account_id, provider_txn_id, timestamp, description,
                amount_cents, currency, is_credit, is_pending, merchant_name,
                counterparty_iban, counterparty_name, category_id, notes, raw_json,
                created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(account_id, provider_txn_id) DO UPDATE SET
                timestamp = excluded.timestamp,
                description = excluded.description,
                amount_cents = excluded.amount_cents,
                is_credit = excluded.is_credit,
                is_pending = excluded.is_pending,
                merchant_name = COALESCE(excluded.merchant_name, transactions.merchant_name),
                counterparty_iban = COALESCE(excluded.counterparty_iban, transactions.counterparty_iban),
                counterparty_name = COALESCE(excluded.counterparty_name, transactions.counterparty_name),
                raw_json = COALESCE(excluded.raw_json, transactions.raw_json),
                updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(account_id)
        .bind(provider_txn_id)
        .bind(timestamp)
        .bind(description)
        .bind(amount_cents)
        .bind(currency)
        .bind(if is_credit { 1 } else { 0 })
        .bind(if is_pending { 1 } else { 0 })
        .bind(merchant_name)
        .bind(counterparty_iban)
        .bind(counterparty_name)
        .bind(category_id)
        .bind(notes)
        .bind(raw_json)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn get_transaction(&self, id: i64) -> Result<Option<Transaction>> {
        Ok(sqlx::query_as::<_, Transaction>("SELECT * FROM transactions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn update_transaction_category(
        &self,
        id: i64,
        category_id: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE transactions SET category_id = ?, updated_at = ? WHERE id = ?",
        )
        .bind(category_id)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_transaction_notes(&self, id: i64, notes: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE transactions SET notes = ?, updated_at = ? WHERE id = ?")
            .bind(notes)
            .bind(Utc::now().timestamp())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Filtered/paged transactions query.
    /// All filters optional; ordering newest-first by default.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_transactions(
        &self,
        account_ids: Option<&[i64]>,
        category_ids: Option<&[i64]>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
        min_amount_cents: Option<i64>,
        max_amount_cents: Option<i64>,
        is_credit: Option<bool>,
        description_like: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Transaction>> {
        let mut q = String::from("SELECT * FROM transactions WHERE 1=1");
        let mut binds: Vec<String> = Vec::new();
        let mut int_binds: Vec<i64> = Vec::new();

        if let Some(aids) = account_ids {
            if !aids.is_empty() {
                let placeholders = vec!["?"; aids.len()].join(",");
                q.push_str(&format!(" AND account_id IN ({placeholders})"));
                int_binds.extend(aids.iter().copied());
            }
        }
        if let Some(cids) = category_ids {
            if !cids.is_empty() {
                let placeholders = vec!["?"; cids.len()].join(",");
                q.push_str(&format!(" AND category_id IN ({placeholders})"));
                int_binds.extend(cids.iter().copied());
            }
        }
        if from_ts.is_some() {
            q.push_str(" AND timestamp >= ?");
            int_binds.push(from_ts.unwrap());
        }
        if to_ts.is_some() {
            q.push_str(" AND timestamp <= ?");
            int_binds.push(to_ts.unwrap());
        }
        if min_amount_cents.is_some() {
            q.push_str(" AND amount_cents >= ?");
            int_binds.push(min_amount_cents.unwrap());
        }
        if max_amount_cents.is_some() {
            q.push_str(" AND amount_cents <= ?");
            int_binds.push(max_amount_cents.unwrap());
        }
        if let Some(c) = is_credit {
            q.push_str(" AND is_credit = ?");
            int_binds.push(if c { 1 } else { 0 });
        }
        if let Some(d) = description_like {
            q.push_str(" AND (description LIKE ? OR merchant_name LIKE ?)");
            let pat = format!("%{}%", d);
            binds.push(pat.clone());
            binds.push(pat);
        }

        q.push_str(" ORDER BY timestamp DESC LIMIT ? OFFSET ?");
        let mut query = sqlx::query_as::<_, Transaction>(&q);
        for v in int_binds {
            query = query.bind(v);
        }
        for v in &binds {
            query = query.bind(v);
        }
        query = query.bind(limit).bind(offset);
        Ok(query.fetch_all(&self.pool).await?)
    }

    pub async fn count_transactions_filtered(
        &self,
        account_ids: Option<&[i64]>,
        category_ids: Option<&[i64]>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
    ) -> Result<i64> {
        let mut q = String::from("SELECT COUNT(*) FROM transactions WHERE 1=1");
        let mut binds: Vec<i64> = Vec::new();
        if let Some(aids) = account_ids {
            if !aids.is_empty() {
                let p = vec!["?"; aids.len()].join(",");
                q.push_str(&format!(" AND account_id IN ({p})"));
                binds.extend(aids.iter().copied());
            }
        }
        if let Some(cids) = category_ids {
            if !cids.is_empty() {
                let p = vec!["?"; cids.len()].join(",");
                q.push_str(&format!(" AND category_id IN ({p})"));
                binds.extend(cids.iter().copied());
            }
        }
        if let Some(f) = from_ts {
            q.push_str(" AND timestamp >= ?");
            binds.push(f);
        }
        if let Some(t) = to_ts {
            q.push_str(" AND timestamp <= ?");
            binds.push(t);
        }
        let mut query = sqlx::query_as::<_, (i64,)>(&q);
        for v in binds {
            query = query.bind(v);
        }
        Ok(query.fetch_one(&self.pool).await?.0)
    }

    pub async fn spending_by_category(
        &self,
        from_ts: i64,
        to_ts: i64,
    ) -> Result<Vec<(Option<i64>, i64)>> {
        // Sum of withdrawal amounts per category in date range.
        let rows: Vec<(Option<i64>, i64)> = sqlx::query_as(
            "SELECT category_id, SUM(amount_cents) AS total_cents
             FROM transactions
             WHERE timestamp >= ? AND timestamp <= ? AND is_credit = 0
             GROUP BY category_id",
        )
        .bind(from_ts)
        .bind(to_ts)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ============================================================
    // v2 — Categories
    // ============================================================

    pub async fn list_categories(&self) -> Result<Vec<Category>> {
        Ok(sqlx::query_as::<_, Category>(
            "SELECT * FROM categories ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn create_category(
        &self,
        name: &str,
        parent_id: Option<i64>,
        icon: Option<&str>,
        colour: Option<&str>,
    ) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO categories (name, parent_id, icon, colour, created_at)
             VALUES (?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(name)
        .bind(parent_id)
        .bind(icon)
        .bind(colour)
        .bind(Utc::now().timestamp())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update_category(
        &self,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        icon: Option<&str>,
        colour: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE categories SET name = ?, parent_id = ?, icon = ?, colour = ? WHERE id = ?",
        )
        .bind(name)
        .bind(parent_id)
        .bind(icon)
        .bind(colour)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_category(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ============================================================
    // v2 — Tags
    // ============================================================

    pub async fn list_tags(&self) -> Result<Vec<Tag>> {
        Ok(sqlx::query_as::<_, Tag>("SELECT * FROM tags ORDER BY name")
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn upsert_tag(&self, name: &str) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO tags (name, created_at) VALUES (?, ?)
             ON CONFLICT(name) DO UPDATE SET name = excluded.name
             RETURNING id",
        )
        .bind(name)
        .bind(Utc::now().timestamp())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn tag_transaction(&self, transaction_id: i64, tag_id: i64) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO transaction_tags (transaction_id, tag_id) VALUES (?, ?)",
        )
        .bind(transaction_id)
        .bind(tag_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn untag_transaction(&self, transaction_id: i64, tag_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM transaction_tags WHERE transaction_id = ? AND tag_id = ?")
            .bind(transaction_id)
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn tags_for_transaction(&self, transaction_id: i64) -> Result<Vec<Tag>> {
        Ok(sqlx::query_as::<_, Tag>(
            "SELECT t.* FROM tags t INNER JOIN transaction_tags tt ON tt.tag_id = t.id
             WHERE tt.transaction_id = ? ORDER BY t.name",
        )
        .bind(transaction_id)
        .fetch_all(&self.pool)
        .await?)
    }

    // ============================================================
    // v2 — Rules
    // ============================================================

    pub async fn list_rules(&self, enabled_only: bool) -> Result<Vec<Rule>> {
        let q = if enabled_only {
            "SELECT * FROM rules WHERE enabled = 1 ORDER BY priority ASC, id ASC"
        } else {
            "SELECT * FROM rules ORDER BY priority ASC, id ASC"
        };
        Ok(sqlx::query_as::<_, Rule>(q).fetch_all(&self.pool).await?)
    }

    pub async fn get_rule(&self, id: i64) -> Result<Option<Rule>> {
        Ok(sqlx::query_as::<_, Rule>("SELECT * FROM rules WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_rule(
        &self,
        name: &str,
        priority: i64,
        match_description_regex: Option<&str>,
        match_merchant_regex: Option<&str>,
        match_min_amount_cents: Option<i64>,
        match_max_amount_cents: Option<i64>,
        match_account_id: Option<i64>,
        match_is_credit: Option<i64>,
        set_category_id: Option<i64>,
        add_tag_ids: Option<&str>,
        set_notes: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO rules (name, priority, match_description_regex, match_merchant_regex,
                match_min_amount_cents, match_max_amount_cents, match_account_id, match_is_credit,
                set_category_id, add_tag_ids, set_notes, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(name)
        .bind(priority)
        .bind(match_description_regex)
        .bind(match_merchant_regex)
        .bind(match_min_amount_cents)
        .bind(match_max_amount_cents)
        .bind(match_account_id)
        .bind(match_is_credit)
        .bind(set_category_id)
        .bind(add_tag_ids)
        .bind(set_notes)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_rule(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM rules WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_rule_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE rules SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(if enabled { 1 } else { 0 })
            .bind(Utc::now().timestamp())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn bump_rule_applied(&self, id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE rules SET times_applied = times_applied + 1, last_applied_at = ? WHERE id = ?",
        )
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ============================================================
    // v2 — Budgets
    // ============================================================

    pub async fn list_budgets(&self) -> Result<Vec<Budget>> {
        Ok(
            sqlx::query_as::<_, Budget>("SELECT * FROM budgets ORDER BY name")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn get_budget(&self, id: i64) -> Result<Option<Budget>> {
        Ok(sqlx::query_as::<_, Budget>("SELECT * FROM budgets WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn create_budget(
        &self,
        name: &str,
        category_id: Option<i64>,
        amount_cents: i64,
        period: &str,
        currency: &str,
        rollover: bool,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO budgets (name, category_id, amount_cents, period, currency, rollover,
                created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(name)
        .bind(category_id)
        .bind(amount_cents)
        .bind(period)
        .bind(currency)
        .bind(if rollover { 1 } else { 0 })
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_budget(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM budgets WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Total spend (withdrawal amount in cents) against the budget's category over the period
    /// containing `now`. Period is calendar-aligned for simplicity.
    pub async fn budget_period_spend(&self, budget: &Budget) -> Result<i64> {
        let (from, to) = period_range(&budget.period, Utc::now().timestamp());
        let row: (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(amount_cents) FROM transactions
             WHERE category_id IS ? AND is_credit = 0 AND timestamp >= ? AND timestamp <= ?",
        )
        .bind(budget.category_id)
        .bind(from)
        .bind(to)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0.unwrap_or(0))
    }

    // ============================================================
    // v2 — Bills
    // ============================================================

    pub async fn list_bills(&self) -> Result<Vec<Bill>> {
        Ok(
            sqlx::query_as::<_, Bill>("SELECT * FROM bills ORDER BY next_expected_date ASC")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn list_bills_due_within(&self, within_days: i64) -> Result<Vec<Bill>> {
        let now = Utc::now().timestamp();
        let cutoff = now + within_days * 86_400;
        Ok(sqlx::query_as::<_, Bill>(
            "SELECT * FROM bills
             WHERE enabled = 1 AND next_expected_date IS NOT NULL
               AND next_expected_date BETWEEN ? AND ?
             ORDER BY next_expected_date ASC",
        )
        .bind(now - 86_400)
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_bill(
        &self,
        name: &str,
        amount_min_cents: i64,
        amount_max_cents: i64,
        currency: &str,
        repeat_freq: &str,
        next_expected_date: Option<i64>,
        match_description_regex: Option<&str>,
        source_recurring_id: Option<i64>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO bills (name, expected_amount_min_cents, expected_amount_max_cents,
                currency, repeat_freq, next_expected_date, match_description_regex,
                source_recurring_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(name)
        .bind(amount_min_cents)
        .bind(amount_max_cents)
        .bind(currency)
        .bind(repeat_freq)
        .bind(next_expected_date)
        .bind(match_description_regex)
        .bind(source_recurring_id)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_bill(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM bills WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Find the bill auto-created for a given recurring mandate (DD / standing order),
    /// so re-syncs update it in place instead of inserting a duplicate every hour.
    pub async fn bill_id_for_recurring(&self, recurring_id: i64) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM bills WHERE source_recurring_id = ? LIMIT 1")
                .bind(recurring_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Update an existing bill's expected amount window + projected next date. Used on re-sync
    /// when we've inferred a fresher amount/date for a DD from transaction history.
    pub async fn update_bill_schedule(
        &self,
        id: i64,
        amount_min_cents: i64,
        amount_max_cents: i64,
        next_expected_date: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE bills SET expected_amount_min_cents = ?, expected_amount_max_cents = ?,
                next_expected_date = ?, updated_at = ? WHERE id = ?",
        )
        .bind(amount_min_cents)
        .bind(amount_max_cents)
        .bind(next_expected_date)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// TrueLayer often doesn't populate `previous_payment_amount` / date on DD mandates
    /// (Nationwide especially). Recover them from the real transaction history instead:
    /// the most recent settled debit whose description or merchant matches the mandate name.
    /// `name_like` should already be wrapped with `%` wildcards by the caller.
    /// Returns (amount_cents, timestamp) of that payment.
    pub async fn infer_recurring_payment(
        &self,
        account_id: i64,
        name_like: &str,
    ) -> Result<Option<(i64, i64)>> {
        Ok(sqlx::query_as::<_, (i64, i64)>(
            "SELECT amount_cents, timestamp FROM transactions
             WHERE account_id = ? AND is_credit = 0
               AND (description LIKE ? OR merchant_name LIKE ?)
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(account_id)
        .bind(name_like)
        .bind(name_like)
        .fetch_optional(&self.pool)
        .await?)
    }

    // ============================================================
    // v2 — Holdings / brokers / quotes
    // ============================================================

    pub async fn list_brokers(&self) -> Result<Vec<Broker>> {
        Ok(sqlx::query_as::<_, Broker>(
            "SELECT * FROM brokers ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn upsert_broker(
        &self,
        name: &str,
        kind: &str,
        currency: &str,
        notes: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO brokers (name, kind, currency, notes, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET
                kind = excluded.kind,
                currency = excluded.currency,
                notes = excluded.notes
             RETURNING id",
        )
        .bind(name)
        .bind(kind)
        .bind(currency)
        .bind(notes)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_broker(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM brokers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_holdings(&self) -> Result<Vec<Holding>> {
        Ok(sqlx::query_as::<_, Holding>(
            "SELECT * FROM holdings WHERE enabled = 1 ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_holdings_for_broker(&self, broker_id: i64) -> Result<Vec<Holding>> {
        Ok(sqlx::query_as::<_, Holding>(
            "SELECT * FROM holdings WHERE broker_id = ? AND enabled = 1 ORDER BY symbol",
        )
        .bind(broker_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_holding(&self, id: i64) -> Result<Option<Holding>> {
        Ok(sqlx::query_as::<_, Holding>("SELECT * FROM holdings WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn distinct_symbols(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT symbol FROM holdings WHERE enabled = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_holding(
        &self,
        broker_id: i64,
        symbol: &str,
        asset_class: &str,
        quantity: f64,
        avg_cost: Option<f64>,
        currency: &str,
        name: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().timestamp();
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO holdings (broker_id, symbol, asset_class, quantity, avg_cost_per_unit,
                currency, name, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(broker_id, symbol) DO UPDATE SET
                asset_class = excluded.asset_class,
                quantity = excluded.quantity,
                avg_cost_per_unit = excluded.avg_cost_per_unit,
                currency = excluded.currency,
                name = COALESCE(excluded.name, holdings.name),
                updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(broker_id)
        .bind(symbol)
        .bind(asset_class)
        .bind(quantity)
        .bind(avg_cost)
        .bind(currency)
        .bind(name)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_holding(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM holdings WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_holding_synced(&self, id: i64, name: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE holdings SET last_synced_at = ?, name = COALESCE(?, name) WHERE id = ?",
        )
        .bind(Utc::now().timestamp())
        .bind(name)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_activities(&self, holding_id: i64) -> Result<Vec<HoldingActivity>> {
        Ok(sqlx::query_as::<_, HoldingActivity>(
            "SELECT * FROM holding_activities WHERE holding_id = ? ORDER BY timestamp DESC",
        )
        .bind(holding_id)
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_activity(
        &self,
        holding_id: i64,
        activity_type: &str,
        timestamp: i64,
        quantity: f64,
        price_per_unit: Option<f64>,
        fee: f64,
        currency: &str,
        notes: Option<&str>,
    ) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO holding_activities (holding_id, activity_type, timestamp, quantity,
                price_per_unit, fee, currency, notes, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(holding_id)
        .bind(activity_type)
        .bind(timestamp)
        .bind(quantity)
        .bind(price_per_unit)
        .bind(fee)
        .bind(currency)
        .bind(notes)
        .bind(Utc::now().timestamp())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn delete_activity(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM holding_activities WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Edit an existing activity in place (e.g. to correct a mis-entered date/quantity/price).
    /// Returns the holding_id so the caller can recompute derived position totals.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_activity(
        &self,
        id: i64,
        activity_type: &str,
        timestamp: i64,
        quantity: f64,
        price_per_unit: Option<f64>,
        fee: f64,
        notes: Option<&str>,
    ) -> Result<Option<i64>> {
        let holding_id: Option<i64> =
            sqlx::query_scalar("SELECT holding_id FROM holding_activities WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        if holding_id.is_none() {
            return Ok(None);
        }
        sqlx::query(
            "UPDATE holding_activities SET activity_type = ?, timestamp = ?, quantity = ?,
                price_per_unit = ?, fee = ?, notes = ? WHERE id = ?",
        )
        .bind(activity_type)
        .bind(timestamp)
        .bind(quantity)
        .bind(price_per_unit)
        .bind(fee)
        .bind(notes)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(holding_id)
    }

    /// Recompute a holding's `quantity` and `avg_cost_per_unit` from its activity log.
    /// Source-of-truth for current position becomes the activities — BUY adds, SELL subtracts,
    /// avg cost is the weighted average of remaining BUY lots (FIFO-free simple weighted).
    pub async fn recompute_holding_from_activities(&self, holding_id: i64) -> Result<()> {
        let acts = self.list_activities(holding_id).await?;
        let mut qty: f64 = 0.0;
        let mut cost_basis: f64 = 0.0;
        for a in acts.iter().rev() {
            // chronological
            match a.activity_type.as_str() {
                "BUY" | "TRANSFER_IN" => {
                    let p = a.price_per_unit.unwrap_or(0.0);
                    cost_basis += a.quantity * p + a.fee;
                    qty += a.quantity;
                }
                "SELL" | "TRANSFER_OUT" => {
                    // Sell proportional cost basis — keeps avg cost stable.
                    if qty > 1e-12 {
                        let sell_qty = a.quantity.min(qty);
                        let unit_cost = cost_basis / qty;
                        cost_basis -= sell_qty * unit_cost;
                        qty -= sell_qty;
                    }
                }
                _ => {}
            }
        }
        let avg = if qty > 1e-12 {
            Some(cost_basis / qty)
        } else {
            None
        };
        sqlx::query(
            "UPDATE holdings SET quantity = ?, avg_cost_per_unit = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(qty)
        .bind(avg)
        .bind(Utc::now().timestamp())
        .bind(holding_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn latest_quote(&self, symbol: &str) -> Result<Option<LatestQuote>> {
        Ok(sqlx::query_as::<_, LatestQuote>(
            "SELECT * FROM latest_quotes WHERE symbol = ?",
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn all_latest_quotes(&self) -> Result<Vec<LatestQuote>> {
        Ok(sqlx::query_as::<_, LatestQuote>(
            "SELECT * FROM latest_quotes ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_latest_quote(
        &self,
        symbol: &str,
        price: f64,
        currency: &str,
        previous_close: Option<f64>,
        day_change_pct: Option<f64>,
        company_name: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO latest_quotes (symbol, price, currency, fetched_at,
                previous_close, day_change_pct, company_name)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(symbol) DO UPDATE SET
                price = excluded.price,
                currency = excluded.currency,
                fetched_at = excluded.fetched_at,
                previous_close = excluded.previous_close,
                day_change_pct = excluded.day_change_pct,
                company_name = COALESCE(excluded.company_name, latest_quotes.company_name)",
        )
        .bind(symbol)
        .bind(price)
        .bind(currency)
        .bind(Utc::now().timestamp())
        .bind(previous_close)
        .bind(day_change_pct)
        .bind(company_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_bill_paid_by_transaction(
        &self,
        bill_id: i64,
        transaction_id: i64,
        paid_ts: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO bill_payments (bill_id, transaction_id) VALUES (?, ?)",
        )
        .bind(bill_id)
        .bind(transaction_id)
        .execute(&self.pool)
        .await?;
        // Record the payment and roll the projected next-due date forward one period past the
        // payment we just matched, so the bill keeps pointing at a future date instead of
        // freezing on a stale value (the reason "Bills ahead" was silently empty).
        let freq: Option<String> =
            sqlx::query_scalar("SELECT repeat_freq FROM bills WHERE id = ?")
                .bind(bill_id)
                .fetch_optional(&self.pool)
                .await?;
        let next = freq
            .as_deref()
            .map(|f| advance_past(paid_ts, f, Utc::now().timestamp()));
        sqlx::query(
            "UPDATE bills SET last_paid_date = ?, next_expected_date = COALESCE(?, next_expected_date),
                updated_at = ? WHERE id = ?",
        )
        .bind(paid_ts)
        .bind(next)
        .bind(Utc::now().timestamp())
        .bind(bill_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Roll `from_ts` forward by `freq` periods until it is strictly after `now`.
/// Month/year stepping is calendar-correct (clamps to month length) via chrono Months.
pub fn advance_past(from_ts: i64, freq: &str, now: i64) -> i64 {
    use chrono::{Duration, Months, TimeZone};
    let mut dt = Utc.timestamp_opt(from_ts, 0).single().unwrap_or_else(Utc::now);
    let mut guard = 0;
    while dt.timestamp() <= now && guard < 600 {
        dt = match freq {
            "weekly" => dt + Duration::days(7),
            "fortnightly" => dt + Duration::days(14),
            "yearly" => dt.checked_add_months(Months::new(12)).unwrap_or(dt + Duration::days(365)),
            // monthly is the default for DDs
            _ => dt.checked_add_months(Months::new(1)).unwrap_or(dt + Duration::days(30)),
        };
        guard += 1;
    }
    dt.timestamp()
}

/// Return (from_ts, to_ts) for the calendar-aligned current period.
/// Naive impl: monthly = 1st of month → end of month. Weekly = Monday 00:00 → Sunday 23:59:59.
/// Yearly = Jan 1 → Dec 31. UTC throughout.
fn period_range(period: &str, now_ts: i64) -> (i64, i64) {
    use chrono::{DateTime, Datelike, Duration, TimeZone};
    let dt: DateTime<Utc> = chrono::Utc.timestamp_opt(now_ts, 0).unwrap();
    match period {
        "weekly" => {
            // chrono's weekday Mon=0 ... Sun=6 when using num_days_from_monday
            let days_from_mon = dt.weekday().num_days_from_monday() as i64;
            let start = (dt - Duration::days(days_from_mon))
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp();
            let end = start + 7 * 86_400 - 1;
            (start, end)
        }
        "yearly" => {
            let start = chrono::Utc
                .with_ymd_and_hms(dt.year(), 1, 1, 0, 0, 0)
                .unwrap()
                .timestamp();
            let end = chrono::Utc
                .with_ymd_and_hms(dt.year(), 12, 31, 23, 59, 59)
                .unwrap()
                .timestamp();
            (start, end)
        }
        _ => {
            // monthly (default)
            let start = chrono::Utc
                .with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
                .unwrap()
                .timestamp();
            let (ey, em) = if dt.month() == 12 {
                (dt.year() + 1, 1)
            } else {
                (dt.year(), dt.month() + 1)
            };
            let end = chrono::Utc
                .with_ymd_and_hms(ey, em, 1, 0, 0, 0)
                .unwrap()
                .timestamp()
                - 1;
            (start, end)
        }
    }
}
