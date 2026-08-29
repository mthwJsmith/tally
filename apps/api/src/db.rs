//! Database layer. Wraps libsql + encrypts/decrypts secrets transparently.
//!
//! Migrated from sqlx to the libsql crate (Turso). Every query goes through a single shared
//! `libsql::Connection` (held behind an Arc and cloned cheaply). Rows are mapped via the manual
//! `FromLibsqlRow` impls in `models.rs` — by column NAME, never positional index, so join-only
//! columns (e.g. `Account::consent_nickname`) don't misalign other SELECTs.

use crate::crypto::Crypto;
use crate::models::*;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use libsql::{params, Builder, Connection, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct Db {
    pub conn: Arc<Connection>,
    pub crypto: Crypto,
}

/// Collect every row of a query into `Vec<T>` using the type's `FromLibsqlRow` mapper.
async fn map_rows<T: FromLibsqlRow>(mut rows: libsql::Rows) -> Result<Vec<T>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(T::from_row(&row)?);
    }
    Ok(out)
}

/// Map the first row of a query (if any) using `FromLibsqlRow`.
async fn map_opt<T: FromLibsqlRow>(mut rows: libsql::Rows) -> Result<Option<T>> {
    match rows.next().await? {
        Some(row) => Ok(Some(T::from_row(&row)?)),
        None => Ok(None),
    }
}

impl Db {
    /// Connect to the configured database. The connection mode is chosen by the URL scheme of
    /// `database_url` (the value of `TALLY_DATABASE_URL`):
    ///
    /// - `file:` / `sqlite:` prefix OR a bare path (the DEFAULT — Raspberry Pi / offline):
    ///   strip the scheme to an OS path and open a local libsql database, then set
    ///   `PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;`.
    /// - `libsql://` / `wss://` / `https://` with `TALLY_DATABASE_AUTH_TOKEN` set
    ///   (Turso / serverless): open an embedded remote replica backed by a local replica file,
    ///   falling back to a pure remote connection. The local-file path is the supported default;
    ///   the remote path is secondary.
    pub async fn connect(database_url: &str, crypto: Crypto) -> Result<Self> {
        let url = database_url.trim();
        let is_remote = url.starts_with("libsql://")
            || url.starts_with("wss://")
            || url.starts_with("https://");

        let conn = if is_remote {
            let token = std::env::var("TALLY_DATABASE_AUTH_TOKEN").unwrap_or_default();
            // Local replica file kept alongside the app data dir so reads are local and the
            // node keeps working offline; writes replicate to the remote.
            let replica_path = "/app/data/replica.db".to_string();
            let db = match Builder::new_remote_replica(
                &replica_path,
                url.to_string(),
                token.clone(),
            )
            .build()
            .await
            {
                Ok(db) => db,
                Err(e) => {
                    tracing::warn!(
                        "remote-replica build failed ({e}); falling back to pure remote"
                    );
                    Builder::new_remote(url.to_string(), token)
                        .build()
                        .await
                        .context("connecting to remote libsql")?
                }
            };
            let conn = db.connect().context("opening remote libsql connection")?;
            conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();
            conn
        } else {
            // Strip a leading `file:` or `sqlite:` scheme down to an OS path. `sqlite://x` and
            // `sqlite:x` both map to `x`; a bare path is used verbatim.
            let path = strip_db_scheme(url);
            let db = Builder::new_local(&path)
                .build()
                .await
                .with_context(|| format!("opening local libsql database at {path}"))?;
            let conn = db.connect().context("opening local libsql connection")?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .await
                .context("setting local PRAGMAs")?;
            conn
        };

        crate::migrate::run(&conn).await.context("running migrations")?;
        Ok(Self {
            conn: Arc::new(conn),
            crypto,
        })
    }

    // ---------- settings ----------

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut rows = self
            .conn
            .query("SELECT value FROM settings WHERE key = ?1", params![key])
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.get::<String>(0)?)),
            None => Ok(None),
        }
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .await?;
        Ok(())
    }

    /// Store Telegram bot token (encrypted at rest) + chat id. Empty strings clear them.
    pub async fn set_telegram_config(&self, token: &str, chat_id: &str) -> Result<()> {
        let token = token.trim();
        // Empty token = keep the existing one (the UI never round-trips the secret).
        if !token.is_empty() {
            let (nonce, ct) = self.crypto.encrypt(token)?;
            let v = format!("enc:{}:{}", B64.encode(nonce), B64.encode(ct));
            self.set_setting("telegram_bot_token", &v).await?;
        }
        self.set_setting("telegram_chat_id", chat_id.trim()).await?;
        Ok(())
    }

    /// Returns (bot_token, chat_id) from settings, decrypting the token. Either may be None.
    pub async fn get_telegram_config(&self) -> Result<(Option<String>, Option<String>)> {
        let token = match self.get_setting("telegram_bot_token").await? {
            Some(v) if v.starts_with("enc:") => {
                let parts: Vec<&str> = v.splitn(3, ':').collect();
                if parts.len() == 3 {
                    match (B64.decode(parts[1]), B64.decode(parts[2])) {
                        (Ok(nonce), Ok(ct)) => self.crypto.decrypt(&nonce, &ct).ok(),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let chat = self
            .get_setting("telegram_chat_id")
            .await?
            .filter(|s| !s.is_empty());
        Ok((token, chat))
    }

    /// Store the Claude routine endpoint URL (plain) + bearer token (encrypted at rest).
    /// An empty token keeps the existing one; the UI never round-trips the secret.
    pub async fn set_routine_config(&self, endpoint: &str, token: &str) -> Result<()> {
        let token = token.trim();
        if !token.is_empty() {
            let (nonce, ct) = self.crypto.encrypt(token)?;
            let v = format!("enc:{}:{}", B64.encode(nonce), B64.encode(ct));
            self.set_setting("claude_routine_token", &v).await?;
        }
        self.set_setting("claude_routine_endpoint", endpoint.trim())
            .await?;
        Ok(())
    }

    /// Returns (endpoint, bearer_token), decrypting the token. Either may be None.
    pub async fn get_routine_config(&self) -> Result<(Option<String>, Option<String>)> {
        let token = match self.get_setting("claude_routine_token").await? {
            Some(v) if v.starts_with("enc:") => {
                let parts: Vec<&str> = v.splitn(3, ':').collect();
                if parts.len() == 3 {
                    match (B64.decode(parts[1]), B64.decode(parts[2])) {
                        (Ok(nonce), Ok(ct)) => self.crypto.decrypt(&nonce, &ct).ok(),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let endpoint = self
            .get_setting("claude_routine_endpoint")
            .await?
            .filter(|s| !s.is_empty());
        Ok((endpoint, token))
    }

    // ---------- consents ----------

    pub async fn list_consents(&self) -> Result<Vec<Consent>> {
        let rows = self
            .conn
            .query("SELECT * FROM consents ORDER BY nickname", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn list_enabled_consents(&self) -> Result<Vec<Consent>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM consents WHERE enabled = 1 ORDER BY nickname",
                (),
            )
            .await?;
        map_rows(rows).await
    }

    pub async fn get_consent(&self, id: i64) -> Result<Option<Consent>> {
        let rows = self
            .conn
            .query("SELECT * FROM consents WHERE id = ?1", params![id])
            .await?;
        map_opt(rows).await
    }

    pub async fn get_consent_by_nickname(&self, nickname: &str) -> Result<Option<Consent>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM consents WHERE nickname = ?1",
                params![nickname],
            )
            .await?;
        map_opt(rows).await
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

        self.conn
            .execute(
                "INSERT INTO consents (nickname, credentials_id, provider_id, provider_display_name,
                    access_token_enc, access_token_nonce, refresh_token_enc, refresh_token_nonce,
                    expires_at, consent_expires_at, scopes, created_at, updated_at, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1)
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
                    enabled = 1,
                    -- Re-linking is the cure for a `reauth`/`fail` consent: clear the old
                    -- health state so the scheduler stops skipping it and the UI stops
                    -- showing the stale error.
                    last_sync_status = NULL,
                    last_sync_error = NULL",
                params![
                    nickname,
                    credentials_id,
                    provider_id,
                    provider_display_name,
                    atok_ct,
                    atok_nonce,
                    rtok_ct,
                    rtok_nonce,
                    expires_at,
                    consent_expires_at,
                    scopes,
                    now,
                    now,
                ],
            )
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
        self.conn
            .execute(
                "UPDATE consents SET
                    access_token_enc = ?1, access_token_nonce = ?2,
                    refresh_token_enc = ?3, refresh_token_nonce = ?4,
                    expires_at = ?5, updated_at = ?6
                 WHERE id = ?7",
                params![
                    atok_ct,
                    atok_nonce,
                    rtok_ct,
                    rtok_nonce,
                    expires_at,
                    Utc::now().timestamp(),
                    consent_id,
                ],
            )
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
        self.conn
            .execute(
                "UPDATE consents SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                params![
                    if enabled { 1_i64 } else { 0 },
                    Utc::now().timestamp(),
                    consent_id,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_consent(&self, consent_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM consents WHERE id = ?1",
                params![consent_id],
            )
            .await?;
        Ok(())
    }

    pub async fn touch_consent_sync_status(
        &self,
        consent_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE consents SET last_sync_at = ?1, last_sync_status = ?2, last_sync_error = ?3 WHERE id = ?4",
                params![Utc::now().timestamp(), status, error, consent_id],
            )
            .await?;
        Ok(())
    }

    // ---------- accounts ----------

    pub async fn list_accounts_for_consent(&self, consent_id: i64) -> Result<Vec<Account>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM accounts WHERE consent_id = ?1 ORDER BY kind, display_name",
                params![consent_id],
            )
            .await?;
        map_rows(rows).await
    }

    pub async fn list_all_enabled_accounts(&self) -> Result<Vec<Account>> {
        // This is the ONLY SELECT that adds `consent_nickname` and `pending_net_cents` columns;
        // the Account mapper resolves them via the absent-tolerant path, so all other Account
        // SELECTs (which lack these columns) still map cleanly.
        let rows = self
            .conn
            .query(
                "SELECT a.*, c.nickname AS consent_nickname,
                    (SELECT COALESCE(SUM(CASE WHEN t.is_credit = 1 THEN t.amount_cents
                                              ELSE -t.amount_cents END), 0)
                     FROM transactions t WHERE t.account_id = a.id AND t.is_pending = 1)
                        AS pending_net_cents
                 FROM accounts a
                 INNER JOIN consents c ON c.id = a.consent_id
                 WHERE a.enabled = 1 AND c.enabled = 1
                 ORDER BY a.kind, a.display_name",
                (),
            )
            .await?;
        map_rows(rows).await
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
        self.conn
            .execute(
                "INSERT INTO accounts (consent_id, truelayer_id, kind, display_name, iban, sort_code,
                    account_number, card_last4, currency, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11)
                 ON CONFLICT(consent_id, truelayer_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    iban = excluded.iban,
                    sort_code = excluded.sort_code,
                    account_number = excluded.account_number,
                    card_last4 = excluded.card_last4,
                    currency = excluded.currency,
                    updated_at = excluded.updated_at",
                params![
                    consent_id,
                    truelayer_id,
                    kind,
                    display_name,
                    iban,
                    sort_code,
                    account_number,
                    card_last4,
                    currency,
                    now,
                    now,
                ],
            )
            .await?;

        let rows = self
            .conn
            .query(
                "SELECT * FROM accounts WHERE consent_id = ?1 AND truelayer_id = ?2",
                params![consent_id, truelayer_id],
            )
            .await?;
        map_opt(rows)
            .await?
            .context("account disappeared after upsert")
    }

    pub async fn map_account_to_firefly(
        &self,
        account_id: i64,
        firefly_account_id: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE accounts SET firefly_account_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![firefly_account_id, Utc::now().timestamp(), account_id],
            )
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
        self.conn
            .execute(
                "UPDATE accounts SET
                    current_balance_cents = ?1,
                    available_balance_cents = ?2,
                    overdraft_cents = ?3,
                    balance_updated_at = ?4,
                    updated_at = ?5
                 WHERE id = ?6",
                params![current_cents, available_cents, overdraft_cents, now, now, account_id],
            )
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
        self.conn
            .execute(
                "UPDATE accounts SET
                    current_balance_cents = ?1,
                    available_balance_cents = ?2,
                    credit_limit_cents = ?3,
                    last_statement_balance_cents = ?4,
                    last_statement_date = ?5,
                    payment_due_cents = ?6,
                    payment_due_date = ?7,
                    balance_updated_at = ?8,
                    updated_at = ?9
                 WHERE id = ?10",
                params![
                    current_cents,
                    available_cents,
                    credit_limit_cents,
                    last_statement_balance_cents,
                    last_statement_date,
                    payment_due_cents,
                    payment_due_date,
                    now,
                    now,
                    account_id,
                ],
            )
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
        self.conn
            .execute(
                "UPDATE accounts SET
                    account_type = COALESCE(?1, account_type),
                    card_network = COALESCE(?2, card_network),
                    name_on_card = COALESCE(?3, name_on_card),
                    updated_at = ?4
                 WHERE id = ?5",
                params![account_type, card_network, name_on_card, Utc::now().timestamp(), account_id],
            )
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
        self.conn
            .execute(
                "UPDATE accounts SET custom_display_name = ?1, updated_at = ?2 WHERE id = ?3",
                params![custom, Utc::now().timestamp(), account_id],
            )
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
        self.conn
            .execute(
                "UPDATE consents SET nickname = ?1, updated_at = ?2 WHERE id = ?3",
                params![cleaned, Utc::now().timestamp(), id],
            )
            .await?;
        Ok(())
    }

    // ---------- transactions_seen ----------

    pub async fn is_txn_seen(&self, account_id: i64, truelayer_txn_id: &str) -> Result<bool> {
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM transactions_seen WHERE account_id = ?1 AND truelayer_txn_id = ?2",
                params![account_id, truelayer_txn_id],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }

    pub async fn record_txn_imported(
        &self,
        account_id: i64,
        truelayer_txn_id: &str,
        firefly_txn_id: Option<i64>,
        is_pending: bool,
        raw_json: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO transactions_seen (account_id, truelayer_txn_id, firefly_txn_id, is_pending, raw_json, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(account_id, truelayer_txn_id) DO UPDATE SET
                    firefly_txn_id = COALESCE(excluded.firefly_txn_id, transactions_seen.firefly_txn_id),
                    is_pending = excluded.is_pending",
                params![
                    account_id,
                    truelayer_txn_id,
                    firefly_txn_id,
                    if is_pending { 1_i64 } else { 0 },
                    raw_json,
                    Utc::now().timestamp(),
                ],
            )
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
        self.conn
            .execute(
                "INSERT INTO recurring (account_id, truelayer_id, kind, name, amount, currency,
                    frequency, next_payment_date, status, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(account_id, truelayer_id, kind) DO UPDATE SET
                    name = excluded.name,
                    amount = excluded.amount,
                    currency = excluded.currency,
                    frequency = excluded.frequency,
                    next_payment_date = excluded.next_payment_date,
                    status = excluded.status,
                    last_seen_at = excluded.last_seen_at",
                params![
                    account_id,
                    truelayer_id,
                    kind,
                    name,
                    amount,
                    currency,
                    frequency,
                    next_payment_date,
                    status,
                    Utc::now().timestamp(),
                ],
            )
            .await?;

        let rows = self
            .conn
            .query(
                "SELECT * FROM recurring WHERE account_id = ?1 AND truelayer_id = ?2 AND kind = ?3",
                params![account_id, truelayer_id, kind],
            )
            .await?;
        map_opt(rows)
            .await?
            .context("recurring disappeared after upsert")
    }

    pub async fn map_recurring_to_firefly_bill(
        &self,
        recurring_id: i64,
        firefly_bill_id: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE recurring SET firefly_bill_id = ?1 WHERE id = ?2",
                params![firefly_bill_id, recurring_id],
            )
            .await?;
        Ok(())
    }

    pub async fn list_recurring_for_account(&self, account_id: i64) -> Result<Vec<RecurringEntry>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM recurring WHERE account_id = ?1 ORDER BY kind, name",
                params![account_id],
            )
            .await?;
        map_rows(rows).await
    }

    // ---------- sync_log ----------

    pub async fn start_sync_log(&self, consent_id: Option<i64>) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO sync_log (consent_id, started_at, status) VALUES (?1, ?2, 'in_progress')",
                params![consent_id, Utc::now().timestamp()],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
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
        self.conn
            .execute(
                "UPDATE sync_log SET ended_at = ?1, status = ?2, accounts_synced = ?3,
                    transactions_imported = ?4, transactions_skipped = ?5,
                    recurring_imported = ?6, error_message = ?7
                 WHERE id = ?8",
                params![
                    Utc::now().timestamp(),
                    status,
                    accounts,
                    imported,
                    skipped,
                    recurring,
                    error,
                    log_id,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn recent_sync_logs(&self, limit: i64) -> Result<Vec<SyncLogEntry>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM sync_log ORDER BY started_at DESC LIMIT ?1",
                params![limit],
            )
            .await?;
        map_rows(rows).await
    }

    // ---------- oauth_states ----------

    pub async fn save_oauth_state(&self, state: &str, nickname: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO oauth_states (state, nickname, created_at) VALUES (?1, ?2, ?3)",
                params![state, nickname, Utc::now().timestamp()],
            )
            .await?;
        Ok(())
    }

    pub async fn consume_oauth_state(&self, state: &str) -> Result<Option<String>> {
        let nickname: Option<String> = {
            let mut rows = self
                .conn
                .query(
                    "SELECT nickname FROM oauth_states WHERE state = ?1",
                    params![state],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Some(row.get::<String>(0)?),
                None => None,
            }
        };
        if nickname.is_some() {
            self.conn
                .execute(
                    "DELETE FROM oauth_states WHERE state = ?1",
                    params![state],
                )
                .await?;
            // Garbage-collect old states (> 10 min)
            let cutoff = Utc::now().timestamp() - 600;
            self.conn
                .execute(
                    "DELETE FROM oauth_states WHERE created_at < ?1",
                    params![cutoff],
                )
                .await?;
        }
        Ok(nickname)
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
        let mut rows = self
            .conn
            .query(
                "INSERT INTO transactions (account_id, provider_txn_id, timestamp, description,
                    amount_cents, currency, is_credit, is_pending, merchant_name,
                    counterparty_iban, counterparty_name, category_id, notes, raw_json,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                params![
                    account_id,
                    provider_txn_id,
                    timestamp,
                    description,
                    amount_cents,
                    currency,
                    if is_credit { 1_i64 } else { 0 },
                    if is_pending { 1_i64 } else { 0 },
                    merchant_name,
                    counterparty_iban,
                    counterparty_name,
                    category_id,
                    notes,
                    raw_json,
                    now,
                    now,
                ],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .context("upsert_transaction RETURNING id produced no row")?;
        Ok(row.get::<i64>(0)?)
    }

    pub async fn get_transaction(&self, id: i64) -> Result<Option<Transaction>> {
        let rows = self
            .conn
            .query("SELECT * FROM transactions WHERE id = ?1", params![id])
            .await?;
        map_opt(rows).await
    }

    pub async fn update_transaction_category(
        &self,
        id: i64,
        category_id: Option<i64>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE transactions SET category_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![category_id, Utc::now().timestamp(), id],
            )
            .await?;
        Ok(())
    }

    pub async fn update_transaction_notes(&self, id: i64, notes: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE transactions SET notes = ?1, updated_at = ?2 WHERE id = ?3",
                params![notes, Utc::now().timestamp(), id],
            )
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
        // Dynamic SQL: build the positional bind list in EXACTLY the order the placeholders are
        // appended (ints first, then the two LIKE strings, then limit/offset) — mirroring the old
        // sqlx .bind() chain. `?` positional placeholders are matched left-to-right by libsql.
        let mut q = String::from("SELECT * FROM transactions WHERE 1=1");
        let mut binds: Vec<Value> = Vec::new();

        if let Some(aids) = account_ids {
            if !aids.is_empty() {
                let placeholders = vec!["?"; aids.len()].join(",");
                q.push_str(&format!(" AND account_id IN ({placeholders})"));
                binds.extend(aids.iter().map(|v| Value::from(*v)));
            }
        }
        if let Some(cids) = category_ids {
            if !cids.is_empty() {
                let placeholders = vec!["?"; cids.len()].join(",");
                q.push_str(&format!(" AND category_id IN ({placeholders})"));
                binds.extend(cids.iter().map(|v| Value::from(*v)));
            }
        }
        if let Some(f) = from_ts {
            q.push_str(" AND timestamp >= ?");
            binds.push(Value::from(f));
        }
        if let Some(t) = to_ts {
            q.push_str(" AND timestamp <= ?");
            binds.push(Value::from(t));
        }
        if let Some(m) = min_amount_cents {
            q.push_str(" AND amount_cents >= ?");
            binds.push(Value::from(m));
        }
        if let Some(m) = max_amount_cents {
            q.push_str(" AND amount_cents <= ?");
            binds.push(Value::from(m));
        }
        if let Some(c) = is_credit {
            q.push_str(" AND is_credit = ?");
            binds.push(Value::from(if c { 1_i64 } else { 0 }));
        }
        if let Some(d) = description_like {
            q.push_str(" AND (description LIKE ? OR merchant_name LIKE ?)");
            let pat = format!("%{}%", d);
            binds.push(Value::from(pat.clone()));
            binds.push(Value::from(pat));
        }

        q.push_str(" ORDER BY timestamp DESC LIMIT ? OFFSET ?");
        binds.push(Value::from(limit));
        binds.push(Value::from(offset));

        let rows = self.conn.query(&q, binds).await?;
        map_rows(rows).await
    }

    pub async fn count_transactions_filtered(
        &self,
        account_ids: Option<&[i64]>,
        category_ids: Option<&[i64]>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
    ) -> Result<i64> {
        // Same dynamic-SQL bind-order discipline as list_transactions (ints only here).
        let mut q = String::from("SELECT COUNT(*) FROM transactions WHERE 1=1");
        let mut binds: Vec<Value> = Vec::new();
        if let Some(aids) = account_ids {
            if !aids.is_empty() {
                let p = vec!["?"; aids.len()].join(",");
                q.push_str(&format!(" AND account_id IN ({p})"));
                binds.extend(aids.iter().map(|v| Value::from(*v)));
            }
        }
        if let Some(cids) = category_ids {
            if !cids.is_empty() {
                let p = vec!["?"; cids.len()].join(",");
                q.push_str(&format!(" AND category_id IN ({p})"));
                binds.extend(cids.iter().map(|v| Value::from(*v)));
            }
        }
        if let Some(f) = from_ts {
            q.push_str(" AND timestamp >= ?");
            binds.push(Value::from(f));
        }
        if let Some(t) = to_ts {
            q.push_str(" AND timestamp <= ?");
            binds.push(Value::from(t));
        }
        let mut rows = self.conn.query(&q, binds).await?;
        let row = rows
            .next()
            .await?
            .context("count query produced no row")?;
        Ok(row.get::<i64>(0)?)
    }

    pub async fn spending_by_category(
        &self,
        from_ts: i64,
        to_ts: i64,
    ) -> Result<Vec<(Option<i64>, i64)>> {
        // Sum of withdrawal amounts per category in date range.
        let mut rows = self
            .conn
            .query(
                "SELECT category_id, SUM(amount_cents) AS total_cents
                 FROM transactions
                 WHERE timestamp >= ?1 AND timestamp <= ?2 AND is_credit = 0
                 GROUP BY category_id",
                params![from_ts, to_ts],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push((row.get::<Option<i64>>(0)?, row.get::<i64>(1)?));
        }
        Ok(out)
    }

    // ============================================================
    // v2 — Categories
    // ============================================================

    pub async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = self
            .conn
            .query("SELECT * FROM categories ORDER BY name", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn create_category(
        &self,
        name: &str,
        parent_id: Option<i64>,
        icon: Option<&str>,
        colour: Option<&str>,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO categories (name, parent_id, icon, colour, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, parent_id, icon, colour, Utc::now().timestamp()],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn update_category(
        &self,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        icon: Option<&str>,
        colour: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE categories SET name = ?1, parent_id = ?2, icon = ?3, colour = ?4 WHERE id = ?5",
                params![name, parent_id, icon, colour, id],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_category(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM categories WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    // ============================================================
    // v2 — Tags
    // ============================================================

    pub async fn list_tags(&self) -> Result<Vec<Tag>> {
        let rows = self
            .conn
            .query("SELECT * FROM tags ORDER BY name", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn upsert_tag(&self, name: &str) -> Result<i64> {
        let mut rows = self
            .conn
            .query(
                "INSERT INTO tags (name, created_at) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET name = excluded.name
                 RETURNING id",
                params![name, Utc::now().timestamp()],
            )
            .await?;
        let row = rows.next().await?.context("upsert_tag RETURNING produced no row")?;
        Ok(row.get::<i64>(0)?)
    }

    pub async fn tag_transaction(&self, transaction_id: i64, tag_id: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO transaction_tags (transaction_id, tag_id) VALUES (?1, ?2)",
                params![transaction_id, tag_id],
            )
            .await?;
        Ok(())
    }

    pub async fn untag_transaction(&self, transaction_id: i64, tag_id: i64) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM transaction_tags WHERE transaction_id = ?1 AND tag_id = ?2",
                params![transaction_id, tag_id],
            )
            .await?;
        Ok(())
    }

    pub async fn tags_for_transaction(&self, transaction_id: i64) -> Result<Vec<Tag>> {
        let rows = self
            .conn
            .query(
                "SELECT t.* FROM tags t INNER JOIN transaction_tags tt ON tt.tag_id = t.id
                 WHERE tt.transaction_id = ?1 ORDER BY t.name",
                params![transaction_id],
            )
            .await?;
        map_rows(rows).await
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
        let rows = self.conn.query(q, ()).await?;
        map_rows(rows).await
    }

    pub async fn get_rule(&self, id: i64) -> Result<Option<Rule>> {
        let rows = self
            .conn
            .query("SELECT * FROM rules WHERE id = ?1", params![id])
            .await?;
        map_opt(rows).await
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
        self.conn
            .execute(
                "INSERT INTO rules (name, priority, match_description_regex, match_merchant_regex,
                    match_min_amount_cents, match_max_amount_cents, match_account_id, match_is_credit,
                    set_category_id, add_tag_ids, set_notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    name,
                    priority,
                    match_description_regex,
                    match_merchant_regex,
                    match_min_amount_cents,
                    match_max_amount_cents,
                    match_account_id,
                    match_is_credit,
                    set_category_id,
                    add_tag_ids,
                    set_notes,
                    now,
                    now,
                ],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn delete_rule(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM rules WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    pub async fn set_rule_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE rules SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                params![if enabled { 1_i64 } else { 0 }, Utc::now().timestamp(), id],
            )
            .await?;
        Ok(())
    }

    pub async fn bump_rule_applied(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE rules SET times_applied = times_applied + 1, last_applied_at = ?1 WHERE id = ?2",
                params![Utc::now().timestamp(), id],
            )
            .await?;
        Ok(())
    }

    // ============================================================
    // v2 — Budgets
    // ============================================================

    pub async fn list_budgets(&self) -> Result<Vec<Budget>> {
        let rows = self
            .conn
            .query("SELECT * FROM budgets ORDER BY name", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn get_budget(&self, id: i64) -> Result<Option<Budget>> {
        let rows = self
            .conn
            .query("SELECT * FROM budgets WHERE id = ?1", params![id])
            .await?;
        map_opt(rows).await
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
        self.conn
            .execute(
                "INSERT INTO budgets (name, category_id, amount_cents, period, currency, rollover,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    name,
                    category_id,
                    amount_cents,
                    period,
                    currency,
                    if rollover { 1_i64 } else { 0 },
                    now,
                    now,
                ],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn delete_budget(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM budgets WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    /// Total spend (withdrawal amount in cents) against the budget's category over the period
    /// containing `now`. Period is calendar-aligned for simplicity.
    pub async fn budget_period_spend(&self, budget: &Budget) -> Result<i64> {
        let (from, to) = period_range(&budget.period, Utc::now().timestamp());
        let mut rows = self
            .conn
            .query(
                "SELECT SUM(amount_cents) FROM transactions
                 WHERE category_id IS ?1 AND is_credit = 0 AND timestamp >= ?2 AND timestamp <= ?3",
                params![budget.category_id, from, to],
            )
            .await?;
        let row = rows.next().await?.context("sum query produced no row")?;
        Ok(row.get::<Option<i64>>(0)?.unwrap_or(0))
    }

    // ============================================================
    // v2 — Bills
    // ============================================================

    pub async fn list_bills(&self) -> Result<Vec<Bill>> {
        let rows = self
            .conn
            .query("SELECT * FROM bills ORDER BY next_expected_date ASC", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn list_bills_due_within(&self, within_days: i64) -> Result<Vec<Bill>> {
        let now = Utc::now().timestamp();
        let cutoff = now + within_days * 86_400;
        let rows = self
            .conn
            .query(
                "SELECT * FROM bills
                 WHERE enabled = 1 AND next_expected_date IS NOT NULL
                   AND next_expected_date BETWEEN ?1 AND ?2
                 ORDER BY next_expected_date ASC",
                params![now - 86_400, cutoff],
            )
            .await?;
        map_rows(rows).await
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
        self.conn
            .execute(
                "INSERT INTO bills (name, expected_amount_min_cents, expected_amount_max_cents,
                    currency, repeat_freq, next_expected_date, match_description_regex,
                    source_recurring_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    name,
                    amount_min_cents,
                    amount_max_cents,
                    currency,
                    repeat_freq,
                    next_expected_date,
                    match_description_regex,
                    source_recurring_id,
                    now,
                    now,
                ],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn delete_bill(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM bills WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    /// Find the bill auto-created for a given recurring mandate (DD / standing order),
    /// so re-syncs update it in place instead of inserting a duplicate every hour.
    pub async fn bill_id_for_recurring(&self, recurring_id: i64) -> Result<Option<i64>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM bills WHERE source_recurring_id = ?1 LIMIT 1",
                params![recurring_id],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.get::<i64>(0)?)),
            None => Ok(None),
        }
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
        self.conn
            .execute(
                "UPDATE bills SET expected_amount_min_cents = ?1, expected_amount_max_cents = ?2,
                    next_expected_date = ?3, updated_at = ?4 WHERE id = ?5",
                params![
                    amount_min_cents,
                    amount_max_cents,
                    next_expected_date,
                    Utc::now().timestamp(),
                    id,
                ],
            )
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
        let mut rows = self
            .conn
            .query(
                "SELECT amount_cents, timestamp FROM transactions
                 WHERE account_id = ?1 AND is_credit = 0
                   AND (description LIKE ?2 OR merchant_name LIKE ?3)
                 ORDER BY timestamp DESC LIMIT 1",
                params![account_id, name_like, name_like],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some((row.get::<i64>(0)?, row.get::<i64>(1)?))),
            None => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_bill(
        &self,
        id: i64,
        name: Option<&str>,
        amount_min_cents: Option<i64>,
        amount_max_cents: Option<i64>,
        next_expected_date: Option<i64>,
        match_description_regex: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "UPDATE bills SET
                    name = COALESCE(?1, name),
                    expected_amount_min_cents = COALESCE(?2, expected_amount_min_cents),
                    expected_amount_max_cents = COALESCE(?3, expected_amount_max_cents),
                    next_expected_date = COALESCE(?4, next_expected_date),
                    match_description_regex = COALESCE(?5, match_description_regex),
                    enabled = COALESCE(?6, enabled),
                    updated_at = ?7
                 WHERE id = ?8",
                params![
                    name,
                    amount_min_cents,
                    amount_max_cents,
                    next_expected_date,
                    match_description_regex,
                    enabled.map(|b| if b { 1_i64 } else { 0 }),
                    now,
                    id,
                ],
            )
            .await?;
        Ok(())
    }

    // ============================================================
    // Reminders / checklists
    // ============================================================

    pub async fn list_reminders(&self) -> Result<Vec<Reminder>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM reminders WHERE archived = 0 ORDER BY due_at ASC",
                (),
            )
            .await?;
        map_rows(rows).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_reminder(
        &self,
        title: &str,
        notes: Option<&str>,
        freq: &str,
        every_n: i64,
        anchor_day: Option<i64>,
        due_at: i64,
        notify_before: i64,
        notify_enabled: bool,
    ) -> Result<i64> {
        const FREQS: [&str; 4] = ["hours", "days", "weeks", "months"];
        if !FREQS.contains(&freq) {
            anyhow::bail!("invalid freq '{freq}' (expected hours|days|weeks|months)");
        }
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "INSERT INTO reminders (title, notes, freq, every_n, anchor_day, due_at,
                    notify_before, notify_enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    title,
                    notes,
                    freq,
                    every_n,
                    anchor_day,
                    due_at,
                    notify_before,
                    if notify_enabled { 1_i64 } else { 0 },
                    now,
                    now,
                ],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn update_reminder(
        &self,
        id: i64,
        title: Option<&str>,
        notes: Option<&str>,
        notify_before: Option<i64>,
        notify_enabled: Option<bool>,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "UPDATE reminders SET
                    title = COALESCE(?1, title),
                    notes = COALESCE(?2, notes),
                    notify_before = COALESCE(?3, notify_before),
                    notify_enabled = COALESCE(?4, notify_enabled),
                    updated_at = ?5
                 WHERE id = ?6",
                params![
                    title,
                    notes,
                    notify_before,
                    notify_enabled.map(|b| if b { 1_i64 } else { 0 }),
                    now,
                    id,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn tick_reminder(&self, id: i64) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "UPDATE reminders SET completed_at = ?1, updated_at = ?2 WHERE id = ?3",
                params![now, now, id],
            )
            .await?;
        Ok(())
    }

    pub async fn untick_reminder(&self, id: i64) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "UPDATE reminders SET completed_at = NULL, updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_reminder(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM reminders WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    /// Active, enabled reminders within their notify window not yet ticked or pinged this period.
    pub async fn reminders_to_notify(&self, now: i64) -> Result<Vec<Reminder>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM reminders
                 WHERE archived = 0 AND notify_enabled = 1
                   AND completed_at IS NULL AND notified_at IS NULL
                   AND ?1 >= due_at - notify_before",
                params![now],
            )
            .await?;
        map_rows(rows).await
    }

    pub async fn mark_reminder_notified(&self, id: i64, now: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE reminders SET notified_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .await?;
        Ok(())
    }

    /// Reminders whose current period has elapsed and need rolling to the next deadline.
    pub async fn reminders_to_roll(&self, now: i64) -> Result<Vec<Reminder>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM reminders WHERE archived = 0 AND ?1 >= due_at",
                params![now],
            )
            .await?;
        map_rows(rows).await
    }

    pub async fn roll_reminder(&self, id: i64, next_due: i64) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn
            .execute(
                "UPDATE reminders SET due_at = ?1, completed_at = NULL, notified_at = NULL,
                    updated_at = ?2 WHERE id = ?3",
                params![next_due, now, id],
            )
            .await?;
        Ok(())
    }

    // ============================================================
    // v2 — Holdings / brokers / quotes
    // ============================================================

    pub async fn list_brokers(&self) -> Result<Vec<Broker>> {
        let rows = self
            .conn
            .query("SELECT * FROM brokers ORDER BY name", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn upsert_broker(
        &self,
        name: &str,
        kind: &str,
        currency: &str,
        notes: Option<&str>,
    ) -> Result<i64> {
        let mut rows = self
            .conn
            .query(
                "INSERT INTO brokers (name, kind, currency, notes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET
                    kind = excluded.kind,
                    currency = excluded.currency,
                    notes = excluded.notes
                 RETURNING id",
                params![name, kind, currency, notes, Utc::now().timestamp()],
            )
            .await?;
        let row = rows.next().await?.context("upsert_broker RETURNING produced no row")?;
        Ok(row.get::<i64>(0)?)
    }

    pub async fn delete_broker(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM brokers WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    pub async fn list_holdings(&self) -> Result<Vec<Holding>> {
        let rows = self
            .conn
            .query("SELECT * FROM holdings WHERE enabled = 1 ORDER BY symbol", ())
            .await?;
        map_rows(rows).await
    }

    pub async fn list_holdings_for_broker(&self, broker_id: i64) -> Result<Vec<Holding>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM holdings WHERE broker_id = ?1 AND enabled = 1 ORDER BY symbol",
                params![broker_id],
            )
            .await?;
        map_rows(rows).await
    }

    pub async fn get_holding(&self, id: i64) -> Result<Option<Holding>> {
        let rows = self
            .conn
            .query("SELECT * FROM holdings WHERE id = ?1", params![id])
            .await?;
        map_opt(rows).await
    }

    pub async fn distinct_symbols(&self) -> Result<Vec<String>> {
        let mut rows = self
            .conn
            .query("SELECT DISTINCT symbol FROM holdings WHERE enabled = 1", ())
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row.get::<String>(0)?);
        }
        Ok(out)
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
        let mut rows = self
            .conn
            .query(
                "INSERT INTO holdings (broker_id, symbol, asset_class, quantity, avg_cost_per_unit,
                    currency, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(broker_id, symbol) DO UPDATE SET
                    asset_class = excluded.asset_class,
                    quantity = excluded.quantity,
                    avg_cost_per_unit = excluded.avg_cost_per_unit,
                    currency = excluded.currency,
                    name = COALESCE(excluded.name, holdings.name),
                    updated_at = excluded.updated_at
                 RETURNING id",
                params![
                    broker_id,
                    symbol,
                    asset_class,
                    quantity,
                    avg_cost,
                    currency,
                    name,
                    now,
                    now,
                ],
            )
            .await?;
        let row = rows.next().await?.context("upsert_holding RETURNING produced no row")?;
        Ok(row.get::<i64>(0)?)
    }

    pub async fn delete_holding(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM holdings WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    pub async fn touch_holding_synced(&self, id: i64, name: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE holdings SET last_synced_at = ?1, name = COALESCE(?2, name) WHERE id = ?3",
                params![Utc::now().timestamp(), name, id],
            )
            .await?;
        Ok(())
    }

    pub async fn list_activities(&self, holding_id: i64) -> Result<Vec<HoldingActivity>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM holding_activities WHERE holding_id = ?1 ORDER BY timestamp DESC",
                params![holding_id],
            )
            .await?;
        map_rows(rows).await
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
        validate_activity(activity_type, quantity, price_per_unit, fee)?;
        self.conn
            .execute(
                "INSERT INTO holding_activities (holding_id, activity_type, timestamp, quantity,
                    price_per_unit, fee, currency, notes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    holding_id,
                    activity_type,
                    timestamp,
                    quantity,
                    price_per_unit,
                    fee,
                    currency,
                    notes,
                    Utc::now().timestamp(),
                ],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub async fn delete_activity(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM holding_activities WHERE id = ?1", params![id])
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
        validate_activity(activity_type, quantity, price_per_unit, fee)?;
        let holding_id = self.holding_id_for_activity(id).await?;
        if holding_id.is_none() {
            return Ok(None);
        }
        self.conn
            .execute(
                "UPDATE holding_activities SET activity_type = ?1, timestamp = ?2, quantity = ?3,
                    price_per_unit = ?4, fee = ?5, notes = ?6 WHERE id = ?7",
                params![activity_type, timestamp, quantity, price_per_unit, fee, notes, id],
            )
            .await?;
        Ok(holding_id)
    }

    /// The holding a given activity belongs to (None if the activity row is gone). Backs the
    /// holdings route handlers so they never touch a raw connection.
    pub async fn holding_id_for_activity(&self, activity_id: i64) -> Result<Option<i64>> {
        let mut rows = self
            .conn
            .query(
                "SELECT holding_id FROM holding_activities WHERE id = ?1",
                params![activity_id],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.get::<i64>(0)?)),
            None => Ok(None),
        }
    }

    /// Earliest BUY/TRANSFER_IN activity timestamp across all holdings, or None if there are no
    /// such activities. Used for "performance since I started investing".
    pub async fn earliest_activity_ts(&self) -> Result<Option<i64>> {
        let mut rows = self
            .conn
            .query(
                "SELECT MIN(timestamp) FROM holding_activities
                 WHERE activity_type IN ('BUY','TRANSFER_IN')",
                (),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.get::<Option<i64>>(0)?),
            None => Ok(None),
        }
    }

    /// Best-effort: stamp `last_synced_at = now` on every holding for a symbol (UI freshness).
    pub async fn touch_holdings_synced_for_symbol(&self, symbol: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE holdings SET last_synced_at = strftime('%s','now') WHERE symbol = ?1",
                params![symbol],
            )
            .await?;
        Ok(())
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
        self.conn
            .execute(
                "UPDATE holdings SET quantity = ?1, avg_cost_per_unit = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![qty, avg, Utc::now().timestamp(), holding_id],
            )
            .await?;
        Ok(())
    }

    pub async fn latest_quote(&self, symbol: &str) -> Result<Option<LatestQuote>> {
        let rows = self
            .conn
            .query(
                "SELECT * FROM latest_quotes WHERE symbol = ?1",
                params![symbol],
            )
            .await?;
        map_opt(rows).await
    }

    pub async fn all_latest_quotes(&self) -> Result<Vec<LatestQuote>> {
        let rows = self
            .conn
            .query("SELECT * FROM latest_quotes ORDER BY symbol", ())
            .await?;
        map_rows(rows).await
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
        self.conn
            .execute(
                "INSERT INTO latest_quotes (symbol, price, currency, fetched_at,
                    previous_close, day_change_pct, company_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(symbol) DO UPDATE SET
                    price = excluded.price,
                    currency = excluded.currency,
                    fetched_at = excluded.fetched_at,
                    previous_close = excluded.previous_close,
                    day_change_pct = excluded.day_change_pct,
                    company_name = COALESCE(excluded.company_name, latest_quotes.company_name)",
                params![
                    symbol,
                    price,
                    currency,
                    Utc::now().timestamp(),
                    previous_close,
                    day_change_pct,
                    company_name,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn mark_bill_paid_by_transaction(
        &self,
        bill_id: i64,
        transaction_id: i64,
        paid_ts: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO bill_payments (bill_id, transaction_id) VALUES (?1, ?2)",
                params![bill_id, transaction_id],
            )
            .await?;
        // Record the payment and roll the projected next-due date forward one period past the
        // payment we just matched, so the bill keeps pointing at a future date instead of
        // freezing on a stale value (the reason "Bills ahead" was silently empty).
        let freq: Option<String> = {
            let mut rows = self
                .conn
                .query(
                    "SELECT repeat_freq FROM bills WHERE id = ?1",
                    params![bill_id],
                )
                .await?;
            match rows.next().await? {
                Some(row) => Some(row.get::<String>(0)?),
                None => None,
            }
        };
        let next = freq
            .as_deref()
            .map(|f| advance_past(paid_ts, f, Utc::now().timestamp()));
        self.conn
            .execute(
                "UPDATE bills SET last_paid_date = ?1, next_expected_date = COALESCE(?2, next_expected_date),
                    updated_at = ?3 WHERE id = ?4",
                params![paid_ts, next, Utc::now().timestamp(), bill_id],
            )
            .await?;
        Ok(())
    }
}

/// Strip a leading `file:` or `sqlite:` scheme from a database URL down to an OS path.
/// Handles `file:///abs`, `file:rel`, `sqlite://x`, `sqlite:x`. A bare path is returned as-is.
fn strip_db_scheme(url: &str) -> String {
    for prefix in ["file://", "sqlite://", "file:", "sqlite:"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    url.to_string()
}

/// Guard activity writes (REST and MCP both land here): known type, finite non-negative
/// amounts. SQLite would happily store NaN/garbage and silently skew cost-basis maths.
fn validate_activity(
    activity_type: &str,
    quantity: f64,
    price_per_unit: Option<f64>,
    fee: f64,
) -> Result<()> {
    const TYPES: [&str; 8] = [
        "BUY",
        "SELL",
        "DIVIDEND",
        "SPLIT",
        "FEE",
        "INTEREST",
        "TRANSFER_IN",
        "TRANSFER_OUT",
    ];
    if !TYPES.contains(&activity_type) {
        anyhow::bail!("invalid activity_type '{activity_type}' (expected one of {TYPES:?})");
    }
    for (name, v) in [
        ("quantity", Some(quantity)),
        ("price_per_unit", price_per_unit),
        ("fee", Some(fee)),
    ] {
        if let Some(v) = v {
            if !v.is_finite() || v < 0.0 {
                anyhow::bail!("{name} must be a finite non-negative number");
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod roundtrip_tests {
    //! Independent financial-correctness net for the sqlx→libsql cutover: insert via the real Db
    //! methods, read back via the real Db methods, assert field-by-field. Targets the three
    //! flagged risk areas — encrypted BLOB columns, the Account dual-mapper (join-only columns),
    //! and the dynamic transaction SQL bind order — plus bool-as-int and NULL handling.
    use super::*;
    use crate::crypto::Crypto;
    use base64::engine::general_purpose::STANDARD as B64STD;
    use base64::Engine;

    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([9u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
    }

    #[tokio::test]
    async fn consent_blob_and_crypto_roundtrip() {
        let db = test_db().await;
        let c = db
            .upsert_consent(
                "barclays",
                "creds-123",
                Some("ob-barclays"),
                Some("Barclays"),
                "ACCESS-TOKEN-xyz",
                "REFRESH-TOKEN-abc",
                1_700_000_000,
                Some(1_800_000_000),
                "accounts balance",
            )
            .await
            .unwrap();
        assert_eq!(c.nickname, "barclays");
        assert_eq!(c.credentials_id, "creds-123");
        assert_eq!(c.provider_id.as_deref(), Some("ob-barclays"));
        assert_eq!(c.provider_display_name.as_deref(), Some("Barclays"));
        assert_eq!(c.expires_at, 1_700_000_000);
        assert_eq!(c.consent_expires_at, Some(1_800_000_000));
        assert_eq!(c.scopes, "accounts balance");
        assert_eq!(c.enabled, 1);
        // Encrypted BLOB columns are populated (the catastrophic-if-wrong path).
        assert!(!c.access_token_enc.is_empty());
        assert!(!c.access_token_nonce.is_empty());
        assert!(!c.refresh_token_enc.is_empty());
        assert!(!c.refresh_token_nonce.is_empty());
        // And decrypt to the original plaintext, through every read path.
        assert_eq!(db.decrypt_access_token(&c).unwrap(), "ACCESS-TOKEN-xyz");
        assert_eq!(db.decrypt_refresh_token(&c).unwrap(), "REFRESH-TOKEN-abc");
        let by_nick = db.get_consent_by_nickname("barclays").await.unwrap().unwrap();
        assert_eq!(db.decrypt_access_token(&by_nick).unwrap(), "ACCESS-TOKEN-xyz");
        let by_id = db.get_consent(c.id).await.unwrap().unwrap();
        assert_eq!(db.decrypt_refresh_token(&by_id).unwrap(), "REFRESH-TOKEN-abc");
    }

    #[tokio::test]
    async fn consent_null_optionals_map_to_none() {
        let db = test_db().await;
        let c = db
            .upsert_consent("monzo", "creds-9", None, None, "at", "rt", 123, None, "accounts")
            .await
            .unwrap();
        assert_eq!(c.provider_id, None);
        assert_eq!(c.provider_display_name, None);
        assert_eq!(c.consent_expires_at, None);
        assert_eq!(c.last_sync_at, None);
        assert_eq!(c.last_sync_status, None);
        assert_eq!(c.last_sync_error, None);
    }

    #[tokio::test]
    async fn account_dual_mapper_base_vs_join() {
        let db = test_db().await;
        let c = db
            .upsert_consent("acct-bank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        let a = db
            .upsert_account(
                c.id,
                "tl-acc-1",
                "transaction",
                "Current Account",
                Some("GB00BANK"),
                Some("01-02-03"),
                Some("12345678"),
                None,
                "GBP",
            )
            .await
            .unwrap();
        // Base SELECT: the join-only columns are ABSENT → must map to None (not error/panic).
        let base = db.list_accounts_for_consent(c.id).await.unwrap();
        assert_eq!(base.len(), 1);
        let ba = &base[0];
        assert_eq!(ba.truelayer_id, "tl-acc-1");
        assert_eq!(ba.display_name, "Current Account");
        assert_eq!(ba.iban.as_deref(), Some("GB00BANK"));
        assert_eq!(ba.sort_code.as_deref(), Some("01-02-03"));
        assert_eq!(ba.account_number.as_deref(), Some("12345678"));
        assert_eq!(ba.card_last4, None);
        assert_eq!(ba.enabled, 1);
        assert_eq!(ba.consent_nickname, None, "base query must not populate join-only column");
        assert_eq!(ba.pending_net_cents, None, "base query must not populate computed column");
        // Join SELECT: the same columns are PRESENT and must populate.
        let joined = db.list_all_enabled_accounts().await.unwrap();
        assert_eq!(joined.len(), 1);
        let ja = &joined[0];
        assert_eq!(ja.id, a.id);
        assert_eq!(ja.consent_nickname.as_deref(), Some("acct-bank"));
        assert_eq!(ja.pending_net_cents, Some(0), "no pending txns => 0, not None");
    }

    #[tokio::test]
    async fn transaction_fields_bools_and_nulls() {
        let db = test_db().await;
        let c = db
            .upsert_consent("txbank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        let a = db
            .upsert_account(c.id, "tl-acc", "transaction", "Acct", None, None, None, None, "GBP")
            .await
            .unwrap();
        let id = db
            .upsert_transaction(
                a.id,
                "txn-1",
                1_650_000_000,
                "SALARY",
                250_000,
                "GBP",
                true,
                false,
                Some("ACME LTD"),
                None,
                Some("Acme Payroll"),
                None,
                Some("note"),
                None,
            )
            .await
            .unwrap();
        let t = db.get_transaction(id).await.unwrap().unwrap();
        assert_eq!(t.account_id, a.id);
        assert_eq!(t.provider_txn_id, "txn-1");
        assert_eq!(t.timestamp, 1_650_000_000);
        assert_eq!(t.description, "SALARY");
        assert_eq!(t.amount_cents, 250_000);
        assert_eq!(t.is_credit, 1, "bool true stored/read as i64 1");
        assert_eq!(t.is_pending, 0, "bool false stored/read as i64 0");
        assert_eq!(t.merchant_name.as_deref(), Some("ACME LTD"));
        assert_eq!(t.counterparty_iban, None, "NULL optional maps to None");
        assert_eq!(t.counterparty_name.as_deref(), Some("Acme Payroll"));
        assert_eq!(t.category_id, None);
        assert_eq!(t.notes.as_deref(), Some("note"));
        assert_eq!(t.raw_json, None);
    }

    #[tokio::test]
    async fn list_transactions_dynamic_filters() {
        let db = test_db().await;
        let c = db
            .upsert_consent("fbank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        let a1 = db
            .upsert_account(c.id, "acc-1", "transaction", "A1", None, None, None, None, "GBP")
            .await
            .unwrap();
        let a2 = db
            .upsert_account(c.id, "acc-2", "transaction", "A2", None, None, None, None, "GBP")
            .await
            .unwrap();
        // a1: debit COFFEE 500 @1000 ; credit TESCO REFUND 1500 @2000
        db.upsert_transaction(a1.id, "d1", 1000, "COFFEE", 500, "GBP", false, false, None, None, None, None, None, None).await.unwrap();
        db.upsert_transaction(a1.id, "c1", 2000, "TESCO REFUND", 1500, "GBP", true, false, Some("TESCO"), None, None, None, None, None).await.unwrap();
        // a2: debit TESCO STORES 9999 @1500
        db.upsert_transaction(a2.id, "d2", 1500, "TESCO STORES", 9999, "GBP", false, false, Some("TESCO"), None, None, None, None, None).await.unwrap();

        let ids = |v: &[Transaction]| v.iter().map(|t| t.provider_txn_id.clone()).collect::<Vec<_>>();

        // No filters: all three, newest first.
        let all = db.list_transactions(None, None, None, None, None, None, None, None, 100, 0).await.unwrap();
        assert_eq!(ids(&all), vec!["c1", "d2", "d1"], "ORDER BY timestamp DESC");

        // Account filter.
        let only_a1 = db.list_transactions(Some(&[a1.id]), None, None, None, None, None, None, None, 100, 0).await.unwrap();
        assert_eq!(only_a1.len(), 2);
        assert!(only_a1.iter().all(|t| t.account_id == a1.id));

        // is_credit = true.
        let credits = db.list_transactions(None, None, None, None, None, None, Some(true), None, 100, 0).await.unwrap();
        assert_eq!(ids(&credits), vec!["c1"]);

        // Time window [1200,1800] → only d2 @1500.
        let windowed = db.list_transactions(None, None, Some(1200), Some(1800), None, None, None, None, 100, 0).await.unwrap();
        assert_eq!(ids(&windowed), vec!["d2"]);

        // Amount range [1000,5000] → only c1 (1500).
        let amt = db.list_transactions(None, None, None, None, Some(1000), Some(5000), None, None, 100, 0).await.unwrap();
        assert_eq!(ids(&amt), vec!["c1"]);

        // LIKE "TESCO" matches description OR merchant → c1 + d2.
        let tesco = db.list_transactions(None, None, None, None, None, None, None, Some("TESCO"), 100, 0).await.unwrap();
        assert_eq!(tesco.len(), 2);

        // Combined multi-filter (exercises bind ordering): a2 + debit + TESCO → d2 only.
        let combo = db.list_transactions(Some(&[a2.id]), None, None, None, None, None, Some(false), Some("TESCO"), 100, 0).await.unwrap();
        assert_eq!(ids(&combo), vec!["d2"]);

        // count parity for the account filter.
        assert_eq!(db.count_transactions_filtered(Some(&[a1.id]), None, None, None).await.unwrap(), 2);

        // LIMIT/OFFSET: second-newest is d2.
        let paged = db.list_transactions(None, None, None, None, None, None, None, None, 1, 1).await.unwrap();
        assert_eq!(ids(&paged), vec!["d2"]);
    }

    #[test]
    fn strip_db_scheme_resolves_the_pi_url() {
        // The production Pi runs with TALLY_DATABASE_URL=sqlite:///app/data/state.db; this MUST
        // resolve to the existing absolute file, not a fresh empty DB at the wrong path.
        assert_eq!(strip_db_scheme("sqlite:///app/data/state.db"), "/app/data/state.db");
        assert_eq!(strip_db_scheme("file:///app/data/state.db"), "/app/data/state.db");
        assert_eq!(strip_db_scheme("sqlite:state.db"), "state.db");
        assert_eq!(strip_db_scheme("/app/data/state.db"), "/app/data/state.db");
        assert_eq!(strip_db_scheme("sqlite::memory:"), ":memory:");
    }

    #[tokio::test]
    async fn pending_net_cents_computed() {
        let db = test_db().await;
        let c = db
            .upsert_consent("pbank", "creds", None, None, "at", "rt", 1, None, "accounts")
            .await
            .unwrap();
        let a = db
            .upsert_account(c.id, "acc", "transaction", "A", None, None, None, None, "GBP")
            .await
            .unwrap();
        // pending credit +300, pending debit -100 → net +200; the settled txn is excluded.
        db.upsert_transaction(a.id, "p1", 1000, "in", 300, "GBP", true, true, None, None, None, None, None, None).await.unwrap();
        db.upsert_transaction(a.id, "p2", 1001, "out", 100, "GBP", false, true, None, None, None, None, None, None).await.unwrap();
        db.upsert_transaction(a.id, "s1", 1002, "settled", 9999, "GBP", false, false, None, None, None, None, None, None).await.unwrap();
        let joined = db.list_all_enabled_accounts().await.unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].pending_net_cents, Some(200));
    }
}
