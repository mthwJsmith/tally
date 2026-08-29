//! Per-consent sync loop.
//!
//! For each enabled consent:
//!  - Refresh access token if expired
//!  - List TrueLayer accounts + cards → upsert into our `accounts` table
//!  - Pull transactions (posted + pending) since last sync
//!  - Apply the rule engine to each new transaction (auto-categorise, auto-tag)
//!  - Persist to tally's own `transactions` table (full content, not just IDs)
//!  - Match against existing bills' regex → record bill_payments
//!  - Pull standing orders + direct debits → tally `recurring` + auto-promote to `bills`
//!  - Write a sync_log row throughout

use crate::clients::truelayer::TrueLayerClient;
use crate::db::Db;
use crate::models::truelayer::*;
use crate::models::Consent;
use crate::notifier::Notifier;
use crate::rules::{self, CompiledRules};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use tracing::{debug, error, info, warn};

pub struct Importer {
    pub db: Db,
    pub tl: TrueLayerClient,
    pub notifier: Notifier,
    pub ai: crate::ai::AiClient,
}

#[derive(Debug, Default)]
pub struct SyncResult {
    pub accounts_synced: i64,
    pub transactions_imported: i64,
    pub transactions_skipped: i64,
    pub recurring_imported: i64,
    pub errors: Vec<String>,
}

impl Importer {
    pub async fn sync_consent(&self, consent: &Consent) -> Result<SyncResult> {
        let log_id = self.db.start_sync_log(Some(consent.id)).await?;
        let mut result = SyncResult::default();

        let (tl_accounts, tl_cards) = match self.fetch_tl_inventory(consent).await {
            Ok(v) => v,
            Err(e) => {
                // A dead consent is not a transient sync failure — no retry ever fixes it, the
                // user has to walk the OAuth flow again. Give it a status of its own so the UI
                // can offer the re-link, and nudge once rather than every hour.
                let revoked = crate::clients::truelayer::is_consent_revoked(&e);
                let (status, msg) = if revoked {
                    (
                        "reauth",
                        format!(
                            "{} needs re-linking — the bank's 90-day open-banking consent has expired (or was revoked). Re-link it on the Banks page; the transactions already imported stay put.",
                            consent.nickname
                        ),
                    )
                } else {
                    ("fail", format!("failed to fetch TL inventory: {e:#}"))
                };
                error!("{msg}");
                let already_flagged = consent.last_sync_status.as_deref() == Some("reauth");
                self.db
                    .finish_sync_log(log_id, status, 0, 0, 0, 0, Some(&msg))
                    .await?;
                self.db
                    .touch_consent_sync_status(consent.id, status, Some(&msg))
                    .await?;
                if revoked && !already_flagged {
                    self.notifier
                        .send_telegram_text(
                            &format!(
                                "🔗 *Bank link expired* — {}\n\nIts 90-day consent ran out, so syncing has stopped. Re-link it on the Banks page to resume.",
                                consent.nickname
                            ),
                            false,
                        )
                        .await;
                }
                return Err(e);
            }
        };

        // Compile rules once for the whole sync.
        let rules_raw = self.db.list_rules(true).await.unwrap_or_default();
        let compiled_rules = CompiledRules::compile(rules_raw);

        // Pre-compile bill match regexes.
        let bills = self.db.list_bills().await.unwrap_or_default();
        let bill_matchers: Vec<(i64, Option<Regex>)> = bills
            .iter()
            .map(|b| {
                let re = b
                    .match_description_regex
                    .as_ref()
                    .and_then(|s| Regex::new(&format!("(?i){s}")).ok());
                (b.id, re)
            })
            .collect();

        for tl_acc in &tl_accounts {
            if let Err(e) = self
                .sync_account(consent, tl_acc, &compiled_rules, &bill_matchers, &mut result)
                .await
            {
                let msg = format!(
                    "account {} sync error: {e:#}",
                    tl_acc.display_name.as_deref().unwrap_or(&tl_acc.account_id)
                );
                warn!("{msg}");
                result.errors.push(msg);
            }
            result.accounts_synced += 1;
        }

        for tl_card in &tl_cards {
            if let Err(e) = self
                .sync_card(consent, tl_card, &compiled_rules, &bill_matchers, &mut result)
                .await
            {
                let msg = format!(
                    "card {} sync error: {e:#}",
                    tl_card.display_name.as_deref().unwrap_or(&tl_card.account_id)
                );
                warn!("{msg}");
                result.errors.push(msg);
            }
            result.accounts_synced += 1;
        }

        let status = if result.errors.is_empty() {
            "success"
        } else if result.transactions_imported > 0 || result.accounts_synced > 0 {
            "partial"
        } else {
            "fail"
        };
        let err_msg = if result.errors.is_empty() {
            None
        } else {
            Some(result.errors.join("\n"))
        };

        self.db
            .finish_sync_log(
                log_id,
                status,
                result.accounts_synced,
                result.transactions_imported,
                result.transactions_skipped,
                result.recurring_imported,
                err_msg.as_deref(),
            )
            .await?;
        self.db
            .touch_consent_sync_status(consent.id, status, err_msg.as_deref())
            .await?;

        info!(
            consent = %consent.nickname,
            accounts = result.accounts_synced,
            imported = result.transactions_imported,
            skipped = result.transactions_skipped,
            recurring = result.recurring_imported,
            "sync complete"
        );

        // Auto-categorise newly-imported transactions, if the user enabled it (Settings) and
        // OpenRouter is configured. Capped so a sync can't run up a big AI bill.
        if result.transactions_imported > 0
            && self
                .db
                .get_setting("ai_auto_categorise")
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some("1")
        {
            match self.ai.categorise_uncategorised(25).await {
                Ok(n) if n > 0 => info!(consent = %consent.nickname, categorised = n, "auto-categorised"),
                Ok(_) => {}
                Err(e) => warn!("auto-categorise failed: {e:#}"),
            }
        }

        // Snapshot every planning account's balance for today, so the Ahead graph accrues history
        // going forward (manual accounts included, since they can't be backfilled). Best-effort.
        let today_iso = Utc::now().format("%Y-%m-%d").to_string();
        if let Err(e) = self.db.snapshot_all_plan_accounts(&today_iso).await {
            warn!("balance snapshot failed: {e:#}");
        }

        Ok(result)
    }

    async fn fetch_tl_inventory(
        &self,
        consent: &Consent,
    ) -> Result<(Vec<TLAccount>, Vec<TLCard>)> {
        let accounts = self
            .tl
            .get_accounts(&self.db, consent)
            .await
            .context("get_accounts")?;
        let cards = self.tl.get_cards(&self.db, consent).await.unwrap_or_default();
        Ok((accounts, cards))
    }

    async fn sync_account(
        &self,
        consent: &Consent,
        tl: &TLAccount,
        rules: &CompiledRules,
        bill_matchers: &[(i64, Option<Regex>)],
        result: &mut SyncResult,
    ) -> Result<()> {
        let iban = tl.account_number.as_ref().and_then(|n| n.iban.clone());
        let sort_code = tl.account_number.as_ref().and_then(|n| n.sort_code.clone());
        let number = tl.account_number.as_ref().and_then(|n| n.number.clone());
        let currency = tl.currency.clone().unwrap_or_else(|| "GBP".to_string());
        let display_name = tl
            .display_name
            .clone()
            .unwrap_or_else(|| format!("Account {}", tl.account_id));

        let account = self
            .db
            .upsert_account(
                consent.id,
                &tl.account_id,
                "account",
                &display_name,
                iban.as_deref(),
                sort_code.as_deref(),
                number.as_deref(),
                None,
                &currency,
            )
            .await?;

        // Capture account_type (TRANSACTION/SAVINGS/etc). Best-effort.
        if tl.account_type.is_some() {
            let _ = self
                .db
                .update_account_metadata(account.id, tl.account_type.as_deref(), None, None)
                .await;
        }

        // Pull live balance — separate endpoint we previously had scope for but never called.
        match self
            .tl
            .get_account_balance(&self.db, consent, &tl.account_id)
            .await
        {
            Ok(b) => {
                let current = (b.current * 100.0).round() as i64;
                let available = b.available.map(|v| (v * 100.0).round() as i64);
                let overdraft = b.overdraft.map(|v| (v * 100.0).round() as i64);
                if let Err(e) = self
                    .db
                    .update_account_balance(account.id, current, available, overdraft)
                    .await
                {
                    warn!("update_account_balance failed: {e:#}");
                }
            }
            Err(e) => debug!("balance fetch failed for {}: {e:#}", tl.account_id),
        }

        // Posted + pending — 90-day rolling window. Banks expose ~90 days after the first hour
        // post-consent; the wider window lets monthly direct debits land in history so we can
        // backfill the DD amount/date that TrueLayer leaves null on the mandate itself.
        let to_date = Utc::now().format("%Y-%m-%d").to_string();
        let from_date = (Utc::now() - Duration::days(90)).format("%Y-%m-%d").to_string();
        let posted = self
            .tl
            .get_account_transactions(&self.db, consent, &tl.account_id, Some(&from_date), Some(&to_date))
            .await
            .unwrap_or_default();
        let pending = self
            .tl
            .get_account_pending(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();

        self.persist_transactions(&account, posted, false, rules, bill_matchers, result)
            .await;
        self.persist_transactions(&account, pending, true, rules, bill_matchers, result)
            .await;

        // Standing orders + direct debits → tally `recurring` + auto-promote to bills.
        let sos = self
            .tl
            .get_standing_orders(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();
        for so in sos {
            if let Err(e) = self.sync_standing_order(&account, &so, &currency, result).await {
                warn!("standing order sync error: {e:#}");
            }
        }
        let dds = self
            .tl
            .get_direct_debits(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();
        for dd in dds {
            if let Err(e) = self.sync_direct_debit(&account, &dd, &currency, result).await {
                warn!("direct debit sync error: {e:#}");
            }
        }

        // Reconcile any mandate we've seen before but TrueLayer didn't re-return this sync
        // (Nationwide's DD endpoint is flaky and often comes back empty). Without this, a DD
        // that was synced once but has no bill yet would silently never appear.
        self.reconcile_orphan_recurring(&account, &currency).await;

        Ok(())
    }

    /// Ensure every stored recurring mandate for this account has a bill, inferring the amount
    /// and next date from transaction history when the provider didn't supply them. Idempotent:
    /// skips mandates that already have a bill.
    async fn reconcile_orphan_recurring(&self, account: &crate::models::Account, currency: &str) {
        let rows = self
            .db
            .list_recurring_for_account(account.id)
            .await
            .unwrap_or_default();
        for r in rows {
            if r.firefly_bill_id.is_some() {
                continue;
            }
            if matches!(self.db.bill_id_for_recurring(r.id).await, Ok(Some(_))) {
                continue;
            }
            let inferred = self.infer_payment(account.id, &r.name).await;
            let amount_cents = r
                .amount
                .map(|a| (a * 100.0).round() as i64)
                .or(inferred.map(|(c, _)| c));
            let freq = freq_to_tally(r.frequency.as_deref()).unwrap_or("monthly");
            let last_ts = r
                .next_payment_date
                .as_deref()
                .and_then(parse_iso_date)
                .or(inferred.map(|(_, ts)| ts));
            let now = Utc::now().timestamp();
            let next_ts = last_ts.map(|t| crate::db::advance_past(t, freq, now));
            let display_name = if r.kind == "direct_debit" {
                format!("{} (DD)", r.name)
            } else {
                r.name.clone()
            };
            self.upsert_bill_for_recurring(r.id, &display_name, &r.name, amount_cents, currency, freq, next_ts)
                .await;
        }
    }

    async fn sync_card(
        &self,
        consent: &Consent,
        tl: &TLCard,
        rules: &CompiledRules,
        bill_matchers: &[(i64, Option<Regex>)],
        result: &mut SyncResult,
    ) -> Result<()> {
        let display_name = tl
            .display_name
            .clone()
            .unwrap_or_else(|| format!("Card {}", tl.account_id));
        let last4 = tl
            .partial_card_number
            .as_ref()
            .map(|s| s.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>());
        let currency = tl.currency.clone().unwrap_or_else(|| "GBP".to_string());

        let account = self
            .db
            .upsert_account(
                consent.id,
                &tl.account_id,
                "card",
                &display_name,
                None,
                None,
                None,
                last4.as_deref(),
                &currency,
            )
            .await?;

        // Card metadata — network (VISA/MASTERCARD/AMEX) + name on card.
        if tl.card_network.is_some() || tl.name_on_card.is_some() {
            let _ = self
                .db
                .update_account_metadata(
                    account.id,
                    Some("CARD"),
                    tl.card_network.as_deref(),
                    tl.name_on_card.as_deref(),
                )
                .await;
        }

        // Card-specific balance is richer than account balance — credit_limit, statement, payment due.
        match self
            .tl
            .get_card_balance(&self.db, consent, &tl.account_id)
            .await
        {
            Ok(b) => {
                let current = (b.current * 100.0).round() as i64;
                let available = b.available.map(|v| (v * 100.0).round() as i64);
                let credit_limit = b.credit_limit.map(|v| (v * 100.0).round() as i64);
                let last_stmt = b.last_statement_balance.map(|v| (v * 100.0).round() as i64);
                let payment_due = b.payment_due.map(|v| (v * 100.0).round() as i64);
                if let Err(e) = self
                    .db
                    .update_card_balance(
                        account.id,
                        current,
                        available,
                        credit_limit,
                        last_stmt,
                        b.last_statement_date.as_deref(),
                        payment_due,
                        b.payment_due_date.as_deref(),
                    )
                    .await
                {
                    warn!("update_card_balance failed: {e:#}");
                }
            }
            Err(e) => debug!("card balance fetch failed for {}: {e:#}", tl.account_id),
        }

        let to_date = Utc::now().format("%Y-%m-%d").to_string();
        let from_date = (Utc::now() - Duration::days(30)).format("%Y-%m-%d").to_string();
        let posted = self
            .tl
            .get_card_transactions(&self.db, consent, &tl.account_id, Some(&from_date), Some(&to_date))
            .await
            .unwrap_or_default();
        let pending = self
            .tl
            .get_card_pending(&self.db, consent, &tl.account_id)
            .await
            .unwrap_or_default();

        self.persist_transactions(&account, posted, false, rules, bill_matchers, result)
            .await;
        self.persist_transactions(&account, pending, true, rules, bill_matchers, result)
            .await;

        Ok(())
    }

    async fn persist_transactions(
        &self,
        account: &crate::models::Account,
        txns: Vec<TLTransaction>,
        is_pending: bool,
        rules: &CompiledRules,
        bill_matchers: &[(i64, Option<Regex>)],
        result: &mut SyncResult,
    ) {
        for txn in txns {
            // De-dup gate: have we seen this provider txn id before?
            match self.db.is_txn_seen(account.id, &txn.transaction_id).await {
                Ok(true) => {
                    result.transactions_skipped += 1;
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!("is_txn_seen lookup failed: {e:#}");
                    continue;
                }
            }

            let is_credit = !txn.transaction_type.eq_ignore_ascii_case("DEBIT");
            let amount_cents = (txn.amount.abs() * 100.0).round() as i64;

            // TrueLayer timestamps are ISO 8601 — parse to unix.
            let ts = DateTime::parse_from_rfc3339(&txn.timestamp)
                .map(|d| d.timestamp())
                .unwrap_or_else(|_| Utc::now().timestamp());

            let cp_iban = txn.meta.as_ref().and_then(|m| m.counter_party_iban.clone());
            let cp_name = txn
                .meta
                .as_ref()
                .and_then(|m| m.counter_party_preferred_name.clone())
                .or_else(|| txn.merchant_name.clone());
            let raw_json = serde_json::to_string(&serde_json::json!({
                "description": txn.description,
                "category": txn.transaction_category,
                "classification": txn.transaction_classification,
                "merchant": txn.merchant_name,
                "currency": txn.currency,
            }))
            .ok();

            // Build a temporary Transaction-like view to feed the rules engine.
            // We don't have the db id yet (assigned on insert), so use a synthetic 0.
            let pseudo = crate::models::Transaction {
                id: 0,
                account_id: account.id,
                provider_txn_id: txn.transaction_id.clone(),
                timestamp: ts,
                description: txn.description.clone(),
                amount_cents,
                currency: txn.currency.clone(),
                is_credit: if is_credit { 1 } else { 0 },
                is_pending: if is_pending { 1 } else { 0 },
                merchant_name: txn.merchant_name.clone(),
                counterparty_iban: cp_iban.clone(),
                counterparty_name: cp_name.clone(),
                category_id: None,
                notes: None,
                raw_json: None,
                created_at: 0,
                updated_at: 0,
            };

            // Rule application — may set category, tags, notes.
            let eff = rules::apply(&pseudo, rules);

            // Insert.
            let new_id = match self
                .db
                .upsert_transaction(
                    account.id,
                    &txn.transaction_id,
                    ts,
                    &txn.description,
                    amount_cents,
                    &txn.currency,
                    is_credit,
                    is_pending,
                    txn.merchant_name.as_deref(),
                    cp_iban.as_deref(),
                    cp_name.as_deref(),
                    eff.set_category_id,
                    eff.set_notes.as_deref(),
                    raw_json.as_deref(),
                )
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    warn!("upsert transaction failed: {e:#}");
                    result
                        .errors
                        .push(format!("txn {}: {e:#}", txn.transaction_id));
                    continue;
                }
            };

            // Mark dedup so future syncs don't reprocess.
            let _ = self
                .db
                .record_txn_imported(account.id, &txn.transaction_id, Some(new_id), is_pending, None)
                .await;

            // Apply tag effects.
            for tid in &eff.add_tag_ids {
                let _ = self.db.tag_transaction(new_id, *tid).await;
            }
            for rid in &eff.rules_fired {
                let _ = self.db.bump_rule_applied(*rid).await;
            }

            // Try to link this transaction to a matching bill.
            if !is_credit {
                for (bill_id, re_opt) in bill_matchers {
                    let Some(re) = re_opt.as_ref() else { continue };
                    if re.is_match(&txn.description)
                        || re.is_match(txn.merchant_name.as_deref().unwrap_or(""))
                    {
                        let _ = self
                            .db
                            .mark_bill_paid_by_transaction(*bill_id, new_id, ts)
                            .await;
                    }
                }
            }

            result.transactions_imported += 1;
            self.notifier
                .notify_transaction(&txn, account, is_pending)
                .await;
        }
    }

    async fn sync_standing_order(
        &self,
        account: &crate::models::Account,
        so: &TLStandingOrder,
        currency: &str,
        result: &mut SyncResult,
    ) -> Result<()> {
        let tl_id = so.standing_order_id.clone().unwrap_or_else(|| {
            format!("so:{}:{}", account.truelayer_id, so.payee.clone().unwrap_or_default())
        });
        let name = so.payee.clone().unwrap_or_else(|| "Standing Order".to_string());
        let freq = freq_to_tally(so.frequency.as_deref()).unwrap_or("monthly");

        // Amount: prefer TrueLayer's, else infer from matching debits in transaction history.
        let tl_cents = so.next_payment_amount.map(|a| (a * 100.0).round() as i64);
        let inferred = self.infer_payment(account.id, &name).await;
        let amount_cents = tl_cents.or(inferred.map(|(c, _)| c));

        // Anchor date: TL's next_payment_date if present, else the last inferred payment ts.
        let anchor_ts = so
            .next_payment_date
            .as_deref()
            .and_then(parse_iso_date)
            .or(inferred.map(|(_, ts)| ts));
        let now = Utc::now().timestamp();
        let next_ts = anchor_ts.map(|t| crate::db::advance_past(t, freq, now));
        let next_iso = next_ts.map(iso_from_ts);

        let entry = self
            .db
            .upsert_recurring(
                account.id,
                &tl_id,
                "standing_order",
                &name,
                amount_cents.map(|c| c as f64 / 100.0),
                Some(currency),
                so.frequency.as_deref(),
                next_iso.as_deref().or(so.next_payment_date.as_deref()),
                None,
            )
            .await?;
        result.recurring_imported += 1;

        self.upsert_bill_for_recurring(entry.id, &name, &name, amount_cents, currency, freq, next_ts)
            .await;
        Ok(())
    }

    async fn sync_direct_debit(
        &self,
        account: &crate::models::Account,
        dd: &TLDirectDebit,
        currency: &str,
        result: &mut SyncResult,
    ) -> Result<()> {
        let tl_id = dd.direct_debit_id.clone().unwrap_or_else(|| {
            format!("dd:{}:{}", account.truelayer_id, dd.name.clone().unwrap_or_default())
        });
        let name = dd.name.clone().unwrap_or_else(|| "Direct Debit".to_string());

        // TrueLayer (Nationwide especially) usually leaves previous_payment_amount/date null on
        // the mandate. Fall back to the real transaction history to recover both.
        let tl_cents = dd.previous_payment_amount.map(|a| (a * 100.0).round() as i64);
        let inferred = self.infer_payment(account.id, &name).await;
        let amount_cents = tl_cents.or(inferred.map(|(c, _)| c));

        // Last payment date: TL's, else the inferred transaction ts. Project next as +1 month
        // (DDs are effectively monthly) rolled forward until it's in the future.
        let last_ts = dd
            .previous_payment_timestamp
            .as_deref()
            .and_then(parse_iso_date)
            .or(inferred.map(|(_, ts)| ts));
        let now = Utc::now().timestamp();
        let next_ts = last_ts.map(|t| crate::db::advance_past(t, "monthly", now));
        let next_iso = next_ts.map(iso_from_ts);

        let entry = self
            .db
            .upsert_recurring(
                account.id,
                &tl_id,
                "direct_debit",
                &name,
                amount_cents.map(|c| c as f64 / 100.0),
                Some(currency),
                Some("monthly"),
                next_iso.as_deref(),
                dd.status.as_deref(),
            )
            .await?;
        result.recurring_imported += 1;

        let display_name = format!("{name} (DD)");
        self.upsert_bill_for_recurring(entry.id, &display_name, &name, amount_cents, currency, "monthly", next_ts)
            .await;
        Ok(())
    }

    /// Recover a recurring payment's amount + last date from transaction history when the
    /// provider didn't supply them. Tries the full mandate name, then its leading token.
    async fn infer_payment(&self, account_id: i64, name: &str) -> Option<(i64, i64)> {
        let full = format!("%{name}%");
        if let Ok(Some(hit)) = self.db.infer_recurring_payment(account_id, &full).await {
            return Some(hit);
        }
        if let Some(tok) = leading_token(name) {
            let like = format!("%{tok}%");
            if let Ok(Some(hit)) = self.db.infer_recurring_payment(account_id, &like).await {
                return Some(hit);
            }
        }
        None
    }

    /// Create or update (idempotently across re-syncs) the bill backing a recurring mandate.
    /// Bills are created even when the amount is still unknown (0/0 → UI shows "amount unknown")
    /// so every DD / standing order stays visible in Bills, not silently dropped.
    async fn upsert_bill_for_recurring(
        &self,
        recurring_id: i64,
        display_name: &str,
        match_name: &str,
        amount_cents: Option<i64>,
        currency: &str,
        freq: &str,
        next_ts: Option<i64>,
    ) {
        let (min, max) = match amount_cents {
            Some(c) => ((c as f64 * 0.95).round() as i64, (c as f64 * 1.05).round() as i64),
            None => (0, 0),
        };
        match self.db.bill_id_for_recurring(recurring_id).await {
            Ok(Some(bid)) => {
                // When the provider can't tell us the amount, only advance the schedule —
                // never clobber an amount the user set by hand with 0/0.
                let res = if amount_cents.is_some() {
                    self.db.update_bill_schedule(bid, min, max, next_ts).await
                } else {
                    self.db.update_bill_next_date(bid, next_ts).await
                };
                if let Err(e) = res {
                    debug!("update bill {bid} for recurring {recurring_id} failed: {e:#}");
                }
            }
            Ok(None) => {
                let regex = Some(escape_for_regex(match_name));
                match self
                    .db
                    .create_bill(display_name, min, max, currency, freq, next_ts, regex.as_deref(), Some(recurring_id))
                    .await
                {
                    Ok(bid) => {
                        let _ = self.db.map_recurring_to_firefly_bill(recurring_id, bid).await;
                    }
                    Err(e) => debug!("auto-promote recurring {recurring_id} to bill failed: {e:#}"),
                }
            }
            Err(e) => debug!("bill_id_for_recurring lookup failed: {e:#}"),
        }
    }
}

fn freq_to_tally(tl_freq: Option<&str>) -> Option<&'static str> {
    match tl_freq?.to_uppercase().as_str() {
        "WEEKLY" | "EVRYWEEK" => Some("weekly"),
        "MONTHLY" | "EVRYMNTH" => Some("monthly"),
        "YEARLY" | "EVRYWORK" => Some("yearly"),
        "FORTNIGHTLY" | "EVRY2WEEK" => Some("fortnightly"),
        _ => Some("monthly"),
    }
}

fn escape_for_regex(s: &str) -> String {
    regex::escape(s)
}

/// Parse a TrueLayer date — either a full RFC3339 timestamp ("2018-09-17T00:00:00+01:00")
/// or a bare "YYYY-MM-DD" — to a unix timestamp.
fn parse_iso_date(d: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(d) {
        return Some(dt.timestamp());
    }
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}

fn iso_from_ts(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string()
}

/// The first "significant" whitespace token of a mandate name (length >= 4, trailing
/// punctuation trimmed) — used as a looser LIKE fallback when the full name doesn't match
/// a transaction description verbatim. e.g. "AQUA CREDIT CARD" -> "AQUA".
fn leading_token(name: &str) -> Option<String> {
    name.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '.'))
        .find(|t| t.len() >= 4)
        .map(|t| t.to_string())
}
